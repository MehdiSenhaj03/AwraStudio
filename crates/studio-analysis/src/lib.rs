//! Audio analysis for the AI engineer: loudness, dynamics, tonal balance,
//! noise, sibilance, pitch and tuning, key, tempo, reverberation, width;
//! and matching a reference track's aesthetic.
//!
//! SPDX-License-Identifier: GPL-3.0-or-later

pub mod reference;
pub mod spectrum;

use serde::{Deserialize, Serialize};
use studio_dsp::loudness;
use studio_dsp::pitch;
use studio_dsp::AudioBuffer;

pub use spectrum::BandProfile;

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct PitchStats {
    /// Fraction of the active time with a clear pitch.
    pub voiced_ratio: f32,
    pub median_hz: f32,
    /// Range of the melody (semitones, 5th..95th percentile).
    pub range_semitones: f32,
    /// Mean distance to the nearest semitone (cents, 0..50).
    pub mean_offset_cents: f32,
    /// Fraction of voiced frames within 12 cents of a semitone and steady:
    /// high values mean pitch-corrected ("autotuned") singing.
    pub quantization: f32,
    /// Pitch classes sung (12, C first), normalized.
    pub pitch_classes: [f32; 12],
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Analysis {
    pub duration: f32,
    /// Fraction of time with sound.
    pub active_ratio: f32,
    pub loudness_lufs: f32,
    pub peak_db: f32,
    pub true_peak_db: f32,
    /// Peak to loudness (dB): high = spiky / uncompressed.
    pub crest_db: f32,
    pub loudness_range: f32,
    pub noise_floor_db: f32,
    pub bands: BandProfile,
    pub centroid_hz: f32,
    /// Sibilant energy (5-10 kHz bursts) relative to the voice (dB).
    pub sibilance_db: f32,
    /// Estimated reverberation time (s) of the tails.
    pub rt60: f32,
    /// Side / mid energy ratio (0 mono).
    pub stereo_width: f32,
    pub pitch: Option<PitchStats>,
    /// Estimated key (0 = C), and whether minor.
    pub key: Option<(u8, bool)>,
    pub key_confidence: f32,
    pub tempo_bpm: Option<f32>,
}

/// What a track sounds like, in words (for prompts and the UI).
pub fn describe(a: &Analysis) -> Vec<String> {
    let mut out = Vec::new();
    let b = &a.bands;
    if a.noise_floor_db > -55.0 && a.active_ratio < 0.95 {
        out.push(format!("bruit de fond audible ({:.0} dBFS)", a.noise_floor_db));
    }
    if b.low_mid - b.mid > 1.0 {
        out.push("bas-médiums chargés (son étouffé / boueux)".into());
    }
    if b.bass > b.mid + 3.0 {
        out.push("beaucoup de graves (effet de proximité ou grondements)".into());
    }
    if b.presence < -22.0 && b.brilliance < -26.0 {
        out.push("manque de présence et d'aigus (son terne)".into());
    }
    if b.high_mid > b.mid + 2.0 {
        out.push("haut-médiums agressifs".into());
    }
    if a.sibilance_db > -6.0 {
        out.push("sifflantes marquées".into());
    }
    if a.crest_db > 18.0 {
        out.push("dynamique très irrégulière (besoin de compression)".into());
    }
    if a.peak_db > -0.3 {
        out.push("crêtes au maximum (risque de saturation)".into());
    }
    if a.loudness_lufs < -30.0 {
        out.push("niveau d'enregistrement faible".into());
    }
    if a.rt60 > 1.2 {
        out.push(format!("déjà réverbéré (≈{:.1} s)", a.rt60));
    }
    if let Some(p) = &a.pitch {
        if p.voiced_ratio > 0.3 {
            if p.quantization > 0.55 {
                out.push("justesse très corrigée (autotune marqué)".into());
            } else if p.mean_offset_cents > 22.0 {
                out.push(format!("justesse approximative (écart moyen {:.0} cents)", p.mean_offset_cents));
            }
        }
    }
    out
}

fn db(x: f32) -> f32 {
    20.0 * x.max(1e-9).log10()
}

/// Analyzes a buffer. `vocal` adds pitch statistics.
pub fn analyze(buf: &AudioBuffer, vocal: bool) -> Analysis {
    let sr = buf.sample_rate;
    let mono = buf.mono();
    let (l, r) = buf.lr();
    let chans: Vec<&[f32]> = if buf.channel_count() > 1 { vec![l, r] } else { vec![l] };
    let mut a = Analysis { duration: buf.duration() as f32, ..Default::default() };
    if mono.is_empty() {
        return a;
    }
    a.loudness_lufs = loudness::integrated(&chans, sr) as f32;
    a.peak_db = db(buf.peak());
    a.true_peak_db = loudness::true_peak_db(&chans) as f32;
    a.crest_db = a.peak_db - a.loudness_lufs;
    a.loudness_range = loudness::range(&chans, sr) as f32;

    // 10 ms envelope
    let hop = (sr / 100) as usize;
    let env: Vec<f32> =
        mono.chunks(hop).map(|c| db((c.iter().map(|s| s * s).sum::<f32>() / c.len() as f32).sqrt())).collect();
    let mut sorted: Vec<f32> = env.iter().copied().filter(|v| *v > -100.0).collect();
    sorted.sort_by(|x, y| x.total_cmp(y));
    if !sorted.is_empty() {
        let p95 = sorted[(sorted.len() as f32 * 0.95) as usize % sorted.len()];
        let active = env.iter().filter(|&&v| v > p95 - 30.0).count();
        a.active_ratio = active as f32 / env.len() as f32;
        a.noise_floor_db = sorted[(sorted.len() as f32 * 0.05) as usize];
    } else {
        a.noise_floor_db = -120.0;
    }

    let spec = spectrum::average_spectrum(&mono, sr);
    a.bands = spectrum::bands(&spec, sr);
    a.centroid_hz = spectrum::centroid(&spec, sr);
    a.sibilance_db = sibilance(&mono, sr);
    a.rt60 = reverb_time(&env);
    a.stereo_width = width(l, r);

    if vocal {
        a.pitch = Some(pitch_stats(&mono, sr));
        if let Some(p) = &a.pitch {
            if p.voiced_ratio > 0.1 {
                let (k, m, c) = key_from_chroma(&p.pitch_classes);
                a.key = Some((k, m));
                a.key_confidence = c;
            }
        }
    } else {
        let ch = spectrum::chroma(&mono, sr);
        let (k, m, c) = key_from_chroma(&ch);
        a.key = Some((k, m));
        a.key_confidence = c;
        a.tempo_bpm = tempo(&mono, sr);
    }
    a
}

fn width(l: &[f32], r: &[f32]) -> f32 {
    let (mut m, mut s) = (0.0f64, 0.0f64);
    for (a, b) in l.iter().zip(r) {
        let mid = (a + b) as f64 * 0.5;
        let side = (a - b) as f64 * 0.5;
        m += mid * mid;
        s += side * side;
    }
    if m <= 0.0 {
        0.0
    } else {
        (s / m).sqrt() as f32
    }
}

/// Energy of 5-10 kHz bursts relative to the 200 Hz-5 kHz voice (dB, at
/// the loudest 5 % of sibilant moments).
fn sibilance(x: &[f32], sr: u32) -> f32 {
    use studio_dsp::filters::{Biquad, Coefs, FilterType};
    let mut hp = Biquad::new(Coefs::new(FilterType::HighPass, sr as f64, 5000.0, 0.7, 0.0));
    let mut lp = Biquad::new(Coefs::new(FilterType::LowPass, sr as f64, 10000.0, 0.7, 0.0));
    let mut band = Biquad::new(Coefs::new(FilterType::BandPass, sr as f64, 1000.0, 0.3, 0.0));
    let hop = (sr / 100) as usize;
    let mut ratios = Vec::new();
    for c in x.chunks(hop) {
        let (mut es, mut ev) = (0.0f64, 0.0f64);
        for &s in c {
            let v = s as f64;
            let h = lp.tick(hp.tick(v));
            let b = band.tick(v);
            es += h * h;
            ev += b * b;
        }
        if ev + es > 1e-7 * c.len() as f64 {
            ratios.push(10.0 * (es / ev.max(1e-12)).log10() as f32);
        }
    }
    if ratios.is_empty() {
        return -60.0;
    }
    ratios.sort_by(|a, b| a.total_cmp(b));
    ratios[((ratios.len() as f32 * 0.95) as usize).min(ratios.len() - 1)]
}

/// RT60 estimate from the decays after the louder moments.
fn reverb_time(env: &[f32]) -> f32 {
    if env.len() < 60 {
        return 0.0;
    }
    let mut sorted: Vec<f32> = env.iter().copied().filter(|v| *v > -100.0).collect();
    if sorted.is_empty() {
        return 0.0;
    }
    sorted.sort_by(|a, b| a.total_cmp(b));
    let median = sorted[sorted.len() / 2];
    let mut slopes = Vec::new();
    let mut i = 5;
    while i + 35 < env.len() {
        let v = env[i];
        let is_peak = v > median && (i - 5..i + 5).all(|j| env[j] <= v);
        // a peak followed by a decay (the next 300 ms fall overall)
        if is_peak && env[i + 30] < v - 10.0 {
            // regression over the decay from -5 dB to 300 ms
            let start = (i..i + 30).find(|&j| env[j] < v - 5.0).unwrap_or(i);
            let pts: Vec<(f32, f32)> =
                (start..i + 30).map(|j| ((j - start) as f32 * 0.01, env[j])).filter(|p| p.1 > -90.0).collect();
            if pts.len() > 5 {
                let n = pts.len() as f32;
                let mx = pts.iter().map(|p| p.0).sum::<f32>() / n;
                let my = pts.iter().map(|p| p.1).sum::<f32>() / n;
                let num: f32 = pts.iter().map(|p| (p.0 - mx) * (p.1 - my)).sum();
                let den: f32 = pts.iter().map(|p| (p.0 - mx).powi(2)).sum();
                if den > 0.0 {
                    let slope = num / den; // dB/s
                    if slope < -5.0 {
                        slopes.push(slope);
                    }
                }
            }
            i += 30;
        } else {
            i += 1;
        }
    }
    if slopes.is_empty() {
        return 0.0;
    }
    slopes.sort_by(|a, b| a.total_cmp(b));
    let s = slopes[slopes.len() / 2];
    (-60.0 / s).clamp(0.05, 8.0)
}

pub fn pitch_stats(x: &[f32], sr: u32) -> PitchStats {
    let tr = pitch::track(x, sr, (sr / 100) as usize);
    let voiced: Vec<f32> = tr.iter().map(|p| p.1).filter(|f| *f > 0.0).collect();
    let mut st = PitchStats::default();
    if tr.is_empty() {
        return st;
    }
    // the share of frames that have sound at all
    let active = tr.len().max(1);
    st.voiced_ratio = voiced.len() as f32 / active as f32;
    if voiced.is_empty() {
        return st;
    }
    let mut midis: Vec<f32> = voiced.iter().map(|f| pitch::freq_to_midi(*f)).collect();
    let mut offs = 0.0;
    let mut steady_in_tune = 0usize;
    let mut steady = 0usize;
    for (i, m) in midis.iter().enumerate() {
        let off = (m - m.round()).abs() * 100.0;
        offs += off;
        st.pitch_classes[(m.round() as i32).rem_euclid(12) as usize] += 1.0;
        if i > 0 && (midis[i] - midis[i - 1]).abs() < 0.3 {
            steady += 1;
            if off < 12.0 {
                steady_in_tune += 1;
            }
        }
    }
    st.mean_offset_cents = offs / midis.len() as f32;
    st.quantization = if steady > 0 { steady_in_tune as f32 / steady as f32 } else { 0.0 };
    let total: f32 = st.pitch_classes.iter().sum();
    for v in &mut st.pitch_classes {
        *v /= total.max(1.0);
    }
    midis.sort_by(|a, b| a.total_cmp(b));
    st.median_hz = pitch::midi_to_freq(midis[midis.len() / 2]);
    let lo = midis[(midis.len() as f32 * 0.05) as usize];
    let hi = midis[((midis.len() as f32 * 0.95) as usize).min(midis.len() - 1)];
    st.range_semitones = hi - lo;
    st
}

const MAJOR: [f32; 12] = [6.35, 2.23, 3.48, 2.33, 4.38, 4.09, 2.52, 5.19, 2.39, 3.66, 2.29, 2.88];
const MINOR: [f32; 12] = [6.33, 2.68, 3.52, 5.38, 2.60, 3.53, 2.54, 4.75, 3.98, 2.69, 3.34, 3.17];

fn correlation(a: &[f32; 12], b: &[f32; 12]) -> f32 {
    let ma = a.iter().sum::<f32>() / 12.0;
    let mb = b.iter().sum::<f32>() / 12.0;
    let (mut n, mut da, mut db) = (0.0, 0.0, 0.0);
    for i in 0..12 {
        n += (a[i] - ma) * (b[i] - mb);
        da += (a[i] - ma).powi(2);
        db += (b[i] - mb).powi(2);
    }
    if da * db <= 0.0 {
        0.0
    } else {
        n / (da * db).sqrt()
    }
}

/// Krumhansl-Schmuckler key estimate: (root, minor, confidence).
pub fn key_from_chroma(chroma: &[f32; 12]) -> (u8, bool, f32) {
    let mut best = (0u8, false, f32::MIN);
    for root in 0..12 {
        let mut rot = [0.0f32; 12];
        for i in 0..12 {
            rot[i] = chroma[(i + root) % 12];
        }
        for (minor, prof) in [(false, &MAJOR), (true, &MINOR)] {
            let c = correlation(&rot, prof);
            if c > best.2 {
                best = (root as u8, minor, c);
            }
        }
    }
    (best.0, best.1, best.2.max(0.0))
}

/// Tempo (60..180 BPM) from the onset envelope's autocorrelation.
pub fn tempo(x: &[f32], sr: u32) -> Option<f32> {
    let hop = 512;
    let env = spectrum::onset_envelope(x, hop);
    if env.len() < 200 {
        return None;
    }
    let mean = env.iter().sum::<f32>() / env.len() as f32;
    let e: Vec<f32> = env.iter().map(|v| (v - mean).max(0.0)).collect();
    let fps = sr as f32 / hop as f32;
    let lag_for = |bpm: f32| (60.0 / bpm * fps).round() as usize;
    let (lo, hi) = (lag_for(180.0), lag_for(60.0));
    let ac = |lag: usize| -> f32 { e.iter().zip(e.iter().skip(lag)).map(|(a, b)| a * b).sum::<f32>() / (e.len() - lag) as f32 };
    let mut best = (0usize, 0.0f32);
    for lag in lo..=hi.min(e.len() / 2) {
        // reward lags whose double also correlates (the beat, not a half)
        let s = ac(lag) + 0.5 * if lag * 2 < e.len() / 2 { ac(lag * 2) } else { 0.0 };
        if s > best.1 {
            best = (lag, s);
        }
    }
    if best.0 == 0 {
        return None;
    }
    let mut bpm = 60.0 * fps / best.0 as f32;
    // the usual range for hip-hop / pop: prefer 70..160
    while bpm < 70.0 {
        bpm *= 2.0;
    }
    while bpm > 160.0 {
        bpm /= 2.0;
    }
    Some((bpm * 10.0).round() / 10.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sr() -> u32 {
        48000
    }

    #[test]
    fn analyzes_a_clean_tone() {
        let x: Vec<f32> =
            (0..sr() * 3).map(|i| (2.0 * std::f32::consts::PI * 220.0 * i as f32 / sr() as f32).sin() * 0.3).collect();
        let a = analyze(&AudioBuffer::from_mono(sr(), x), true);
        assert!(a.peak_db < -10.0 && a.peak_db > -11.0);
        let p = a.pitch.unwrap();
        assert!((p.median_hz - 220.0).abs() < 2.0, "{}", p.median_hz);
        assert!(p.quantization > 0.8);
        assert!(a.bands.bass > a.bands.mid);
    }

    #[test]
    fn finds_tempo_of_clicks() {
        let bpm = 96.0;
        let n = sr() as usize * 12;
        let period = (60.0 / bpm * sr() as f32) as usize;
        let mut x = vec![0.0f32; n];
        let mut seed = 7u32;
        for start in (0..n).step_by(period) {
            for k in 0..800.min(n - start) {
                seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                let noise = (seed >> 8) as f32 / (1u32 << 24) as f32 - 0.5;
                x[start + k] = noise * (1.0 - k as f32 / 800.0);
            }
        }
        let t = tempo(&x, sr()).unwrap();
        assert!((t - bpm).abs() < 2.0, "{t}");
    }

    #[test]
    fn key_of_a_minor_triad() {
        let mut c = [0.0f32; 12];
        c[9] = 1.0; // A
        c[0] = 0.8; // C
        c[4] = 0.9; // E
        c[2] = 0.3;
        c[7] = 0.3;
        let (k, minor, _) = key_from_chroma(&c);
        assert_eq!((k, minor), (9, true));
    }

    #[test]
    fn reverb_time_grows_with_reverb() {
        use studio_dsp::effects::{Processor, Reverb};
        let s = sr() as usize;
        let mut dry = vec![0.0f32; s * 6];
        for b in 0..6 {
            for k in 0..s / 4 {
                dry[b * s + k] = (k as f32 * 0.07).sin() * 0.5;
            }
        }
        let mut wet_l = dry.clone();
        let mut wet_r = dry.clone();
        let mut rv = Reverb::new(sr() as f64);
        rv.set_param("size", 0.95);
        rv.set_param("mix", 0.9);
        rv.process(&mut wet_l, &mut wet_r);
        let a = analyze(&AudioBuffer::from_mono(sr(), dry), false);
        let b = analyze(&AudioBuffer::from_mono(sr(), wet_l), false);
        assert!(b.rt60 > a.rt60 * 1.5, "dry {} wet {}", a.rt60, b.rt60);
    }
}
