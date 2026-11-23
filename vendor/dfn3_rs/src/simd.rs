//! Hot kernels: packed GEMM/GEMV and GRU/tanh activations.
//! AVX2+FMA (plus F16C for mixed precision) is selected at construction.
//! Other CPUs use the scalar path.

use crate::weights::{Data, Mat};

#[derive(Clone, Copy)]
pub(crate) struct Simd {
    avx2: bool,
}

impl Simd {
    pub fn new() -> Self {
        #[cfg(target_arch = "x86_64")]
        let avx2 = is_x86_feature_detected!("avx2")
            && is_x86_feature_detected!("fma")
            && is_x86_feature_detected!("f16c");
        #[cfg(not(target_arch = "x86_64"))]
        let avx2 = false;
        Self { avx2 }
    }

    pub fn supports_pair(self) -> bool {
        self.avx2
    }

    /// `out[r][..n] = relu?(a[r] · w + bias + (acc ? out[r] : 0))` for `r < m`.
    /// `m` must be 1, 2, or a positive multiple of 4; all `w.n` padded columns are written.
    #[allow(clippy::too_many_arguments)]
    pub fn gemm(
        self,
        m: usize,
        a: &[f32],
        lda: usize,
        w: &Mat,
        bias: Option<&[f32]>,
        acc: bool,
        relu: bool,
        out: &mut [f32],
        ldo: usize,
    ) {
        assert!(m == 1 || m == 2 || (m > 0 && m.is_multiple_of(4)));
        assert!(a.len() >= (m - 1) * lda + w.k && out.len() >= (m - 1) * ldo + w.n);
        assert!(w.w.len() == w.k * w.n && bias.is_none_or(|b| b.len() >= w.n));
        macro_rules! run {
            ($weights:expr, $expand:expr, $half:literal) => {{
                let weights = $weights;
                #[cfg(target_arch = "x86_64")]
                if self.avx2 {
                    // SAFETY: CPU support checked at construction; bounds asserted above.
                    return unsafe {
                        x86::gemm::<$half>(
                            m,
                            a,
                            lda,
                            w.k,
                            w.n,
                            weights.as_ptr().cast(),
                            bias,
                            acc,
                            relu,
                            out,
                            ldo,
                        )
                    };
                }
                // The packed weights for sixteen output columns are adjacent at each input.
                for r in 0..m {
                    for (tile, packed) in weights.chunks_exact(w.k * 16).enumerate() {
                        let mut sums = [0.0; 16];
                        for (&input, row) in
                            a[r * lda..][..w.k].iter().zip(packed.as_chunks::<16>().0)
                        {
                            for (sum, &weight) in sums.iter_mut().zip(row) {
                                *sum += input * $expand(weight);
                            }
                        }
                        let start = tile * 16;
                        for (i, mut sum) in sums.into_iter().enumerate() {
                            let n = start + i;
                            let o = &mut out[r * ldo + n];
                            sum += bias.map_or(0.0, |b| b[n]) + if acc { *o } else { 0.0 };
                            *o = if relu { sum.max(0.0) } else { sum };
                        }
                    }
                }
            }};
        }
        match w.w {
            Data::F32(weights) => run!(weights, std::convert::identity, false),
            Data::F16(weights) => run!(weights, crate::weights::expand, true),
        }
    }

    /// GRU update with reset applied after the recurrent projection.
    /// Packed pre-activations `gi = W x + b_w`, `gh = R h + b_r` use
    /// z (update), r (reset), n (candidate) order.
    pub fn gru_gates(self, gi: &[f32; 768], gh: &[f32; 768], h: &mut [f32; 256]) {
        #[cfg(target_arch = "x86_64")]
        if self.avx2 {
            // SAFETY: CPU support checked at construction; fixed sizes.
            return unsafe { x86::gru_gates(gi, gh, h) };
        }
        for j in 0..256 {
            let z = sigmoid(gi[j] + gh[j]);
            let r = sigmoid(gi[256 + j] + gh[256 + j]);
            let n = (gi[512 + j] + r * gh[512 + j]).tanh();
            h[j] = n + z * (h[j] - n);
        }
    }

    pub fn tanh(self, x: &mut [f32]) {
        #[cfg(target_arch = "x86_64")]
        if self.avx2 {
            // SAFETY: CPU support checked at construction.
            return unsafe { x86::tanh_slice(x) };
        }
        x.iter_mut().for_each(|v| *v = v.tanh());
    }
}

pub(crate) fn sigmoid(x: f32) -> f32 {
    1.0 / (1.0 + (-x).exp())
}

#[cfg(target_arch = "x86_64")]
mod x86 {
    use core::arch::x86_64::*;

