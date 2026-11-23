//! Measure release-mode DSP cost with deterministic synthetic input and no audio device.
use orvia_core::{BLOCK, Mixing, Processor, SAMPLE_RATE, Settings, calibration};
use std::{hint::black_box, time::Instant};

fn main() {
    let case = std::env::args().nth(1).expect(
        "choose bypass, mono-input, mirrored-input, stereo-input, quiet-low-cut, quiet-compressor, or calibration",
    );
    let mono = case == "mono-input";
    let mirrored = mono || case == "mirrored-input";
    let mut seed = 19_u32;
    let samples: Vec<[f32; 2]> = (0..SAMPLE_RATE * 12)
        .map(|i| {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let x =
                0.13 * (i as f32 * 0.055).sin() + (seed as i32 as f32) * 0.015 / i32::MAX as f32;
            [x, if mirrored { x } else { x * 0.7 }]
        })
        .collect();
    if case == "calibration" {
        let noise: Vec<_> = samples[..SAMPLE_RATE as usize * 2]
            .iter()
            .map(|f| [f[0] * 0.01, f[1] * 0.01])
            .collect();
        let start = Instant::now();
        let result = calibration::recommend(&noise, [&samples, &samples], Mixing::Stereo).unwrap();
        println!(
            "{:.0} ms, {:.6?}",
            start.elapsed().as_secs_f64() * 1_000.,
            result
        );
        return;
    }
    assert!(matches!(
        case.as_str(),
        "bypass"
            | "mono-input"
            | "mirrored-input"
            | "stereo-input"
            | "quiet-low-cut"
            | "quiet-compressor"
    ));
    let suppress_noise = matches!(
        case.as_str(),
        "mono-input" | "mirrored-input" | "stereo-input"
    );
    let settings = Settings {
        suppress_noise,
        low_cut_hz: if case == "quiet-low-cut" { 80 } else { 0 },
        compressor_ratio: if case == "quiet-compressor" { 2 } else { 1 },
        ..Settings::default()
    };
    let mut processor = Processor::for_input(&settings, if mono { 1 } else { 2 });
    let mut times = Vec::new();
    let iterations = if suppress_noise { 1600 } else { 80_000 };
    for frames in samples.chunks_exact(BLOCK).cycle().take(iterations) {
        let mut block = *<&[[f32; 2]; BLOCK]>::try_from(frames).unwrap();
        if case == "quiet-low-cut" && !times.is_empty() {
            block.fill([0.; 2]);
        }
        let start = Instant::now();
        processor.process_unmetered(black_box(&mut block));
        times.push(start.elapsed().as_nanos());
        black_box(&block);
    }
    times.drain(..100);
    times.sort_unstable();
    println!(
        "median {:.1} us, p99 {:.1} us per 10 ms block",
        times[times.len() / 2] as f64 / 1000.,
        times[times.len() * 99 / 100] as f64 / 1000.
    );
}
