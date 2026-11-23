//! Sample-clocked interpolation; repeated targets do not restart a transition.

pub(crate) struct Ramp {
    current: f32,
    pub target: f32,
    start: f32,
    samples: usize,
    remaining: usize,
}

impl Ramp {
    pub fn new(value: f32) -> Self {
        Self {
            current: value,
            target: value,
            start: value,
            samples: 0,
            remaining: 0,
        }
    }

    pub fn reset(&mut self, value: f32) {
        *self = Self::new(value);
    }

    pub fn set_target(&mut self, target: f32, samples: usize) {
        if samples == 0 {
            self.reset(target);
        } else if self.target != target {
            self.start = self.current;
            self.target = target;
            self.samples = samples;
            self.remaining = samples;
        }
    }

    pub fn is_zero(&self) -> bool {
        self.current == 0.0 && self.target == 0.0 && self.remaining == 0
    }

    pub fn next(&mut self) -> f32 {
        if self.remaining != 0 {
            self.remaining -= 1;
            // Derive each value from the endpoints, avoiding accumulated error
            // and overshoot for long transitions or repeatedly changed targets.
            self.current = (f64::from(self.target)
                + (f64::from(self.start) - f64::from(self.target))
                    * (self.remaining as f64 / self.samples as f64))
                as f32;
        }
        self.current
    }
}
