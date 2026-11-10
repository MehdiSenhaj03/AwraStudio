//! The sound engineer's know-how: vocal chains by style, mix balance,
//! mastering, all derived from analyses.

use studio_analysis::Analysis;
use studio_core::effects::EffectKind;
use studio_core::model::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VocalStyle {
    /// Clean, natural, present.
    Clean,
    /// Light pitch correction, transparent.
    Natural,
    /// Hard, audible autotune.
    Autotune,
    /// Melodic: autotune, delay and reverb.
    Melodic,
    /// In your face: saturation, more compression, dry.
    Aggressive,
    /// Spacious, wet.
    Ambient,
}

impl VocalStyle {
    pub fn parse(s: &str) -> VocalStyle {
        let s = s.to_lowercase();
        if s.contains("melod") || s.contains("mélod") {
            VocalStyle::Melodic
        } else if s.contains("auto") || s.contains("robot") {
            VocalStyle::Autotune
        } else if s.contains("agress") || s.contains("aggress") || s.contains("drill") || s.contains("hard") {
            VocalStyle::Aggressive
        } else if s.contains("ambi") || s.contains("planant") || s.contains("spac") || s.contains("aérien") {
            VocalStyle::Ambient
        } else if s.contains("nat") {
            VocalStyle::Natural
        } else {
            VocalStyle::Clean
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            VocalStyle::Clean => "Propre",
            VocalStyle::Natural => "Naturelle",
            VocalStyle::Autotune => "Autotune marqué",
            VocalStyle::Melodic => "Mélodique",
            VocalStyle::Aggressive => "Agressive",
            VocalStyle::Ambient => "Aérienne",
        }
    }
    pub fn key(self) -> &'static str {
        match self {
            VocalStyle::Clean => "clean",
            VocalStyle::Natural => "natural",
            VocalStyle::Autotune => "autotune",
            VocalStyle::Melodic => "melodic",
            VocalStyle::Aggressive => "aggressive",
            VocalStyle::Ambient => "ambient",
        }
    }
}

fn slot(kind: EffectKind) -> EffectSlot {
    EffectSlot::new(kind).by(Actor::Ai)
}

