//! Local recognition of the spoken count during a microphone check.
use super::*;
use crate::transcription::transcribe;
use std::sync::mpsc::{self, Receiver, TryRecvError};

/// Last number in the spoken microphone check.
pub(super) const LAST_NUMBER: usize = 5;

fn japanese(l: Language) -> bool {
    l.text("en", "ja") == "ja"
}

/// Recognize whole tokens, including a group of consecutive digits or Japanese number words.
/// An overlapping transcription can repeat earlier numbers before it advances the count.
fn advance(text: &str, ja: bool, mut progress: usize) -> usize {
    let words: [&[&str]; LAST_NUMBER] = if ja {
        [
            &["いち", "イチ", "一", "１", "1"],
            &["に", "ニ", "二", "２", "2"],
            &["さん", "サン", "三", "３", "3"],
            &["よん", "ヨン", "し", "シ", "四", "４", "4"],
            &["ご", "ゴ", "五", "５", "5"],
        ]
    } else {
        [
            &["one", "1"],
            &["two", "2"],
            &["three", "3"],
            &["four", "4"],
            &["five", "5"],
        ]
    };
    for mut token in text.to_lowercase().split(|c: char| !c.is_alphanumeric()) {
        let mut first = 0;
        let mut last = 0;
        while !token.is_empty() {
            let Some((number, rest)) = words.iter().enumerate().find_map(|(index, forms)| {
                forms
                    .iter()
                    .find_map(|form| token.strip_prefix(form).map(|rest| (index + 1, rest)))
            }) else {
                break;
            };
            if first == 0 {
                first = number;
            } else if number != last + 1 {
                break;
            }
            last = number;
            token = rest;
        }
        if token.is_empty() && first <= progress + 1 {
            progress = progress.max(last);
        }
    }
    progress
}

#[derive(Default)]
pub(super) struct Reader {
    progress: usize,
    recognized: std::ops::Range<usize>,
    decoding: std::ops::Range<usize>,
    pending: Option<Receiver<Result<String, String>>>,
    checked: usize,
    last_sound: usize,
    submitted: usize,
    last_attempt: Option<Instant>,
    last_data: Option<Instant>,
    final_decode: bool,
}

impl Reader {
    /// Highest consecutively recognized number in this take, or zero before speech.
    pub(super) fn progress(&self) -> usize {
        self.progress
    }

    pub(super) fn listen(
        &mut self,
        recording: &mut orvia_engine::mic_test::Recording,
        language: Language,
        cx: &mut Context<Panel>,
    ) -> Result<bool, String> {
        let rate = recording.rate();
        let frames = recording.collect().map_err(|e| e.to_string())?;
        self.poll(frames, rate, language, cx)
    }

