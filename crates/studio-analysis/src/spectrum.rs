//! Average spectra, band energies, chroma, onset envelope.

use rustfft::num_complex::Complex;
use rustfft::FftPlanner;
use serde::{Deserialize, Serialize};

pub const FFT: usize = 4096;

/// Frequency bands the engineer talks about.
pub const BANDS: [(&str, f32, f32); 8] = [
    ("sub", 20.0, 60.0),
    ("bass", 60.0, 250.0),
    ("low_mid", 250.0, 500.0),
    ("mid", 500.0, 2000.0),
    ("high_mid", 2000.0, 4000.0),
    ("presence", 4000.0, 6000.0),
    ("brilliance", 6000.0, 12000.0),
    ("air", 12000.0, 20000.0),
];

/// Average power spectrum of the louder frames (FFT bins 0..=FFT/2).
pub fn average_spectrum(x: &[f32], sr: u32) -> Vec<f32> {
    average_spectrum_n(x, sr, FFT)
}

/// The same with another FFT size (a long one resolves low notes).
pub fn average_spectrum_n(x: &[f32], sr: u32, n_fft: usize) -> Vec<f32> {
        let fft_len = n_fft;
    let mut planner = FftPlanner::<f32>::new();
    let fft = planner.plan_fft_forward(fft_len);
    let win: Vec<f32> =
        (0..fft_len).map(|i| 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / fft_len as f32).cos()).collect();
    let mut acc = vec![0.0f64; fft_len / 2 + 1];
    let mut frames = Vec::new();
    let mut i = 0;
    while i + fft_len <= x.len() {
        let rms = (x[i..i + fft_len].iter().map(|s| s * s).sum::<f32>() / fft_len as f32).sqrt();
        frames.push((i, rms));
        i += fft_len / 2;
    }
    if frames.is_empty() {
        return vec![0.0; fft_len / 2 + 1];
    }
    // ignore silence: frames within 30 dB of the loudest
    let max = frames.iter().map(|f| f.1).fold(0.0f32, f32::max);
    let gate = max * 0.0316;
    let mut buf = vec![Complex::new(0.0f32, 0.0); fft_len];
    let mut n = 0;
    for (start, rms) in frames {
        if rms < gate || rms < 1e-5 {
            continue;
        }
        for k in 0..fft_len {
            buf[k] = Complex::new(x[start + k] * win[k], 0.0);
        }
        fft.process(&mut buf);
        for k in 0..=fft_len / 2 {
            acc[k] += buf[k].norm_sqr() as f64;
        }
        n += 1;
    }
    let _ = sr;
    acc.iter().map(|v| (*v / n.max(1) as f64) as f32).collect()
}

/// Energy of each band in dB relative to the total.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct BandProfile {
    pub sub: f32,
    pub bass: f32,
    pub low_mid: f32,
    pub mid: f32,
    pub high_mid: f32,
    pub presence: f32,
    pub brilliance: f32,
    pub air: f32,
}

impl BandProfile {
    pub fn values(&self) -> [f32; 8] {
        [self.sub, self.bass, self.low_mid, self.mid, self.high_mid, self.presence, self.brilliance, self.air]
    }
    pub fn from_values(v: [f32; 8]) -> BandProfile {
        BandProfile {
            sub: v[0],
            bass: v[1],
            low_mid: v[2],
            mid: v[3],
            high_mid: v[4],
            presence: v[5],
            brilliance: v[6],
            air: v[7],
        }
    }
}

pub fn bands(spec: &[f32], sr: u32) -> BandProfile {
    let hz = sr as f32 / FFT as f32;
    let total: f32 = spec.iter().skip(1).sum::<f32>().max(1e-20);
    let mut v = [0.0f32; 8];
    for (i, (_, lo, hi)) in BANDS.iter().enumerate() {
        let a = (lo / hz).ceil() as usize;
        let b = ((hi / hz).floor() as usize).min(spec.len() - 1);
        let e: f32 = if a <= b { spec[a..=b].iter().sum() } else { 0.0 };
        v[i] = 10.0 * (e / total).max(1e-12).log10();
    }
    BandProfile::from_values(v)
}

pub fn centroid(spec: &[f32], sr: u32) -> f32 {
    let hz = sr as f32 / FFT as f32;
    let (mut num, mut den) = (0.0f64, 0.0f64);
    for (k, &p) in spec.iter().enumerate().skip(1) {
        num += k as f64 * hz as f64 * p as f64;
        den += p as f64;
    }
    if den > 0.0 {
        (num / den) as f32
    } else {
        0.0
    }
}

/// Pitch class profile (12 bins, C first) of a signal: the spectral
/// peaks between 65 Hz and 2 kHz (partials of notes, not noise or drums),
/// each weighted by how close it is to an equal-tempered pitch.
pub fn chroma(x: &[f32], sr: u32) -> [f32; 12] {
    const N: usize = 16384;
    let spec = average_spectrum_n(x, sr, N);
    let hz = sr as f32 / N as f32;
    let mag: Vec<f32> = spec.iter().map(|p| p.sqrt()).collect();
    let lo = (100.0 / hz) as usize;
    let hi = ((2000.0 / hz) as usize).min(mag.len() - 2);
    let mut sorted: Vec<f32> = mag[lo..hi].to_vec();
    sorted.sort_by(|a, b| a.total_cmp(b));
    let floor = sorted.get(sorted.len() * 3 / 4).copied().unwrap_or(0.0);
    let mut c = [0.0f32; 12];
    for k in lo.max(1)..hi {
        let m = mag[k];
        if m <= floor || m < mag[k - 1] || m < mag[k + 1] {
            continue;
        }
        // parabolic peak position
        let (a, b, g) = (mag[k - 1], m, mag[k + 1]);
        let d = (a - g) / (2.0 * (a - 2.0 * b + g)).min(-1e-12);
        let f = (k as f32 + d.clamp(-0.5, 0.5)) * hz;
        let midi = 69.0 + 12.0 * (f / 440.0).log2();
        let off = (midi - midi.round()).abs();
        let w = (1.0 - off * 2.0).max(0.0);
        // lower partials say more about the note than the upper ones
        let weight = (m - floor) * w / (1.0 + f / 1000.0);
        c[(midi.round() as i32).rem_euclid(12) as usize] += weight;
    }
    c
}

/// Spectral-flux onset envelope (hop 512) for tempo detection.
pub fn onset_envelope(x: &[f32], hop: usize) -> Vec<f32> {
    let n = 1024;
    let mut planner = FftPlanner::<f32>::new();
    let fft = planner.plan_fft_forward(n);
    let win: Vec<f32> = (0..n).map(|i| 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / n as f32).cos()).collect();
    let mut prev = vec![0.0f32; n / 2];
    let mut out = Vec::new();
    let mut buf = vec![Complex::new(0.0f32, 0.0); n];
    let mut i = 0;
    while i + n <= x.len() {
        for k in 0..n {
            buf[k] = Complex::new(x[i + k] * win[k], 0.0);
        }
        fft.process(&mut buf);
        let mut flux = 0.0;
        for k in 0..n / 2 {
            let m = (1.0 + buf[k].norm()).ln();
            flux += (m - prev[k]).max(0.0);
            prev[k] = m;
        }
        out.push(flux);
        i += hop;
    }
    out
}