/// A complete vocal chain for a track, from its (raw) analysis.
pub fn vocal_chain(
    a: Option<&Analysis>,
    style: VocalStyle,
    kind: TrackKind,
    key: Option<u8>,
    minor: bool,
) -> Vec<EffectSlot> {
    let adlib = kind == TrackKind::Adlib;
    let mut chain = Vec::new();
    let noise = a.map(|a| a.noise_floor_db).unwrap_or(-80.0);
    let active = a.map(|a| a.active_ratio).unwrap_or(0.8);

    if noise > -62.0 {
        let amount = ((noise + 62.0) / 25.0).clamp(0.25, 0.85) as f64;
        chain.push(slot(EffectKind::Denoise).with("amount", amount));
    }
    if noise > -66.0 && active < 0.92 {
        chain.push(
            slot(EffectKind::Gate)
                .with("threshold_db", (noise as f64 + 9.0).clamp(-75.0, -30.0))
                .with("range_db", 18.0),
        );
    }

    // tonal balance
    let mut eq = slot(EffectKind::Eq).with("hp_freq", if adlib { 160.0 } else { 90.0 });
    if let Some(a) = a {
        let b = &a.bands;
        let mud = b.low_mid - b.mid;
        if mud > -1.0 {
            eq = eq.with("lowmid_gain", -((mud + 3.0) as f64).clamp(1.5, 6.0)).with("lowmid_freq", 320.0);
        }
        if b.bass > b.mid + 2.0 {
            eq = eq.with("low_gain", -2.5);
        }
        if b.high_mid > b.mid + 2.0 {
            eq = eq.with("mid_gain", -2.0).with("mid_freq", 3200.0).with("mid_q", 1.4);
        }
        let dull = b.presence < -22.0 || a.centroid_hz < 1500.0;
        eq = eq.with("presence_gain", if dull { 3.5 } else { 1.5 });
        eq = eq.with("air_gain", if dull { 3.5 } else { 2.0 });
    } else {
        eq = eq.with("lowmid_gain", -2.0).with("presence_gain", 2.0).with("air_gain", 2.0);
    }
    if style == VocalStyle::Aggressive {
        let pg = eq.param("presence_gain");
        eq = eq.with("presence_gain", pg + 1.0);
    }
    chain.push(eq);

    // dynamics
    let crest = a.map(|a| a.crest_db).unwrap_or(14.0) as f64;
    let lufs = a.map(|a| a.loudness_lufs).unwrap_or(-20.0) as f64;
    let ratio = match style {
        VocalStyle::Aggressive => 5.0,
        VocalStyle::Natural | VocalStyle::Ambient => 2.5,
        _ => 3.5,
    } + if crest > 18.0 { 1.0 } else { 0.0 };
    let threshold = (lufs + 2.0).clamp(-40.0, -8.0);
    let makeup = (((-threshold - 6.0) * (1.0 - 1.0 / ratio)) * 0.45).clamp(0.0, 12.0);
    chain.push(
        slot(EffectKind::Compressor)
            .with("threshold_db", threshold)
            .with("ratio", ratio)
            .with("attack_ms", if style == VocalStyle::Aggressive { 3.0 } else { 6.0 })
            .with("release_ms", 110.0)
            .with("makeup_db", makeup),
    );

    let sib = a.map(|a| a.sibilance_db).unwrap_or(-8.0);
    if sib > -14.0 || matches!(style, VocalStyle::Aggressive | VocalStyle::Clean) {
        chain.push(slot(EffectKind::DeEsser).with("threshold_db", -32.0).with("amount_db", if sib > -6.0 { 10.0 } else { 6.0 }));
    }

    // pitch
    let (speed, amount) = match style {
        VocalStyle::Autotune => (1.0, 1.0),
        VocalStyle::Melodic => (0.88, 1.0),
        VocalStyle::Natural => (0.35, 0.7),
        VocalStyle::Ambient => (0.7, 0.9),
        _ => (0.0, 0.0),
    };
    if amount > 0.0 {
        let k = key.unwrap_or(0);
        let scale = if key.is_none() { 2.0 } else if minor { 1.0 } else { 0.0 };
        chain.push(
            slot(EffectKind::Autotune)
                .with("key", k as f64)
                .with("scale", scale)
                .with("speed", speed)
                .with("amount", amount)
                .with("humanize", if style == VocalStyle::Natural { 0.5 } else { 0.1 }),
        );
    }

    if style == VocalStyle::Aggressive {
        chain.push(slot(EffectKind::Saturation).with("drive_db", 9.0).with("mix", 0.35).with("tone", 0.2));
    }
    if adlib || matches!(style, VocalStyle::Melodic | VocalStyle::Ambient) {
        chain.push(slot(EffectKind::Doubler).with("mix", if adlib { 0.3 } else { 0.18 }));
    }
    match style {
        VocalStyle::Melodic | VocalStyle::Ambient => {
            chain.push(slot(EffectKind::Delay).with("sync", 4.0).with("mix", 0.12).with("feedback", 0.32));
        }
        _ if adlib => {
            chain.push(slot(EffectKind::Delay).with("sync", 2.0).with("mix", 0.14).with("feedback", 0.25));
        }
        _ => {}
    }
    let (size, mix) = match style {
        VocalStyle::Clean | VocalStyle::Natural => (0.35, 0.1),
        VocalStyle::Autotune => (0.45, 0.13),
        VocalStyle::Melodic => (0.62, 0.2),
        VocalStyle::Aggressive => (0.3, 0.06),
        VocalStyle::Ambient => (0.85, 0.3),
    };
    chain.push(slot(EffectKind::Reverb).with("size", size).with("mix", if adlib { mix + 0.06 } else { mix }));
    chain
}

/// A light chain that makes room for the vocals in a beat.
pub fn instrumental_chain() -> Vec<EffectSlot> {
    vec![slot(EffectKind::Eq).with("mid_gain", -1.5).with("mid_freq", 2600.0).with("mid_q", 0.9).with("hp_freq", 25.0)]
}

/// Mastering presets: target loudness and character.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MasterStyle {
    pub target_lufs: f64,
    pub glue_ratio: f64,
    pub width: f64,
    pub air: f64,
    pub low: f64,
}

pub fn master_style(name: &str) -> MasterStyle {
    let n = name.to_lowercase();
    if n.contains("loud") || n.contains("fort") || n.contains("club") || n.contains("puissant") {
        MasterStyle { target_lufs: -9.0, glue_ratio: 2.5, width: 1.12, air: 1.5, low: 1.0 }
    } else if n.contains("dyna") || n.contains("doux") || n.contains("acoust") {
        MasterStyle { target_lufs: -16.0, glue_ratio: 1.5, width: 1.0, air: 1.0, low: 0.0 }
    } else if n.contains("chaud") || n.contains("warm") {
        MasterStyle { target_lufs: -12.0, glue_ratio: 2.0, width: 1.05, air: 0.0, low: 1.5 }
    } else {
        // streaming platforms normalize around -14 LUFS
        MasterStyle { target_lufs: -12.0, glue_ratio: 2.0, width: 1.08, air: 1.0, low: 0.5 }
    }
}

