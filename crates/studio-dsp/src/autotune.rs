//! Real-time pitch correction ("autotune").
//!
//! The voice's pitch is detected with YIN at a reduced rate every ~10 ms;
//! the nearest note of the chosen key and scale gives the target; the
//! correction glides toward it at the retune speed (instant = the robotic
//! effect, slow = natural correction) and drives a two-tap delay-line
//! pitch shifter with complementary crossfades.

use studio_core::effects::scale_notes;

use crate::effects::Processor;
use crate::pitch::{freq_to_midi, nearest_in_scale, Yin};

const HOP: usize = 256; // in decimated samples
const DEC: usize = 2;

pub struct Autotune {
    sr: f64,
    key: u8,
    scale: usize,
    speed: f64,
    amount: f64,
    humanize: f64,
    mix: f32,
    // analysis
    yin: Yin,
    ring: Vec<f32>,
    ring_w: usize,
    frame: Vec<f32>,
    dec_acc: f32,
    dec_n: usize,
    since: usize,
    target_note: f32,
    /// The correction wanted now (semitones).
    want: f64,
    /// The correction applied (semitones), gliding toward `want`.
    shift: f64,
    voiced: bool,
    // shifter
    shifters: [Shifter; 2],
    /// Detected pitch (Hz) for displays; 0 unvoiced.
    pub last_freq: f32,
}

struct Shifter {
    buf: Vec<f32>,
    w: usize,
    phase: f64,
    win: f64,
}

impl Shifter {
    fn new(sr: f64) -> Shifter {
        Shifter { buf: vec![0.0; (0.1 * sr) as usize], w: 0, phase: 0.0, win: 0.03 * sr }
    }
    #[inline]
    fn read(&self, delay: f64) -> f32 {
        let n = self.buf.len();
        let pos = self.w as f64 + n as f64 - delay;
        let a = pos.floor() as usize % n;
        let b = (a + 1) % n;
        let f = (pos - pos.floor()) as f32;
        self.buf[a] * (1.0 - f) + self.buf[b] * f
    }
    #[inline]
    fn tick(&mut self, x: f32, ratio: f64) -> f32 {
        let n = self.buf.len();
        self.buf[self.w] = x;
        self.phase = (self.phase + (1.0 - ratio) / self.win).rem_euclid(1.0);
        let p2 = (self.phase + 0.5).fract();
        let d1 = 2.0 + self.phase * self.win;
        let d2 = 2.0 + p2 * self.win;
        let g1 = (std::f64::consts::PI * self.phase).sin().powi(2) as f32;
        let g2 = 1.0 - g1;
        let y = self.read(d1) * g1 + self.read(d2) * g2;
        self.w = (self.w + 1) % n;
        y
    }
}

impl Autotune {
    pub fn new(sr: f64) -> Autotune {
        let yin = Yin::new(70.0, 1000.0);
        let win = yin.window((sr / DEC as f64) as f32);
        Autotune {
            sr,
            key: 0,
            scale: 1,
            speed: 0.8,
            amount: 1.0,
            humanize: 0.2,
            mix: 1.0,
            yin,
            ring: vec![0.0; win],
            ring_w: 0,
            frame: vec![0.0; win],
            dec_acc: 0.0,
            dec_n: 0,
            since: 0,
            target_note: 0.0,
            want: 0.0,
            shift: 0.0,
            voiced: false,
            shifters: [Shifter::new(sr), Shifter::new(sr)],
            last_freq: 0.0,
        }
    }