    /// Only recognized words advance the guide. A pause triggers a decoding attempt.
    pub(super) fn poll(
        &mut self,
        frames: &[[f32; 2]],
        rate: u32,
        language: Language,
        cx: &mut Context<Panel>,
    ) -> Result<bool, String> {
        let ja = japanese(language);
        if let Some(rx) = &self.pending {
            match rx.try_recv() {
                Ok(result) => {
                    let text = result?;
                    if !text.is_empty() {
                        let progress = advance(&text, ja, self.progress);
                        if progress > self.progress {
                            self.progress = progress;
                            self.recognized = self.decoding.clone();
                        }
                    }
                    self.pending = None;
                }
                Err(TryRecvError::Disconnected) => return Err("Speech recognition stopped".into()),
                Err(TryRecvError::Empty) => {}
            }
        }
        if self.progress == LAST_NUMBER {
            return Ok(true);
        }
        let step = (rate / 50) as usize;
        let now = Instant::now();
        if self.checked + step <= frames.len() {
            self.last_data = Some(now);
        }
        if now.duration_since(*self.last_data.get_or_insert(now)) > Duration::from_secs(4) {
            return Err(language
                .text(
                    "Microphone disconnected or stalled.",
                    "マイクから音声が届いていません。",
                )
                .into());
        }
        while self.checked + step <= frames.len() {
            let block = &frames[self.checked..self.checked + step];
            self.checked += step;
            if block.iter().map(|f| f[0] * f[0] + f[1] * f[1]).sum::<f32>() > 2e-8 * step as f32 {
                self.last_sound = self.checked;
            }
        }
        let pause = frames.len().saturating_sub(self.last_sound) >= (rate / 3) as usize;
        if self.pending.is_some()
            || (self.last_sound <= self.submitted && (!pause || self.final_decode))
            || self.last_sound < (rate / 3) as usize
        {
            return Ok(false);
        }
        let interval = if pause { 0.5 } else { 2.5 };
        if self
            .last_attempt
            .is_some_and(|t| t.elapsed().as_secs_f32() < interval)
            || (!pause && frames.len() < rate as usize * 3)
        {
            return Ok(false);
        }
        let end = frames.len().min(self.last_sound + rate as usize / 2);
        self.decoding = end.saturating_sub(rate as usize * 16)..end;
        let mut mono = Vec::new();
        // Keep the previously recognized numbers as context after a long pause.
        if !self.recognized.is_empty() && self.recognized.end <= self.decoding.start {
            let prefix = &frames[self.recognized.clone()];
            append_mono(
                &mut mono,
                &prefix[prefix.len().saturating_sub(rate as usize * 5)..],
                rate,
            );
        }
        append_mono(&mut mono, &frames[self.decoding.clone()], rate);
        self.final_decode = pause;
        self.submitted = self.last_sound;
        self.last_attempt = Some(Instant::now());
        let (tx, rx) = mpsc::channel();
        self.pending = Some(rx);
        cx.background_executor()
            .spawn(async move {
                let _ = tx.send(transcribe(mono, ja));
            })
            .detach();
        Ok(false)
    }
}

/// Average stereo source frames into the 16 kHz mono samples expected by Whisper.
fn append_mono(mono: &mut Vec<f32>, input: &[[f32; 2]], rate: u32) {
    let rate = rate as usize;
    let count = input.len() * 16_000 / rate;
    mono.reserve(count);
    for n in 0..count {
        let first = n * rate / 16_000;
        let last = ((n + 1) * rate / 16_000).max(first + 1);
        let part = &input[first..last];
        mono.push(part.iter().map(|s| (s[0] + s[1]) * 0.5).sum::<f32>() / part.len() as f32);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn both_languages_accept_spoken_and_printed_numbers() {
        for text in [
            "いち、に、さん、よん、ご",
            "いちにさんしご",
            "１二三４五",
            "1, 2, 3, 4, 5",
        ] {
            assert_eq!(advance(text, true, 0), LAST_NUMBER, "{text}");
        }
        assert_eq!(
            advance("One, TWO, three, four, FIVE.", false, 0),
            LAST_NUMBER
        );
        assert_eq!(advance("12345", false, 3), LAST_NUMBER);
    }

    #[test]
    fn pauses_repetitions_and_out_of_order_numbers_do_not_finish_early() {
        let four = advance("one two three four", false, 0);
        assert_eq!(four, 4);
        assert_eq!(advance("", false, four), four);
        assert_eq!(advance("4 3 4 4", false, four), four);
        assert_eq!(advance("one two four five", false, 0), 2);
        assert_eq!(advance("Three, four, five.", false, 2), LAST_NUMBER);
        assert_eq!(advance("五", true, 0), 0);
        assert_eq!(advance("四 五", true, 3), LAST_NUMBER);
    }

    #[test]
    fn embedded_numbers_and_unrelated_words_do_not_advance() {
        assert_eq!(advance("15 45th fifty-fivefold", false, 4), 4);
        assert_eq!(advance("ごはん 五十 15", true, 4), 4);
        assert_eq!(advance("にわ", true, 1), 1);
    }
}
