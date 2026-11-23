# dfn3_rs

Orvia vendors [upstream 0.1.2](https://github.com/TanixLu/dfn3_rs/tree/d65d0d12227dc8076ee9ac33b4ff52b1c2e7f1bb). Orvia processes two active channels together on AVX2 CPUs to share model weight reads. It also optimizes the portable matrix kernel, reuses identical channel histories, shares scratch buffers between non-overlapping encoder and decoder stages, and mixes delayed dry audio in blocks. The five-tap deep filter keeps the original low-frequency bins while the full target spectrum is reused for synthesis. The embedded model checksum is recorded in NOTICE.

A standalone Rust library for streaming speech enhancement, derived from
[DeepFilterNet3](https://github.com/Rikorose/DeepFilterNet) and optimized for
48 kHz mono audio. The embedded model uses FP32 encoder matrices and FP16 decoder
matrices with FP32 computation, recurrent state, normalization, biases, and DSP.

- 480 samples per frame (10 ms); algorithmic delay is 1,440 samples (30 ms).
- Shared, 64-byte-aligned weights: 5,260,224 bytes.
- AVX2/FMA/F16C kernels on supported x86-64 CPUs, with a scalar fallback.
- Construction allocates. Processing, parameter updates, and reset perform no
  heap allocations, locks, or file I/O.

```rust
use dfn3_rs::{Denoiser, FRAME_SIZE};

let mut denoiser = Denoiser::new(); // Outside the audio callback.
let input = [0.0_f32; FRAME_SIZE];  // Finite, normalized 48 kHz mono PCM.
let mut output = [0.0_f32; FRAME_SIZE];

denoiser.set_attenuation_limit(12.0);
denoiser.process(&input, &mut output);

// Adjust strength between frames using the same model instance.
denoiser.set_attenuation_limit_smooth(24.0, std::time::Duration::from_millis(20));
denoiser.process(&input, &mut output);
```

The default `embedded-model` feature includes the weights. Disable default
features to build inference code without them. Use `Model::from_bytes` with
process-lifetime storage containing exactly `WEIGHTS_SIZE` bytes, aligned to
64 bytes, then pass the returned model to `Denoiser::with_model`. Validation
checks size and alignment. The caller supplies weights in this version's packed
tensor layout. Model handles are cheap, copyable views. Construction does not
copy the weight bytes. `Denoiser::new`, `Default`, and `Model::embedded` are
available only with `embedded-model` enabled.

On Windows, the embedded weights are stored in the read-only `.dfn3` PE section.

`set_attenuation_limit(db)` is the runtime `atten-lim-db` control. Zero gives
delayed dry audio; larger magnitudes give stronger suppression; 100 dB or more
disables the limit (the default). Pass finite values and call the setter between
frames on the processing thread. Updates take effect on the next processing
call. `set_attenuation_limit_smooth(db, duration)` interpolates the linear mixture
coefficient once per output sample, not once per 10 ms processing block. Zero
duration sets immediately; repeated targets do not restart a transition. New
targets continue from the current coefficient. Both setters require finite dB.

The library mixes full enhancement with a sample-aligned delayed copy of its
input, using one IFFT. At fixed strength this is mathematically equivalent to
the original frequency-domain attenuation blend, up to FFT rounding. At zero dB
the output is exactly delayed input, including very quiet audio.

Stable zero dB skips FFT and neural processing. Exact digital silence also sleeps
the model after six processed zero frames drain its analysis, deep-filter taps
and synthesis overlap. Low-power nonzero audio is never treated as digital
silence. The dry delay line and attenuation envelope advance on every call,
including while asleep, so the algorithmic delay never changes.

When nonzero input and enabled attenuation resume, model state is rebuilt from
new input over 30 ms, with delayed dry audio returned during warm-up. Enhancement
then fades in over 20 ms. Recurrent history is intentionally discarded on sleep;
resumed enhancement is not identical to uninterrupted inference. No extra audio
latency is introduced. The reported local SNR is -15 dB while inference is skipped.

`process_frame(&mut frame)` provides in-place processing. Both processing APIs
return the local SNR estimate. `reset()` starts an unrelated stream, retains
thresholds and snaps attenuation to its configured target. `set_thresholds(min, max_erb, max_df)` controls stage skipping;
defaults are -10, 30, and 20 dB.

Audio I/O, resampling, channel routing, gain, limiting, and input buffering belong
to the caller. Use one instance per independently enhanced channel. Output is
not clipped. Zero-pad a partial final frame and process three more zero frames
to emit the delayed tail. For offline delay compensation, discard the first
`ALGORITHM_DELAY` samples and retain the original input length.

Build and check with `cargo build --release`, `cargo clippy --all-targets -- -D warnings`,
and `cargo test --release`. `realfft` is the only direct dependency; FFT plans and
scratch buffers are allocated during construction. Concurrent instances share immutable
FFT plans and the window. Each channel retains its own scratch, overlap, and five
complex deep-filter coefficients per frequency row. Construct, warm up, and reset
the denoiser before starting real-time processing. To warm all neural stages,
temporarily set thresholds to -100, 100, 100, process a few low-level nonzero
frames, reset, and restore the intended thresholds.

MIT OR Apache-2.0. See [NOTICE](NOTICE), [LICENSE-MIT](LICENSE-MIT), and
[LICENSE-APACHE](LICENSE-APACHE) for model provenance and attribution.
