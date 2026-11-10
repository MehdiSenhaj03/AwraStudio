//! Pitch detection (YIN) and note helpers.

/// YIN fundamental frequency estimator.
pub struct Yin {
    diff: Vec<f32>,
    pub threshold: f32,
    pub min_freq: f32,
    pub max_freq: f32,
}

impl Yin {
    pub fn new(min_freq: f32, max_freq: f32) -> Yin {
        Yin { diff: Vec::new(), threshold: 0.15, min_freq, max_freq }
    }

    /// The window a detection needs at a rate (two periods of the lowest
    /// frequency).
    pub fn window(&self, sr: f32) -> usize {
        2 * (sr / self.min_freq).ceil() as usize
    }

    /// Detects the frequency of `frame` (at least `window` samples).
    /// Returns (Hz, clarity 0..1).
    pub fn detect(&mut self, frame: &[f32], sr: f32) -> Option<(f32, f32)> {
        let max_lag = ((sr / self.min_freq).ceil() as usize).min(frame.len() / 2);
        let min_lag = ((sr / self.max_freq).floor() as usize).max(2);
        if max_lag <= min_lag + 2 {
            return None;
        }
        let w = frame.len() - max_lag;
        // silence
        let energy: f32 = frame[..w].iter().map(|x| x * x).sum::<f32>() / w as f32;
        if energy < 1e-7 {
            return None;
        }
        self.diff.clear();
        self.diff.resize(max_lag + 1, 0.0);
        for tau in 1..=max_lag {
            let mut s = 0.0f32;
            let (a, b) = (&frame[..w], &frame[tau..tau + w]);
            for i in 0..w {
                let d = a[i] - b[i];
                s += d * d;
            }
            self.diff[tau] = s;
        }
        // cumulative mean normalized difference
        self.diff[0] = 1.0;
        let mut running = 0.0f32;
        for tau in 1..=max_lag {
            running += self.diff[tau];
            self.diff[tau] = if running > 0.0 { self.diff[tau] * tau as f32 / running } else { 1.0 };
        }
        let mut tau = min_lag;
        let mut found = None;
        while tau < max_lag {
            if self.diff[tau] < self.threshold {
                while tau + 1 < max_lag && self.diff[tau + 1] < self.diff[tau] {
                    tau += 1;
                }
                found = Some(tau);
                break;
            }
            tau += 1;
        }
        let tau = found?;
        // parabolic interpolation
        let (a, b, c) = (self.diff[tau - 1], self.diff[tau], self.diff[(tau + 1).min(max_lag)]);
        let denom = a + c - 2.0 * b;
        let shift = if denom.abs() > 1e-9 { 0.5 * (a - c) / denom } else { 0.0 };
        let period = tau as f32 + shift.clamp(-1.0, 1.0);
        let f = sr / period;
        if f < self.min_freq || f > self.max_freq {
            return None;
        }
        Some((f, (1.0 - b).clamp(0.0, 1.0)))
    }
}

pub fn freq_to_midi(f: f32) -> f32 {
    69.0 + 12.0 * (f / 440.0).log2()
}

pub fn midi_to_freq(m: f32) -> f32 {
    440.0 * 2f32.powf((m - 69.0) / 12.0)
}

/// The nearest note of a scale to a MIDI pitch.
pub fn nearest_in_scale(midi: f32, key: u8, notes: &[u8]) -> f32 {
    let base = midi.floor() as i32;
    let mut best = midi.round();
    let mut best_d = f32::MAX;
    for n in base - 12..=base + 12 {
        let pc = (n - key as i32).rem_euclid(12) as u8;
        if notes.contains(&pc) {
            let d = (n as f32 - midi).abs();
            if d < best_d {
                best_d = d;
                best = n as f32;
            }
        }
    }
    best
}

/// A pitch track of a whole signal: (time s, Hz or 0 when unvoiced).
pub fn track(signal: &[f32], sr: u32, hop: usize) -> Vec<(f32, f32)> {
    // detect at a reduced rate: voices sit below 1.2 kHz
    let dec = (sr / 16000).max(1) as usize;
    let low: Vec<f32> = signal.chunks(dec).map(|c| c.iter().sum::<f32>() / c.len() as f32).collect();
    let dsr = sr as f32 / dec as f32;
    let mut yin = Yin::new(65.0, 1100.0);
    let win = yin.window(dsr);
    let dhop = (hop / dec).max(1);
    let mut out = Vec::new();
    let mut i = 0;
    while i + win <= low.len() {
        let t = (i + win / 2) as f32 / dsr;
        let f = yin.detect(&low[i..i + win], dsr).filter(|&(_, c)| c > 0.5).map(|(f, _)| f).unwrap_or(0.0);
        out.push((t, f));
        i += dhop;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_sine_and_voice_like() {
        let sr = 48000.0;
        for f in [110.0f32, 220.0, 440.0, 660.0] {
            let x: Vec<f32> = (0..4000)
                .map(|i| {
                    let t = i as f32 / sr;
                    (2.0 * std::f32::consts::PI * f * t).sin() * 0.5 + (4.0 * std::f32::consts::PI * f * t).sin() * 0.2
                })
                .collect();
            let mut y = Yin::new(65.0, 1100.0);
            let (d, _) = y.detect(&x[..y.window(sr)], sr).unwrap();
            assert!((d - f).abs() / f < 0.01, "{f} -> {d}");
        }
    }

    #[test]
    fn scale_snapping() {
        // A minor: A B C D E F G ; C# (61) is not in it
        let notes = studio_core::effects::scale_notes(1);
        let n = nearest_in_scale(61.2, 9, notes);
        assert!(n == 60.0 || n == 62.0);
        assert_eq!(nearest_in_scale(69.3, 9, notes), 69.0);
    }
}