    /// One register tile: `R` rows x `T` 16-column tiles (R * T * 2 <= 8 accumulators).
    #[target_feature(enable = "avx2,fma,f16c")]
    #[allow(clippy::too_many_arguments)]
    unsafe fn tile<const R: usize, const T: usize, const HALF: bool>(
        k: usize,
        a: *const f32,
        lda: usize,
        w: *const u8,
        bias: *const f32,
        acc: bool,
        relu: bool,
        out: *mut f32,
        ldo: usize,
    ) {
        unsafe {
            let mut sum = [[[_mm256_setzero_ps(); 2]; T]; R];
            for i in 0..k {
                for t in 0..T {
                    let p = w.add((t * k + i) * 16 * if HALF { 2 } else { 4 });
                    let v = if HALF {
                        [
                            _mm256_cvtph_ps(_mm_load_si128(p.cast())),
                            _mm256_cvtph_ps(_mm_load_si128(p.add(16).cast())),
                        ]
                    } else {
                        [_mm256_load_ps(p.cast()), _mm256_load_ps(p.add(32).cast())]
                    };
                    for (r, row) in sum.iter_mut().enumerate() {
                        let x = _mm256_set1_ps(*a.add(r * lda + i));
                        row[t][0] = _mm256_fmadd_ps(x, v[0], row[t][0]);
                        row[t][1] = _mm256_fmadd_ps(x, v[1], row[t][1]);
                    }
                }
            }
            for (r, row) in sum.iter().enumerate() {
                for (t, s) in row.iter().enumerate() {
                    for (h, &v) in s.iter().enumerate() {
                        let o = out.add(r * ldo + t * 16 + h * 8);
                        let mut v = v;
                        if !bias.is_null() {
                            v = _mm256_add_ps(v, _mm256_loadu_ps(bias.add(t * 16 + h * 8)));
                        }
                        if acc {
                            v = _mm256_add_ps(v, _mm256_loadu_ps(o));
                        }
                        if relu {
                            v = _mm256_max_ps(v, _mm256_setzero_ps());
                        }
                        _mm256_storeu_ps(o, v);
                    }
                }
            }
        }
    }

    #[target_feature(enable = "avx2,fma,f16c")]
    #[allow(clippy::too_many_arguments)]
    pub(super) unsafe fn gemm<const HALF: bool>(
        m: usize,
        a: &[f32],
        lda: usize,
        k: usize,
        n: usize,
        wp: *const u8,
        bias: Option<&[f32]>,
        acc: bool,
        relu: bool,
        out: &mut [f32],
        ldo: usize,
    ) {
        let tiles = n / 16;
        let bytes = if HALF { 2 } else { 4 };
        let (a, o) = (a.as_ptr(), out.as_mut_ptr());
        let bias = |t: usize| bias.map_or(std::ptr::null(), |b| b[t * 16..].as_ptr());
        unsafe {
            if m == 1 {
                let mut t = 0;
                while t + 4 <= tiles {
                    tile::<1, 4, HALF>(
                        k,
                        a,
                        0,
                        wp.add(t * k * 16 * bytes),
                        bias(t),
                        acc,
                        relu,
                        o.add(t * 16),
                        0,
                    );
                    t += 4;
                }
                while t + 2 <= tiles {
                    tile::<1, 2, HALF>(
                        k,
                        a,
                        0,
                        wp.add(t * k * 16 * bytes),
                        bias(t),
                        acc,
                        relu,
                        o.add(t * 16),
                        0,
                    );
                    t += 2;
                }
                if t < tiles {
                    tile::<1, 1, HALF>(
                        k,
                        a,
                        0,
                        wp.add(t * k * 16 * bytes),
                        bias(t),
                        acc,
                        relu,
                        o.add(t * 16),
                        0,
                    );
                }
            } else if m == 2 {
                for t in (0..tiles).step_by(2) {
                    let w = wp.add(t * k * 16 * bytes);
                    if t + 1 < tiles {
                        tile::<2, 2, HALF>(k, a, lda, w, bias(t), acc, relu, o.add(t * 16), ldo);
                    } else {
                        tile::<2, 1, HALF>(k, a, lda, w, bias(t), acc, relu, o.add(t * 16), ldo);
                    }
                }
            } else {
                for t in 0..tiles {
                    for r in (0..m).step_by(4) {
                        let (a, o) = (a.add(r * lda), o.add(r * ldo + t * 16));
                        tile::<4, 1, HALF>(
                            k,
                            a,
                            lda,
                            wp.add(t * k * 16 * bytes),
                            bias(t),
                            acc,
                            relu,
                            o,
                            ldo,
                        );
                    }
                }
            }
        }
    }

