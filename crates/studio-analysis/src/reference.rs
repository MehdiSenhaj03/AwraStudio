//! Matching a reference track: from what a reference sounds like and what
//! a vocal sounds like, the settings that bring the vocal closer
//! (tonal balance, reverb, autotune style, width, loudness).

use serde::{Deserialize, Serialize};

use crate::Analysis;

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct ReferenceMatch {
    /// EQ moves (param id → dB) for the vocal's EQ.
    pub eq: Vec<(String, f64)>,
    /// Reverb amount (0..1) and size (0..1), None to leave it.
    pub reverb_mix: Option<f64>,
    pub reverb_size: Option<f64>,
    /// Autotune speed (0..1) and amount, None: no autotune suggested.
    pub autotune_speed: Option<f64>,
    pub autotune_amount: Option<f64>,
    /// Doubler amount for a thicker vocal.
    pub doubler_mix: Option<f64>,
    /// Master loudness target (LUFS).
    pub target_lufs: f64,
    /// Master stereo width (0..2).
    pub master_width: Option<f64>,
    /// What was found, in words.
    pub notes: Vec<String>,
}

/// The reference's mix: the vocal sits in the 200 Hz - 8 kHz bands, so
/// its balance there says how bright / present the vocal is.
pub fn match_reference(vocal: &Analysis, reference: &Analysis) -> ReferenceMatch {
    let mut m = ReferenceMatch { target_lufs: reference.loudness_lufs.clamp(-16.0, -7.0) as f64, ..Default::default() };
    // tonal balance: compare the shape of the bands in the vocal's range,
    // each relative to the mids so that overall level doesn't matter
    let rel = |a: &Analysis| {
        let b = &a.bands;
        [b.low_mid - b.mid, b.high_mid - b.mid, b.presence - b.mid, b.brilliance - b.mid, b.air - b.mid]
    };
    let (v, r) = (rel(vocal), rel(reference));
    let ids = ["lowmid_gain", "presence_gain", "presence_gain", "air_gain", "air_gain"];
    let mut moves = std::collections::BTreeMap::<&str, f64>::new();
    for i in 0..5 {
        // half the difference, at most 6 dB: references are full mixes
        let d = ((r[i] - v[i]) as f64 * 0.5).clamp(-6.0, 6.0);
        *moves.entry(ids[i]).or_default() += d / if i >= 1 { 2.0 } else { 1.0 };
    }
    for (k, d) in moves {
        if d.abs() >= 0.5 {
            m.eq.push((k.to_string(), (d * 2.0).round() / 2.0));
        }
    }
    if !m.eq.is_empty() {
        m.notes.push("équilibre tonal rapproché de la référence".into());
    }

    // reverb: the reference's tails against the vocal's own
    let extra = (reference.rt60 - vocal.rt60.max(0.2)).max(0.0) as f64;
    if extra > 0.15 {
        m.reverb_size = Some((0.35 + extra * 0.25).clamp(0.3, 0.95));
        m.reverb_mix = Some((0.1 + extra * 0.12).clamp(0.08, 0.4));
        m.notes.push(format!("ambiance réverbérée (≈{:.1} s de queue)", reference.rt60));
    } else {
        m.reverb_size = Some(0.35);
        m.reverb_mix = Some(0.08);
        m.notes.push("voix assez sèche, proche".into());
    }

    // autotune: a reference whose melody sits on the semitones
    if let Some(p) = &reference.pitch {
        if p.voiced_ratio > 0.15 {
            if p.quantization > 0.6 {
                m.autotune_speed = Some(0.95 + (p.quantization as f64 - 0.6).min(0.05));
                m.autotune_amount = Some(1.0);
                m.notes.push("autotune marqué, correction rapide".into());
            } else if p.quantization > 0.4 {
                m.autotune_speed = Some(0.7);
                m.autotune_amount = Some(0.85);
                m.notes.push("correction de justesse audible mais naturelle".into());
            } else {
                m.autotune_speed = Some(0.35);
                m.autotune_amount = Some(0.6);
                m.notes.push("justesse naturelle (correction discrète)".into());
            }
        }
    }

    if reference.stereo_width > 0.35 {
        m.doubler_mix = Some(((reference.stereo_width as f64 - 0.3) * 0.8).clamp(0.15, 0.45));
        m.master_width = Some((1.0 + (reference.stereo_width as f64 - 0.35)).clamp(1.0, 1.4));
        m.notes.push("image stéréo large".into());
    }
    m.notes.push(format!("niveau final visé : {:.0} LUFS", m.target_lufs));
    m
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BandProfile, PitchStats};

    #[test]
    fn brighter_wetter_reference() {
        let vocal = Analysis {
            bands: BandProfile::from_values([-30.0, -10.0, -6.0, -5.0, -14.0, -20.0, -28.0, -40.0]),
            rt60: 0.3,
            loudness_lufs: -24.0,
            ..Default::default()
        };
        let reference = Analysis {
            bands: BandProfile::from_values([-14.0, -6.0, -9.0, -7.0, -12.0, -15.0, -18.0, -26.0]),
            rt60: 1.6,
            loudness_lufs: -8.5,
            stereo_width: 0.5,
            pitch: Some(PitchStats { voiced_ratio: 0.5, quantization: 0.8, ..Default::default() }),
            ..Default::default()
        };
        let m = match_reference(&vocal, &reference);
        assert!(m.eq.iter().any(|(k, v)| k == "air_gain" && *v > 0.0));
        assert!(m.eq.iter().any(|(k, v)| k == "lowmid_gain" && *v < 0.0));
        assert!(m.reverb_mix.unwrap() > 0.15);
        assert!(m.autotune_speed.unwrap() > 0.9);
        assert_eq!(m.target_lufs, -8.5);
    }
}
