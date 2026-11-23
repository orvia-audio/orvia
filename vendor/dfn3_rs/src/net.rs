//! DeepFilterNet3 network for one 10 ms frame. Activations are frequency-major
//! `[F][C]` with 64 contiguous channels, so DFN3's permutes/flattens are free.

use crate::simd::{Simd, sigmoid};
use crate::weights::{Grouped, Gru, Mat, Model, Reader};

const C: usize = 64;
pub(crate) const NB_ERB: usize = 32;
pub(crate) const NB_DF: usize = 96;
pub(crate) const DF_ORDER: usize = 5;

type Taps = &'static [f32]; // [kernel][C]

struct Pointwise {
    taps: Taps,
    mat: Mat,
    bias: &'static [f32],
}

struct Pathway {
    scale: &'static [f32],
    bias: &'static [f32],
}

struct Weights {
    erb0: (Taps, &'static [f32]),
    erb: [Pointwise; 3],
    df0: Pointwise,
    df1: Pointwise,
    df_fc_emb: Grouped,
    enc_in: Grouped,
    enc_gru: Gru,
    enc_out: Grouped,
    lsnr: (&'static [f32], &'static [f32]),
    erb_in: Grouped,
    erb_gru: [Gru; 2],
    erb_out: Grouped,
    up: [(Pathway, Pointwise); 3],
    path0: Pathway,
    out0: (Taps, &'static [f32]),
    df_in: Grouped,
    df_gru: [Gru; 2],
    df_skip: Grouped,
    df_convp: ([Mat; DF_ORDER], &'static [f32]),
    df_out: Grouped,
}

impl Weights {
    fn new(model: Model) -> Self {
        let mut r = Reader::new(model);
        let pw = |r: &mut Reader, taps| Pointwise {
            taps: r.take(taps * C),
            mat: r.mat(C, C),
            bias: r.take(C),
        };
        let path = |r: &mut Reader| Pathway {
            scale: r.take(C),
            bias: r.take(C),
        };
        let w = Self {
            erb0: (r.take(9 * C), r.take(C)),
            erb: std::array::from_fn(|_| pw(&mut r, 3)),
            df0: pw(&mut r, 9),
            df1: pw(&mut r, 3),
            df_fc_emb: r.grouped(32, 96, 16),
            enc_in: r.grouped(16, 32, 16),
            enc_gru: r.gru(256),
            enc_out: r.grouped(16, 16, 32),
            lsnr: (r.take(512), r.take(1)),
            erb_in: {
                r.decoder = true;
                r.grouped(16, 32, 16)
            },
            erb_gru: [r.gru(256), r.gru(256)],
            erb_out: r.grouped(16, 16, 32),
            up: std::array::from_fn(|_| (path(&mut r), pw(&mut r, 3))),
            path0: path(&mut r),
            out0: (r.take(3 * C), r.take(1)),
            df_in: r.grouped(8, 64, 32),
            df_gru: [r.gru(256), r.gru(256)],
            df_skip: r.grouped(16, 32, 16),
            df_convp: (std::array::from_fn(|_| r.mat(C, 10)), r.take(10)),
            df_out: r.grouped(16, 16, 60),
        };
        r.finish();
        w
    }
}

#[derive(Clone, Copy)]
pub(crate) enum RecurrentStage {
    Encoder,
    Erb,
    Df,
}

pub(crate) struct Network {
    simd: Simd,
    w: Weights,
    // Streaming state.
    erb_hist: [[f32; NB_ERB + 2]; 3],
    spec_hist: [[[f32; NB_DF + 2]; 2]; 3],
    h_enc: [f32; 256],
    h_erb: [[f32; 256]; 2],
    h_df: [[f32; 256]; 2],
    c0: [f32; NB_DF * C],
    pending: [[f32; NB_DF * 16]; DF_ORDER], // projected FIR sums; advance only when DF runs
    c0_head: usize,
    // Scratch (encoder outputs persist until the decoders run).
    e0: [f32; 32 * C],
    e1: [f32; 16 * C],
    e2: [f32; 8 * C],
    e3: [f32; 8 * C],
    emb: [f32; 512],
    tmp: [f32; NB_DF * C],
    tmp2: [f32; 32 * C],
    c1: [f32; 48 * C],
    v512: [f32; 512],
    v256: [f32; 256],
    gi: [f32; 768],
    gh: [f32; 768],
    convp: [f32; NB_DF * 16],
    df_out: [f32; 15 * 60 + 64],
}

/// Depthwise 1x3 conv over frequency, zero padded: `out[f] = Σ_k w[k] * x[f*stride + k - 1]`.
fn depthwise(x: &[f32], fin: usize, taps: Taps, stride: usize, out: &mut [f32]) {
    for (f, o) in out
        .as_chunks_mut::<C>()
        .0
        .iter_mut()
        .take((fin - 1) / stride + 1)
        .enumerate()
    {
        o.fill(0.0);
        for k in 0..3 {
            let Some(i) = (f * stride + k).checked_sub(1).filter(|&i| i < fin) else {
                continue;
            };
            for ((o, &w), &x) in o.iter_mut().zip(&taps[k * C..][..C]).zip(&x[i * C..][..C]) {
                *o += w * x;
            }
        }
    }
}

/// Depthwise 1x3 transposed conv with frequency stride 2 (`fin` -> `2 * fin`).
fn depthwise_up(x: &[f32], fin: usize, taps: Taps, out: &mut [f32]) {
    let (w0, w1, w2) = (&taps[..C], &taps[C..2 * C], &taps[2 * C..3 * C]);
    for j in 0..fin {
        let (even, odd) = out[2 * j * C..][..2 * C].split_at_mut(C);
        let (cur, next) = (&x[j * C..][..C], x.get((j + 1) * C..(j + 2) * C));
        for c in 0..C {
            even[c] = w1[c] * cur[c];
            odd[c] = w2[c] * cur[c] + next.map_or(0.0, |n| w0[c] * n[c]);
        }
    }
}

/// `relu(x * scale + bias) + add`, per channel (the decoder's 1x1 depthwise pathway).
fn pathway(p: &Pathway, x: &[f32], add: &mut [f32]) {
    for (row, a) in x
        .as_chunks::<C>()
        .0
        .iter()
        .zip(add.as_chunks_mut::<C>().0.iter_mut())
    {
        for c in 0..C {
            a[c] += (row[c] * p.scale[c] + p.bias[c]).max(0.0);
        }
    }
}

impl Network {
    pub fn new(model: Model) -> Self {
        Self {
            simd: Simd::new(),
            w: Weights::new(model),
            erb_hist: [[0.0; NB_ERB + 2]; 3],
            spec_hist: [[[0.0; NB_DF + 2]; 2]; 3],
            h_enc: [0.0; 256],
            h_erb: [[0.0; 256]; 2],
            h_df: [[0.0; 256]; 2],
            c0: [0.0; NB_DF * C],
            pending: [[0.0; NB_DF * 16]; DF_ORDER],
            c0_head: 0,
            e0: [0.0; 32 * C],
            e1: [0.0; 16 * C],
            e2: [0.0; 8 * C],
            e3: [0.0; 8 * C],
            emb: [0.0; 512],
            tmp: [0.0; NB_DF * C],
            tmp2: [0.0; 32 * C],
            c1: [0.0; 48 * C],
            v512: [0.0; 512],
            v256: [0.0; 256],
            gi: [0.0; 768],
            gh: [0.0; 768],
            convp: [0.0; NB_DF * 16],
            df_out: [0.0; 15 * 60 + 64],
        }
    }

    pub fn reset(&mut self) {
        self.erb_hist = [[0.0; NB_ERB + 2]; 3];
        self.spec_hist = [[[0.0; NB_DF + 2]; 2]; 3];
        self.h_enc.fill(0.0);
        self.h_erb = [[0.0; 256]; 2];
        self.h_df = [[0.0; 256]; 2];
        self.c0.fill(0.0);
        self.pending = [[0.0; NB_DF * 16]; DF_ORDER];
        self.c0_head = 0;
    }

    pub fn copy_history_from(&mut self, source: &Self) {
        self.erb_hist = source.erb_hist;
        self.spec_hist = source.spec_hist;
        self.h_enc = source.h_enc;
        self.h_erb = source.h_erb;
        self.h_df = source.h_df;
        self.c0 = source.c0;
        self.pending = source.pending;
        self.c0_head = source.c0_head;
        // The transposed convolutions can read a padding row beyond the previous stage's output.
        self.tmp2 = source.tmp2;
    }

    fn grouped(
        simd: Simd,
        g: &Grouped,
        x: &[f32],
        out: &mut [f32],
        stride: usize,
        acc: bool,
        relu: bool,
    ) {
        for i in 0..g.groups {
            let k = g.mat.k;
            simd.gemm(
                1,
                &x[i * k..][..k],
                k,
                &g.group(i),
                None,
                acc,
                relu,
                &mut out[i * stride..],
                0,
            );
        }
    }

    fn gru(
        simd: Simd,
        g: &Gru,
        x: &[f32],
        h: &mut [f32; 256],
        gi: &mut [f32; 768],
        gh: &mut [f32; 768],
    ) {
        simd.gemm(1, x, 0, &g.w, Some(g.bias_w), false, false, gi, 0);
        simd.gemm(1, h, 0, &g.r, Some(g.bias_r), false, false, gh, 0);
        simd.gru_gates(gi, gh, h);
    }

    pub fn can_batch_with(&self, other: &Self) -> bool {
        // The SNR tensor is a fixed view into the immutable model blob.
        self.simd.supports_pair() && std::ptr::eq(self.w.lsnr.0, other.w.lsnr.0)
    }

    pub fn recurrent_pair(&mut self, right: &mut Self, stage: RecurrentStage) {
        let simd = self.simd;
        let layers = if matches!(stage, RecurrentStage::Encoder) {
            1
        } else {
            2
        };
        for layer in 0..layers {
            let (w, left_x, left_h) = self.layer(stage, layer);
            let (_, right_x, right_h) = right.layer(stage, layer);
            let mut gi = [[0.; 768]; 2];
            let mut gh = [[0.; 768]; 2];
            simd.gemm(
                2,
                [*left_x, *right_x].as_flattened(),
                256,
                &w.w,
                Some(w.bias_w),
                false,
                false,
                gi.as_flattened_mut(),
                768,
            );
            simd.gemm(
                2,
                [*left_h, *right_h].as_flattened(),
                256,
                &w.r,
                Some(w.bias_r),
                false,
                false,
                gh.as_flattened_mut(),
                768,
            );
            simd.gru_gates(&gi[0], &gh[0], left_h);
            simd.gru_gates(&gi[1], &gh[1], right_h);
        }
    }

    fn layer(
        &mut self,
        stage: RecurrentStage,
        layer: usize,
    ) -> (Gru, &[f32; 256], &mut [f32; 256]) {
        let (weights, states) = match stage {
            RecurrentStage::Encoder => return (self.w.enc_gru, &self.v256, &mut self.h_enc),
            RecurrentStage::Erb => (&self.w.erb_gru, &mut self.h_erb),
            RecurrentStage::Df => (&self.w.df_gru, &mut self.h_df),
        };
        let [first, second] = states;
        if layer == 0 {
            (weights[0], &self.v256, first)
        } else {
            (weights[1], first, second)
        }
    }

    /// Runs the encoder on the newest features and returns the local SNR in dB.
    pub fn encode(&mut self, erb: &[f32; NB_ERB], re: &[f32; NB_DF], im: &[f32; NB_DF]) -> f32 {
        self.encode_features(erb, re, im);
        let (s, w) = (self.simd, &self.w);
        Self::gru(
            s,
            &w.enc_gru,
            &self.v256,
            &mut self.h_enc,
            &mut self.gi,
            &mut self.gh,
        );
        self.encoded_snr()
    }

    pub fn encode_features(&mut self, erb: &[f32; NB_ERB], re: &[f32; NB_DF], im: &[f32; NB_DF]) {
        let (s, w) = (self.simd, &self.w);
        self.erb_hist.copy_within(1.., 0);
        self.erb_hist[2][1..=NB_ERB].copy_from_slice(erb);
        self.spec_hist.copy_within(1.., 0);
        self.spec_hist[2][0][1..=NB_DF].copy_from_slice(re);
        self.spec_hist[2][1][1..=NB_DF].copy_from_slice(im);

        // erb_conv0: 3x3 conv, 1 -> 64 channels.
        let (taps, bias) = w.erb0;
        for (f, o) in self.e0.as_chunks_mut::<C>().0.iter_mut().enumerate() {
            o.copy_from_slice(&bias[..C]);
            for (t, hist) in self.erb_hist.iter().enumerate() {
                for k in 0..3 {
                    let x = hist[f + k];
                    for (o, &w) in o.iter_mut().zip(&taps[(t * 3 + k) * C..][..C]) {
                        *o += w * x;
                    }
                }
            }
            o.iter_mut().for_each(|v| *v = v.max(0.0));
        }
        depthwise(&self.e0, 32, w.erb[0].taps, 2, &mut self.tmp);
        s.gemm(
            16,
            &self.tmp,
            C,
            &w.erb[0].mat,
            Some(w.erb[0].bias),
            false,
            true,
            &mut self.e1,
            C,
        );
        depthwise(&self.e1, 16, w.erb[1].taps, 2, &mut self.tmp);
        s.gemm(
            8,
            &self.tmp,
            C,
            &w.erb[1].mat,
            Some(w.erb[1].bias),
            false,
            true,
            &mut self.e2,
            C,
        );
        depthwise(&self.e2, 8, w.erb[2].taps, 1, &mut self.tmp);
        s.gemm(
            8,
            &self.tmp,
            C,
            &w.erb[2].mat,
            Some(w.erb[2].bias),
            false,
            true,
            &mut self.e3,
            C,
        );

        // df_conv0: 3x3 conv with 2 groups (re -> channels 0..32, im -> 32..64), then 1x1.
        let taps = w.df0.taps;
        for (f, o) in self.tmp.as_chunks_mut::<C>().0.iter_mut().enumerate() {
            o.fill(0.0);
            for (t, hist) in self.spec_hist.iter().enumerate() {
                for k in 0..3 {
                    let w = &taps[(t * 3 + k) * C..][..C];
                    for ((half, w), plane) in o
                        .as_chunks_mut::<{ C / 2 }>()
                        .0
                        .iter_mut()
                        .zip(w.as_chunks::<{ C / 2 }>().0.iter())
                        .zip(hist)
                    {
                        let x = plane[f + k];
                        half.iter_mut().zip(w).for_each(|(o, &w)| *o += w * x);
                    }
                }
            }
        }
        let c0 = &mut self.c0;
        s.gemm(
            NB_DF,
            &self.tmp,
            C,
            &w.df0.mat,
            Some(w.df0.bias),
            false,
            true,
            c0,
            C,
        );
        depthwise(c0, NB_DF, w.df1.taps, 2, &mut self.tmp);
        s.gemm(
            48,
            &self.tmp,
            C,
            &w.df1.mat,
            Some(w.df1.bias),
            false,
            true,
            &mut self.c1,
            C,
        );

        // Embedding: e3 (flattened [F][C]) + relu(df_fc_emb(c1)), then the squeezed GRU.
        self.emb.copy_from_slice(&self.e3);
        Self::grouped(s, &w.df_fc_emb, &self.c1, &mut self.v512, 16, false, true);
        self.emb
            .iter_mut()
            .zip(&self.v512)
            .for_each(|(e, c)| *e += c);
        Self::grouped(s, &w.enc_in, &self.emb, &mut self.v256, 16, false, true);
    }

    pub fn encoded_snr(&mut self) -> f32 {
        let (s, w) = (self.simd, &self.w);
        Self::grouped(s, &w.enc_out, &self.h_enc, &mut self.emb, 32, false, true);

        let (lw, lb) = w.lsnr;
        let dot = self.emb.iter().zip(lw).map(|(a, b)| a * b).sum::<f32>();
        sigmoid(dot + lb[0]) * 50.0 - 15.0
    }

    /// ERB decoder: per-band gains in [0, 1].
    pub fn erb_decode(&mut self, gains: &mut [f32; NB_ERB]) {
        self.prepare_erb();
        let (s, w) = (self.simd, &self.w);
        let [h0, h1] = &mut self.h_erb;
        Self::gru(s, &w.erb_gru[0], &self.v256, h0, &mut self.gi, &mut self.gh);
        Self::gru(s, &w.erb_gru[1], &h0[..], h1, &mut self.gi, &mut self.gh);
        self.finish_erb(gains);
    }

    pub fn prepare_erb(&mut self) {
        let (s, w) = (self.simd, &self.w);
        Self::grouped(s, &w.erb_in, &self.emb, &mut self.v256, 16, false, true);
    }

    pub fn finish_erb(&mut self, gains: &mut [f32; NB_ERB]) {
        let (s, w) = (self.simd, &self.w);
        let h1 = &self.h_erb[1];
        Self::grouped(s, &w.erb_out, &h1[..], &mut self.v512, 32, false, true);

        let pointwise = |p: &Pointwise, rows, tmp: &[f32], out: &mut [f32]| {
            s.gemm(rows, tmp, C, &p.mat, Some(p.bias), false, true, out, C)
        };
        // emb [8][64] + pathway(e3) -> convt3 (stride 1) -> [8][64]
        pathway(&w.up[0].0, &self.e3, &mut self.v512);
        depthwise(&self.v512, 8, w.up[0].1.taps, 1, &mut self.tmp);
        pointwise(&w.up[0].1, 8, &self.tmp, &mut self.tmp2);
        // + pathway(e2) -> convt2 (upsample) -> [16][64]
        pathway(&w.up[1].0, &self.e2, &mut self.tmp2[..8 * C]);
        depthwise_up(&self.tmp2, 8, w.up[1].1.taps, &mut self.tmp);
        pointwise(&w.up[1].1, 16, &self.tmp, &mut self.tmp2);
        // + pathway(e1) -> convt1 (upsample) -> [32][64]
        pathway(&w.up[2].0, &self.e1, &mut self.tmp2[..16 * C]);
        depthwise_up(&self.tmp2, 16, w.up[2].1.taps, &mut self.tmp);
        pointwise(&w.up[2].1, 32, &self.tmp, &mut self.tmp2);
        // + pathway(e0) -> 1x3 conv to one channel -> sigmoid
        pathway(&w.path0, &self.e0, &mut self.tmp2);
        let (taps, bias) = w.out0;
        for (f, g) in gains.iter_mut().enumerate() {
            let mut sum = bias[0];
            for k in 0..3 {
                let Some(i) = (f + k).checked_sub(1).filter(|&i| i < NB_ERB) else {
                    continue;
                };
                let row = &self.tmp2[i * C..][..C];
                sum += row
                    .iter()
                    .zip(&taps[k * C..][..C])
                    .map(|(a, b)| a * b)
                    .sum::<f32>();
            }
            *g = sigmoid(sum);
        }
    }

    /// DF decoder: complex filter coefficients `[NB_DF][DF_ORDER][re, im]`.
    pub fn df_decode(&mut self, coefs: &mut [f32; NB_DF * DF_ORDER * 2]) {
        self.prepare_df();
        let (s, w) = (self.simd, &self.w);
        let [h0, h1] = &mut self.h_df;
        Self::gru(s, &w.df_gru[0], &self.v256, h0, &mut self.gi, &mut self.gh);
        Self::gru(s, &w.df_gru[1], &h0[..], h1, &mut self.gi, &mut self.gh);
        self.finish_df(coefs);
    }

    pub fn prepare_df(&mut self) {
        let (s, w) = (self.simd, &self.w);
        Self::grouped(s, &w.df_in, &self.emb, &mut self.v256, 32, false, true);
    }

    pub fn finish_df(&mut self, coefs: &mut [f32; NB_DF * DF_ORDER * 2]) {
        let (s, w) = (self.simd, &self.w);
        let h1 = &self.h_df[1];
        self.v256.copy_from_slice(h1);
        Self::grouped(s, &w.df_skip, &self.emb, &mut self.v256, 16, true, false);

        // Transposed FIR: project the current c0 into this and four future outputs.
        // Each slot receives contributions in chronological order, oldest first.
        let (mats, bias) = &w.df_convp;
        for (tap, mat) in mats.iter().enumerate() {
            let slot = (self.c0_head + DF_ORDER - 1 - tap) % DF_ORDER;
            s.gemm(
                NB_DF,
                &self.c0,
                C,
                mat,
                None,
                true,
                false,
                &mut self.pending[slot],
                16,
            );
        }
        let ready = &mut self.pending[self.c0_head];
        for (i, out) in self.convp.iter_mut().enumerate() {
            *out = (ready[i] + bias[i % 16]).max(0.0);
        }
        ready.fill(0.0);
        self.c0_head = (self.c0_head + 1) % DF_ORDER;

        // Groups are written in ascending order; each overwrites the previous group's padding.
        Self::grouped(s, &w.df_out, &self.v256, &mut self.df_out, 60, false, false);
        s.tanh(&mut self.df_out[..NB_DF * 10]);
        for (f, c) in coefs.as_chunks_mut::<10>().0.iter_mut().enumerate() {
            for (j, c) in c.iter_mut().enumerate() {
                *c = self.df_out[f * 10 + j] + self.convp[f * 16 + j];
            }
        }
    }
}
