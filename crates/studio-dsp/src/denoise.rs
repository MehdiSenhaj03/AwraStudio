//! Spectral noise reduction: STFT (1024, 75 % overlap), noise floor by
//! minimum statistics, a smoothed Wiener-like gain per bin.

use std::sync::Arc;

use rustfft::num_complex::Complex;
use rustfft::{Fft, FftPlanner};

use crate::effects::Processor;

const N: usize = 1024;
const HOP: usize = N / 4;

pub struct Denoise {
    fwd: Arc<dyn Fft<f32>>,
    inv: Arc<dyn Fft<f32>>,
    scratch: Vec<Complex<f32>>,
    window: Vec<f32>,
    inbuf: [Vec<f32>; 2],
    outbuf: [Vec<f32>; 2],
    spec: [Vec<Complex<f32>>; 2],
    fill: usize,
    out_pos: usize,
    smooth: Vec<f32>,
    slow: Vec<f32>,
    noise: Vec<f32>,
    gains: Vec<f32>,
    frames: usize,
    amount: f32,
    sensitivity: f32,
}

impl Denoise {
    pub fn new(_sr: f64) -> Denoise {
        let mut planner = FftPlanner::new();
        let fwd = planner.plan_fft_forward(N);
        let inv = planner.plan_fft_inverse(N);
        let scratch_len = fwd.get_inplace_scratch_len().max(inv.get_inplace_scratch_len());
        let window: Vec<f32> = (0..N)
            .map(|i| (0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / N as f32).cos()).sqrt())
            .collect();
        Denoise {
            fwd,
            inv,
            scratch: vec![Complex::default(); scratch_len],
            window,
            inbuf: [vec![0.0; N], vec![0.0; N]],
            outbuf: [vec![0.0; N * 2], vec![0.0; N * 2]],
            spec: [vec![Complex::default(); N], vec![Complex::default(); N]],
            fill: 0,
            out_pos: 0,
            smooth: vec![0.0; N / 2 + 1],
            slow: vec![0.0; N / 2 + 1],
            noise: vec![f32::MAX; N / 2 + 1],
            gains: vec![1.0; N / 2 + 1],
            frames: 0,
            amount: 0.5,
            sensitivity: 0.5,
        }
    }

    fn frame(&mut self) {
        for ch in 0..2 {
            for i in 0..N {
                self.spec[ch][i] = Complex::new(self.inbuf[ch][i] * self.window[i], 0.0);
            }
            self.fwd.process_with_scratch(&mut self.spec[ch], &mut self.scratch);
        }
        self.frames += 1;
        let over = 2.0 + 4.0 * self.sensitivity;
        let floor = 1.0 - self.amount * 0.92;
        for k in 0..=N / 2 {
            let p = (self.spec[0][k].norm_sqr() + self.spec[1][k].norm_sqr()) * 0.5;
            self.smooth[k] = if self.frames == 1 { p } else { 0.75 * self.smooth[k] + 0.25 * p };
            // minimum statistics over a heavily smoothed spectrum (low
            // variance, so its minimum is close to the noise's mean):
            // follow it down, rise slowly
            self.slow[k] = if self.frames == 1 { p } else { 0.95 * self.slow[k] + 0.05 * p };
            if self.frames > 8 {
                self.noise[k] = if self.slow[k] < self.noise[k] { self.slow[k] } else { self.noise[k] * 1.0025 };
            }
            let snr_gain = if self.smooth[k] > 1e-20 { 1.0 - over * self.noise[k] / self.smooth[k] } else { 0.0 };
            let g = snr_gain.clamp(floor, 1.0);
            // fast to open, slower to close (less "musical noise")
            self.gains[k] = if g > self.gains[k] { g } else { 0.7 * self.gains[k] + 0.3 * g };
        }
        for ch in 0..2 {
            for k in 0..N {
                let kk = if k <= N / 2 { k } else { N - k };
                self.spec[ch][k] *= self.gains[kk];
            }
            self.inv.process_with_scratch(&mut self.spec[ch], &mut self.scratch);
            // overlap-add: sqrt-Hann twice at 75 % overlap sums to 2
            let norm = 1.0 / (N as f32 * 2.0);
            let len = self.outbuf[ch].len();
            for i in 0..N {
                let o = (self.out_pos + i) % len;
                self.outbuf[ch][o] += self.spec[ch][i].re * self.window[i] * norm;
            }
        }
    }
}

impl Processor for Denoise {
    fn set_param(&mut self, id: &str, v: f64) {
        match id {
            "amount" => self.amount = v as f32,
            "sensitivity" => self.sensitivity = v as f32,
            _ => {}
        }
    }

    fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        let len = self.outbuf[0].len();
        for i in 0..l.len() {
            let pos = N - HOP + self.fill;
            self.inbuf[0][pos] = l[i];
            self.inbuf[1][pos] = r[i];
            // output lags the input by N samples
            let o = (self.out_pos + self.fill) % len;
            l[i] = self.outbuf[0][o];
            r[i] = self.outbuf[1][o];
            self.outbuf[0][o] = 0.0;
            self.outbuf[1][o] = 0.0;
            self.fill += 1;
            if self.fill == HOP {
                self.fill = 0;
                self.out_pos = (self.out_pos + HOP) % len;
                self.frame();
                for ch in 0..2 {
                    self.inbuf[ch].copy_within(HOP.., 0);
                }
            }
        }
    }

    fn reset(&mut self) {
        for ch in 0..2 {
            self.inbuf[ch].fill(0.0);
            self.outbuf[ch].fill(0.0);
        }
        self.noise.fill(f32::MAX);
        self.gains.fill(1.0);
        self.frames = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reduces_steady_noise_and_keeps_tone() {
        let sr = 48000;
        let mut seed = 1u32;
        let mut noise = || {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            (seed >> 8) as f32 / (1u32 << 24) as f32 - 0.5
        };
        let n = sr * 3;
        let mut l: Vec<f32> = (0..n)
            .map(|i| {
                let tone = if i > sr * 2 { (i as f32 * 0.06).sin() * 0.3 } else { 0.0 };
                tone + noise() * 0.02
            })
            .collect();
        let mut r = l.clone();
        let mut d = Denoise::new(sr as f64);
        d.set_param("amount", 1.0);
        for (a, b) in l.chunks_mut(300).zip(r.chunks_mut(300)) {
            d.process(a, b);
        }
        let rms = |s: &[f32]| (s.iter().map(|x| x * x).sum::<f32>() / s.len() as f32).sqrt();
        let noise_out = rms(&l[sr..sr * 2]);
        let tone_out = rms(&l[sr * 2 + 4000..]);
        assert!(noise_out < 0.02 * 0.29 * 0.5, "noise {noise_out}");
        assert!(tone_out > 0.18, "tone {tone_out}");
    }
}
