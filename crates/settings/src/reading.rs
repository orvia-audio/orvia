//! Local recognition of the displayed reading passage. Audio never leaves the mixer.
use super::*;
use crate::transcription::transcribe;
use std::sync::mpsc::{self, Receiver, TryRecvError};

pub(super) fn passage(l: Language) -> &'static str {
    l.text(
        "I opened the window and felt a cool breeze.",
        "窓を開けると、涼しい風が入ってきました。",
    )
}

fn japanese(l: Language) -> bool {
    l.text("en", "ja") == "ja"
}

fn advance(text: &str, ja: bool, mut progress: usize) -> usize {
    // Match ordered anchors through the final word, allowing recognition errors between them.
    let words: &[&str] = if ja {
        &["まどを", "あけると", "かぜが", "はいってきました"]
    } else {
        &["open", "window", "cool", "breeze"]
    };
    let normalized = if ja {
        text.replace("窓", "まど")
            .replace("マド", "まど")
            .replace("開ける", "あける")
            .replace("風", "かぜ")
            .replace("入って", "はいって")
            .replace("来ました", "きました")
            .chars()
            .filter(|c| c.is_alphanumeric())
            .collect::<String>()
    } else {
        let text = text.to_lowercase().replace(['’', '\''], "");
        let words = text
            .split(|c: char| !c.is_alphanumeric())
            .filter(|s| !s.is_empty())
            .map(|word| if word == "opened" { "open" } else { word })
            .collect::<Vec<_>>();
        format!(" {} ", words.join(" "))
    };
    let mut rest = normalized.as_str();
    while let Some(&piece) = words.get(progress) {
        let token = if ja {
            piece.to_owned()
        } else {
            format!(" {piece} ")
        };
        let Some(start) = rest.find(&token) else {
            break;
        };
        rest = &rest[start + token.len() - usize::from(!ja)..];
        progress += 1;
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
    /// Whether the opening clause has been recognized in this take.
    pub(super) fn heard_first_half(&self) -> bool {
        self.progress >= 2
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
        if self.progress == 4 {
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
        // Keep the previously recognized clause as context after a long pause.
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
    fn pauses_and_unrelated_words_cannot_finish_the_passage() {
        assert_eq!(advance("窓を開けると", true, 0), 2);
        assert_eq!(advance("", true, 2), 2);
        assert_eq!(advance("入ってきました", true, 2), 2);
        assert_eq!(advance("涼しい風が入って来ました。", true, 2), 4);
        assert_eq!(advance("a cool breeze", false, 0), 0);
        assert_eq!(
            advance("I opened the window and felt a cool breeze.", false, 0),
            4
        );
        assert_eq!(advance("a cool breezeway", false, 3), 3);
        assert_eq!(advance("まどを開けると数指風が入ってきました", true, 0), 4);
        assert_eq!(
            advance("I open the window and fell to cool breeze.", false, 0),
            4
        );
        assert_eq!(advance("マイクの音を確認します。", true, 0), 0);
    }
}
