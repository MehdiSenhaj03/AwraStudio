//! Biquads (RBJ cookbook), one-pole filters, envelope helpers.

use std::f64::consts::PI;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FilterType {
    LowPass,
    HighPass,
    BandPass,
    Peak,
    LowShelf,
    HighShelf,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Coefs {
    pub b0: f64,
    pub b1: f64,
    pub b2: f64,
    pub a1: f64,
    pub a2: f64,
}

impl Coefs {
    pub const IDENTITY: Coefs = Coefs { b0: 1.0, b1: 0.0, b2: 0.0, a1: 0.0, a2: 0.0 };

    pub fn new(kind: FilterType, sr: f64, freq: f64, q: f64, gain_db: f64) -> Coefs {
        let freq = freq.clamp(5.0, sr * 0.49);
        let a = 10f64.powf(gain_db / 40.0);
        let w0 = 2.0 * PI * freq / sr;
        let (sw, cw) = w0.sin_cos();
        let q = q.max(0.05);
        let alpha = sw / (2.0 * q);
        let (b0, b1, b2, a0, a1, a2) = match kind {
            FilterType::LowPass => ((1.0 - cw) / 2.0, 1.0 - cw, (1.0 - cw) / 2.0, 1.0 + alpha, -2.0 * cw, 1.0 - alpha),
            FilterType::HighPass => {
                ((1.0 + cw) / 2.0, -(1.0 + cw), (1.0 + cw) / 2.0, 1.0 + alpha, -2.0 * cw, 1.0 - alpha)
            }
            FilterType::BandPass => (alpha, 0.0, -alpha, 1.0 + alpha, -2.0 * cw, 1.0 - alpha),
            FilterType::Peak => (1.0 + alpha * a, -2.0 * cw, 1.0 - alpha * a, 1.0 + alpha / a, -2.0 * cw, 1.0 - alpha / a),
            FilterType::LowShelf => {
                let s = 2.0 * a.sqrt() * alpha;
                (
                    a * ((a + 1.0) - (a - 1.0) * cw + s),
                    2.0 * a * ((a - 1.0) - (a + 1.0) * cw),
                    a * ((a + 1.0) - (a - 1.0) * cw - s),
                    (a + 1.0) + (a - 1.0) * cw + s,
                    -2.0 * ((a - 1.0) + (a + 1.0) * cw),
                    (a + 1.0) + (a - 1.0) * cw - s,
                )
            }
            FilterType::HighShelf => {
                let s = 2.0 * a.sqrt() * alpha;
                (
                    a * ((a + 1.0) + (a - 1.0) * cw + s),
                    -2.0 * a * ((a - 1.0) + (a + 1.0) * cw),
                    a * ((a + 1.0) + (a - 1.0) * cw - s),
                    (a + 1.0) - (a - 1.0) * cw + s,
                    2.0 * ((a - 1.0) - (a + 1.0) * cw),
                    (a + 1.0) - (a - 1.0) * cw - s,
                )
            }
        };
        Coefs { b0: b0 / a0, b1: b1 / a0, b2: b2 / a0, a1: a1 / a0, a2: a2 / a0 }
    }

    /// Magnitude response (dB) at a frequency, for drawing EQ curves.
    pub fn magnitude_db(&self, sr: f64, freq: f64) -> f64 {
        let w = 2.0 * PI * freq / sr;
        let (s1, c1) = w.sin_cos();
        let (s2, c2) = (2.0 * w).sin_cos();
        let nr = self.b0 + self.b1 * c1 + self.b2 * c2;
        let ni = -(self.b1 * s1 + self.b2 * s2);
        let dr = 1.0 + self.a1 * c1 + self.a2 * c2;
        let di = -(self.a1 * s1 + self.a2 * s2);
        let m = ((nr * nr + ni * ni) / (dr * dr + di * di).max(1e-30)).sqrt();
        20.0 * m.max(1e-9).log10()
    }
}

/// Transposed direct form II biquad.
#[derive(Clone, Copy, Debug, Default)]
pub struct Biquad {
    pub c: Coefs,
    z1: f64,
    z2: f64,
}

impl Biquad {
    pub fn new(c: Coefs) -> Biquad {
        Biquad { c, z1: 0.0, z2: 0.0 }
    }
    #[inline]
    pub fn tick(&mut self, x: f64) -> f64 {
        let y = self.c.b0 * x + self.z1;
        self.z1 = self.c.b1 * x - self.c.a1 * y + self.z2;
        self.z2 = self.c.b2 * x - self.c.a2 * y;
        y
    }
    pub fn reset(&mut self) {
        self.z1 = 0.0;
        self.z2 = 0.0;
    }
    pub fn process(&mut self, buf: &mut [f32]) {
        for s in buf {
            *s = self.tick(*s as f64) as f32;
        }
    }
}

/// One-pole low-pass (smoothing, damping).
#[derive(Clone, Copy, Debug, Default)]
pub struct OnePole {
    pub a: f64,
    pub z: f64,
}

impl OnePole {
    pub fn lowpass(sr: f64, freq: f64) -> OnePole {
        OnePole { a: (-2.0 * PI * freq / sr).exp(), z: 0.0 }
    }
    pub fn set_freq(&mut self, sr: f64, freq: f64) {
        self.a = (-2.0 * PI * freq.max(1.0) / sr).exp();
    }
    #[inline]
    pub fn lp(&mut self, x: f64) -> f64 {
        self.z = x + self.a * (self.z - x);
        self.z
    }
    #[inline]
    pub fn hp(&mut self, x: f64) -> f64 {
        x - self.lp(x)
    }
}

/// Coefficient for a time constant (ms) at a rate.
pub fn time_coef(sr: f64, ms: f64) -> f64 {
    if ms <= 0.0 {
        0.0
    } else {
        (-1.0 / (ms * 0.001 * sr)).exp()
    }
}

/// A linear ramp toward a target value (zipper-free gains).
#[derive(Clone, Copy, Debug)]
pub struct Smoothed {
    pub value: f64,
    pub target: f64,
    step: f64,
    left: u32,
    len: u32,
}

impl Smoothed {
    pub fn new(v: f64, len: u32) -> Smoothed {
        Smoothed { value: v, target: v, step: 0.0, left: 0, len: len.max(1) }
    }
    pub fn set(&mut self, t: f64) {
        if (t - self.target).abs() < 1e-12 {
            return;
        }
        self.target = t;
        self.left = self.len;
        self.step = (t - self.value) / self.len as f64;
    }
    pub fn jump(&mut self, t: f64) {
        self.target = t;
        self.value = t;
        self.left = 0;
    }
    #[inline]
    pub fn next(&mut self) -> f64 {
        if self.left > 0 {
            self.left -= 1;
            self.value += self.step;
            if self.left == 0 {
                self.value = self.target;
            }
        }
        self.value
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peak_gain_at_center() {
        let c = Coefs::new(FilterType::Peak, 48000.0, 1000.0, 1.0, 6.0);
        assert!((c.magnitude_db(48000.0, 1000.0) - 6.0).abs() < 0.05);
        assert!(c.magnitude_db(48000.0, 50.0).abs() < 0.3);
    }

    #[test]
    fn highpass_cuts_lows() {
        let c = Coefs::new(FilterType::HighPass, 48000.0, 200.0, 0.707, 0.0);
        assert!(c.magnitude_db(48000.0, 20.0) < -35.0);
        assert!(c.magnitude_db(48000.0, 5000.0).abs() < 0.1);
    }
}
