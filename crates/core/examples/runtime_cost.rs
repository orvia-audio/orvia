//! Measure release-mode DSP cost with deterministic synthetic input and no audio device.
use orvia_core::{BLOCK, Mixing, Processor, SAMPLE_RATE, Settings, calibration};
use std::{hint::black_box, time::Instant};

fn main() {
    let case = std::env::args()
        .nth(1)
        .expect("choose bypass, mono-input, stereo-input, or calibration");
    let mono = case == "mono-input";
    let mut seed = 19_u32;
    let samples: Vec<[f32; 2]> = (0..SAMPLE_RATE * 12)
        .map(|i| {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let x =
                0.13 * (i as f32 * 0.055).sin() + (seed as i32 as f32) * 0.015 / i32::MAX as f32;
            [x, if mono { x } else { x * 0.7 }]
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
        "bypass" | "mono-input" | "stereo-input"
    ));
    let settings = Settings {
        suppress_noise: case != "bypass",
        ..Settings::default()
    };
    let mut processor = Processor::for_input(&settings, if mono { 1 } else { 2 });
    let mut times = Vec::new();
    for frames in samples
        .chunks_exact(BLOCK)
        .cycle()
        .take(if case == "bypass" { 80_000 } else { 1600 })
    {
        let mut block = *<&[[f32; 2]; BLOCK]>::try_from(frames).unwrap();
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