    /// Cephes-style exp, valid after clamping to the finite f32 range.
    #[target_feature(enable = "avx2,fma")]
    fn exp(x: __m256) -> __m256 {
        let x = _mm256_min_ps(
            _mm256_max_ps(x, _mm256_set1_ps(-87.0)),
            _mm256_set1_ps(88.0),
        );
        let n = _mm256_round_ps::<{ _MM_FROUND_TO_NEAREST_INT | _MM_FROUND_NO_EXC }>(
            _mm256_mul_ps(x, _mm256_set1_ps(std::f32::consts::LOG2_E)),
        );
        let f = _mm256_fnmadd_ps(n, _mm256_set1_ps(0.693_359_4), x);
        let f = _mm256_fnmadd_ps(n, _mm256_set1_ps(-2.121_944_4e-4), f);
        let mut p = _mm256_set1_ps(1.987_569_1e-4);
        for c in [
            1.398_199_9e-3,
            8.333_452e-3,
            4.166_579_6e-2,
            1.666_666_5e-1,
            0.5,
        ] {
            p = _mm256_fmadd_ps(p, f, _mm256_set1_ps(c));
        }
        let y = _mm256_fmadd_ps(
            p,
            _mm256_mul_ps(f, f),
            _mm256_add_ps(f, _mm256_set1_ps(1.0)),
        );
        let scale = _mm256_slli_epi32::<23>(_mm256_add_epi32(
            _mm256_cvtps_epi32(n),
            _mm256_set1_epi32(127),
        ));
        _mm256_mul_ps(y, _mm256_castsi256_ps(scale))
    }

    #[target_feature(enable = "avx2,fma")]
    fn sigmoid(x: __m256) -> __m256 {
        let one = _mm256_set1_ps(1.0);
        _mm256_div_ps(
            one,
            _mm256_add_ps(one, exp(_mm256_sub_ps(_mm256_setzero_ps(), x))),
        )
    }

    #[target_feature(enable = "avx2,fma")]
    fn tanh(x: __m256) -> __m256 {
        let two = _mm256_set1_ps(2.0);
        _mm256_fmsub_ps(two, sigmoid(_mm256_mul_ps(two, x)), _mm256_set1_ps(1.0))
    }

    #[target_feature(enable = "avx2,fma")]
    pub(super) unsafe fn gru_gates(gi: &[f32; 768], gh: &[f32; 768], h: &mut [f32; 256]) {
        let (gi, gh, hp) = (gi.as_ptr(), gh.as_ptr(), h.as_mut_ptr());
        for j in (0..256).step_by(8) {
            unsafe {
                let ld = |p: *const f32, o: usize| _mm256_loadu_ps(p.add(o + j));
                let z = sigmoid(_mm256_add_ps(ld(gi, 0), ld(gh, 0)));
                let r = sigmoid(_mm256_add_ps(ld(gi, 256), ld(gh, 256)));
                let n = tanh(_mm256_fmadd_ps(r, ld(gh, 512), ld(gi, 512)));
                let old = _mm256_loadu_ps(hp.add(j));
                _mm256_storeu_ps(hp.add(j), _mm256_fmadd_ps(z, _mm256_sub_ps(old, n), n));
            }
        }
    }

    #[target_feature(enable = "avx2,fma")]
    pub(super) unsafe fn tanh_slice(x: &mut [f32]) {
        let (chunks, tail) = x.as_chunks_mut::<8>();
        for c in chunks {
            unsafe { _mm256_storeu_ps(c.as_mut_ptr(), tanh(_mm256_loadu_ps(c.as_ptr()))) };
        }
        tail.iter_mut().for_each(|v| *v = v.tanh());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn portable_tiles_match_scalar_columns_with_padded_rows() {
        const K: usize = 13;
        const N: usize = 32;
        let fp32 = Box::leak(Box::new(std::array::from_fn::<_, { K * N }, _>(|i| {
            ((i % 29) as f32 - 13.) * 0.0625
        })));
        let fp16 = Box::leak(Box::new(std::array::from_fn::<_, { K * N }, _>(|i| {
            ((i * 97) % 0x7400) as u16 | if i % 5 == 0 { 0x8000 } else { 0 }
        })));
        let a = std::array::from_fn::<_, 64, _>(|i| (i as f32 * 0.13).sin());
        let bias = std::array::from_fn::<_, N, _>(|i| i as f32 * -0.05);
        for data in [Data::F32(fp32), Data::F16(fp16)] {
            let w = Mat {
                k: K,
                n: N,
                w: data,
            };
            for (rows, bias, acc, relu) in
                [(1, None, false, true), (4, Some(&bias[..]), true, false)]
            {
                let mut output = [0.25; 140];
                let mut expected = output;
                for r in 0..rows {
                    for n in 0..N {
                        let mut sum = 0.;
                        for k in 0..K {
                            let offset = (n / 16 * K + k) * 16 + n % 16;
                            let weight = match data {
                                Data::F32(s) => s[offset],
                                Data::F16(s) => crate::weights::expand(s[offset]),
                            };
                            sum += a[r * 16 + k] * weight;
                        }
                        sum += bias.map_or(0., |b| b[n]) + if acc { 0.25 } else { 0. };
                        expected[r * 35 + n] = if relu { sum.max(0.) } else { sum };
                    }
                }
                Simd { avx2: false }.gemm(rows, &a, 16, &w, bias, acc, relu, &mut output, 35);
                assert_eq!(output, expected);
            }
        }
    }
}
