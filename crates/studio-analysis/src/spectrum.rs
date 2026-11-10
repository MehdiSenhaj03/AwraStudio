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
    let mut planner = FftPlanner::<f32>::new();
    let fft = planner.plan_fft_forward(FFT);
    let win: Vec<f32> =
        (0..FFT).map(|i| 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / FFT as f32).cos()).collect();
    let mut acc = vec![0.0f64; FFT / 2 + 1];
    let mut frames = Vec::new();
    let mut i = 0;
    while i + FFT <= x.len() {
        let rms = (x[i..i + FFT].iter().map(|s| s * s).sum::<f32>() / FFT as f32).sqrt();
        frames.push((i, rms));
        i += FFT / 2;
    }
    if frames.is_empty() {
        return vec![0.0; FFT / 2 + 1];
    }
    // ignore silence: frames within 30 dB of the loudest
    let max = frames.iter().map(|f| f.1).fold(0.0f32, f32::max);
    let gate = max * 0.0316;
    let mut buf = vec![Complex::new(0.0f32, 0.0); FFT];
    let mut n = 0;
    for (start, rms) in frames {
        if rms < gate || rms < 1e-5 {
            continue;
        }
        for k in 0..FFT {
            buf[k] = Complex::new(x[start + k] * win[k], 0.0);
        }
        fft.process(&mut buf);
        for k in 0..=FFT / 2 {
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

/// Pitch class profile (12 bins, C first) of a signal.
pub fn chroma(x: &[f32], sr: u32) -> [f32; 12] {
    let spec = average_spectrum(x, sr);
    let hz = sr as f32 / FFT as f32;
    let mut c = [0.0f32; 12];
    for (k, &p) in spec.iter().enumerate().skip(1) {
        let f = k as f32 * hz;
        if !(80.0..=5000.0).contains(&f) {
            continue;
        }
        let midi = 69.0 + 12.0 * (f / 440.0).log2();
        let pc = (midi.round() as i32).rem_euclid(12) as usize;
        // magnitude (not power) keeps strong partials from dominating
        c[pc] += p.sqrt();
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