/// The mastering chain, with the limiter's input gain to find.
pub fn master_chain(style: MasterStyle, limiter_gain: f64) -> Vec<EffectSlot> {
    vec![
        slot(EffectKind::Eq).with("hp_freq", 28.0).with("low_gain", style.low).with("low_freq", 90.0).with("air_gain", style.air),
        slot(EffectKind::Compressor)
            .with("threshold_db", -16.0)
            .with("ratio", style.glue_ratio)
            .with("attack_ms", 30.0)
            .with("release_ms", 180.0)
            .with("makeup_db", 0.0)
            .with("knee_db", 8.0),
        slot(EffectKind::StereoWidth).with("width", style.width).with("bass_mono_freq", 120.0),
        slot(EffectKind::Limiter).with("gain_db", limiter_gain).with("ceiling_db", -1.0).with("release_ms", 60.0),
    ]
}

/// Fader moves for a balanced mix from each track's processed loudness
/// (LUFS at 0 dB). Returns (track, volume dB, pan).
pub fn balance(project: &Project, loudness: &[(Id, f64)]) -> Vec<(Id, f64, f64)> {
    let get = |id: Id| loudness.iter().find(|(i, _)| *i == id).map(|x| x.1);
    // the beat sits at -20 LUFS, leaving room for the vocals and the master
    let beat_target = -20.0;
    let lead_target = beat_target + 1.5;
    let mut out = Vec::new();
    let leads: Vec<&Track> = project.tracks.iter().filter(|t| t.kind == TrackKind::Vocal).collect();
    let mut adlib_side = 1.0;
    for t in &project.tracks {
        let Some(l) = get(t.id) else { continue };
        if l < -65.0 {
            continue;
        }
        let (target, pan) = match t.kind {
            TrackKind::Instrumental => (beat_target, 0.0),
            TrackKind::Vocal => {
                let pan = if leads.len() == 2 {
                    if leads[0].id == t.id {
                        -0.12
                    } else {
                        0.12
                    }
                } else {
                    0.0
                };
                (lead_target - if leads.len() > 2 { 1.5 } else { 0.0 }, pan)
            }
            TrackKind::Adlib => {
                adlib_side = -adlib_side;
                (lead_target - 6.0, 0.35 * adlib_side)
            }
            TrackKind::Audio => (lead_target - 4.0, t.pan),
        };
        out.push((t.id, (target - l).clamp(-24.0, 12.0), pan));
    }
    out
}

/// What to say about an analysis for a beginner, in a sentence.
pub fn summary(name: &str, a: &Analysis) -> String {
    let d = studio_analysis::describe(a);
    if d.is_empty() {
        format!("« {name} » sonne plutôt bien ({:.0} LUFS).", a.loudness_lufs)
    } else {
        format!("« {name} » : {}.", d.join(", "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use studio_analysis::BandProfile;

    #[test]
    fn muddy_noisy_vocal_gets_cleaned() {
        let a = Analysis {
            noise_floor_db: -48.0,
            active_ratio: 0.6,
            loudness_lufs: -24.0,
            crest_db: 20.0,
            sibilance_db: -4.0,
            bands: BandProfile::from_values([-30.0, -8.0, -5.0, -6.0, -14.0, -25.0, -30.0, -45.0]),
            centroid_hz: 1200.0,
            ..Default::default()
        };
        let c = vocal_chain(Some(&a), VocalStyle::Clean, TrackKind::Vocal, Some(9), true);
        let kinds: Vec<_> = c.iter().map(|s| s.kind.key()).collect();
        assert!(kinds.contains(&"denoise") && kinds.contains(&"gate") && kinds.contains(&"deesser"));
        let eq = c.iter().find(|s| s.kind == EffectKind::Eq).unwrap();
        assert!(eq.param("lowmid_gain") < 0.0);
        assert!(eq.param("presence_gain") >= 3.0);
        assert!(!kinds.contains(&"autotune"));
        let m = vocal_chain(Some(&a), VocalStyle::Melodic, TrackKind::Vocal, Some(9), true);
        let at = m.iter().find(|s| s.kind == EffectKind::Autotune).unwrap();
        assert_eq!(at.param("key"), 9.0);
        assert_eq!(at.param("scale"), 1.0);
    }

    #[test]
    fn balance_puts_vocals_on_top() {
        let mut p = Project::new("x");
        let beat = Track::new("Instru", TrackKind::Instrumental);
        let v = Track::new("Voix", TrackKind::Vocal);
        let ad = Track::new("Adlibs", TrackKind::Adlib);
        let ids = (beat.id, v.id, ad.id);
        p.tracks = vec![beat, v, ad];
        let moves = balance(&p, &[(ids.0, -10.0), (ids.1, -22.0), (ids.2, -22.0)]);
        let get = |id| moves.iter().find(|m| m.0 == id).unwrap().1;
        assert_eq!(get(ids.0), -10.0);
        assert_eq!(get(ids.1), 3.5);
        assert!(get(ids.2) < get(ids.1));
    }
}
