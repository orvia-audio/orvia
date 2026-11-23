//! Frequency and level scales shared by the engine and mixer display.

use serde::{Deserialize, Serialize};

/// Heartbeat file refreshed while a mixer window displays the spectrum.
pub const OBSERVER_FILE: &str = "observe-spectrum";

/// The spectrum of the processed stereo mix before device rate conversion.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Spectrum {
    /// Peak level in each logarithmic band, in 1 dB steps above −80 dBFS.
    /// Each band uses the louder channel. A value of zero is the display floor.
    pub bands: Vec<u8>,
    /// Frequency of the strongest in-range spectral peak above the display floor.
    pub peak_hz: Option<u16>,
}

impl Spectrum {
    /// Number of equal-width display bands on the logarithmic frequency axis.
    pub const BANDS: usize = 72;
    /// Display range in decibels below full scale.
    pub const RANGE_DB: u8 = 80;
    /// Lowest frequency shown in Hz.
    pub const MIN_HZ: f32 = 20.;
    /// Highest frequency shown in Hz.
    pub const MAX_HZ: f32 = 20_000.;

    /// Frequency in Hz at a fractional position from left (0) to right (1).
    pub fn frequency_at(position: f32) -> f32 {
        Self::MIN_HZ * (Self::MAX_HZ / Self::MIN_HZ).powf(position.clamp(0., 1.))
    }
}
