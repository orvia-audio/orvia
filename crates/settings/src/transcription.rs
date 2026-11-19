//! Offline, unprompted transcription of short microphone readings.
use std::sync::OnceLock;
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

pub(super) fn transcribe(mut audio: Vec<f32>, ja: bool) -> Result<String, String> {
    static MODEL: OnceLock<Result<WhisperContext, String>> = OnceLock::new();
    let model = MODEL
        .get_or_init(|| {
            whisper_rs::install_logging_hooks();
            let mut parameters = WhisperContextParameters::default();
            parameters.use_gpu(false);
            WhisperContext::new_from_buffer_with_params(
                include_bytes!("../../../assets/speech/ggml-tiny-q5_1.bin"),
                parameters,
            )
            .map_err(|e| e.to_string())
        })
        .as_ref()
        .map_err(Clone::clone)?;
    let peak = audio.iter().map(|s| s.abs()).fold(0., f32::max);
    if peak < 0.0001 {
        return Ok(String::new());
    }
    // Long gaps between clauses should not consume the recognizer's context.
    // Work on the recognition copy so the preview retains the speaker's timing.
    let quiet = (peak * 0.015).max(0.0001);
    let mut quiet_samples = 0;
    audio.retain(|s| {
        quiet_samples = if s.abs() <= quiet {
            quiet_samples + 1
        } else {
            0
        };
        quiet_samples < 4_000
    });
    for sample in &mut audio {
        *sample *= 0.8 / peak;
    }
    // Keep a little endpoint context even when the latest buffer cuts close to a word.
    audio.resize((audio.len() + 6400).max(16_000), 0.);
    let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
    params.set_language(Some(if ja { "ja" } else { "en" }));
    params.set_translate(false);
    params.set_no_context(true);
    params.set_n_threads(
        std::thread::available_parallelism().map_or(1, |n| n.get().saturating_sub(1).clamp(1, 3))
            as i32,
    );
    params.set_print_special(false);
    params.set_print_progress(false);
    params.set_print_realtime(false);
    params.set_print_timestamps(false);
    params.set_temperature_inc(0.);
    params.set_single_segment(true);
    params.set_no_timestamps(true);
    params.set_max_tokens(48);
    // Whisper represents 20 ms per encoder position. Round up with endpoint context.
    params.set_audio_ctx(
        (audio.len().div_ceil(320) + 32)
            .next_multiple_of(32)
            .clamp(128, 1500) as i32,
    );
    let mut state = model.create_state().map_err(|e| e.to_string())?;
    state.full(params, &audio).map_err(|e| e.to_string())?;
    let mut text = String::new();
    for seg in state.as_iter().filter(|s| s.no_speech_probability() < 0.85) {
        text.push_str(&seg.to_str_lossy().map_err(|e| e.to_string())?);
    }
    Ok(text.trim().to_owned())
}