    fn analyze(&mut self) {
        let n = self.ring.len();
        for i in 0..n {
            self.frame[i] = self.ring[(self.ring_w + i) % n];
        }
        let dsr = (self.sr / DEC as f64) as f32;
        match self.yin.detect(&self.frame, dsr).filter(|&(_, c)| c > 0.6) {
            Some((f, _)) => {
                self.last_freq = f;
                let midi = freq_to_midi(f);
                let notes = scale_notes(self.scale);
                let nearest = nearest_in_scale(midi, self.key, notes);
                // hysteresis: keep the note until clearly past the midpoint
                if !self.voiced || (midi - self.target_note).abs() > 0.65 || nearest == self.target_note {
                    self.target_note = nearest;
                }
                let dev = (self.target_note - midi) as f64;
                let mut corr = dev * self.amount;
                if dev.abs() < 0.4 {
                    corr *= 1.0 - self.humanize * 0.6;
                }
                self.want = corr.clamp(-3.0, 3.0);
                self.voiced = true;
            }
            None => {
                self.last_freq = 0.0;
                self.voiced = false;
                self.want = 0.0;
            }
        }
    }
}

impl Processor for Autotune {
    fn set_param(&mut self, id: &str, v: f64) {
        match id {
            "key" => self.key = (v.round() as i64).rem_euclid(12) as u8,
            "scale" => self.scale = v.round() as usize,
            "speed" => self.speed = v.clamp(0.0, 1.0),
            "amount" => self.amount = v.clamp(0.0, 1.0),
            "humanize" => self.humanize = v.clamp(0.0, 1.0),
            "mix" => self.mix = v as f32,
            _ => {}
        }
    }

    fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        // glide time: 1 ms (robotic) .. 250 ms (gentle)
        let glide_ms = 1.0 + (1.0 - self.speed).powi(2) * 250.0;
        let glide = crate::filters::time_coef(self.sr, if self.voiced { glide_ms } else { 40.0 });
        for i in 0..l.len() {
            let m = (l[i] + r[i]) * 0.5;
            self.dec_acc += m;
            self.dec_n += 1;
            if self.dec_n == DEC {
                let n = self.ring.len();
                self.ring[self.ring_w] = self.dec_acc / DEC as f32;
                self.ring_w = (self.ring_w + 1) % n;
                self.dec_acc = 0.0;
                self.dec_n = 0;
                self.since += 1;
                if self.since >= HOP {
                    self.since = 0;
                    self.analyze();
                }
            }
            self.shift = self.want + glide * (self.shift - self.want);
            let ratio = 2f64.powf(self.shift / 12.0);
            let wl = self.shifters[0].tick(l[i], ratio);
            let wr = self.shifters[1].tick(r[i], ratio);
            l[i] = l[i] * (1.0 - self.mix) + wl * self.mix;
            r[i] = r[i] * (1.0 - self.mix) + wr * self.mix;
        }
    }

    fn reset(&mut self) {
        self.ring.fill(0.0);
        self.shift = 0.0;
        self.want = 0.0;
        for s in &mut self.shifters {
            s.buf.fill(0.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pitch::Yin;

    #[test]
    fn pulls_an_off_note_into_tune() {
        let sr = 48000.0;
        // 450 Hz: 39 cents above A4 (440), in C major A is in the scale
        let f = 450.0f32;
        let n = 48000;
        let mut l: Vec<f32> = (0..n)
            .map(|i| {
                let t = i as f32 / sr as f32;
                (2.0 * std::f32::consts::PI * f * t).sin() * 0.4 + (4.0 * std::f32::consts::PI * f * t).sin() * 0.15
            })
            .collect();
        let mut r = l.clone();
        let mut at = Autotune::new(sr);
        at.set_param("key", 0.0);
        at.set_param("scale", 0.0);
        at.set_param("speed", 1.0);
        at.set_param("humanize", 0.0);
        for (a, b) in l.chunks_mut(256).zip(r.chunks_mut(256)) {
            at.process(a, b);
        }
        let mut yin = Yin::new(70.0, 1000.0);
        let w = yin.window(sr as f32);
        let (d, _) = yin.detect(&l[30000..30000 + w], sr as f32).unwrap();
        assert!((d - 440.0).abs() < 3.0, "corrected to {d}");
    }
}
