//! Built-in effects: their kinds and parameter specifications.
//!
//! The specifications are the shared vocabulary of the UI (sliders), the
//! AI agent (tool schemas) and the DSP (`studio-dsp`).

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum EffectKind {
    Gain,
    Eq,
    Compressor,
    Gate,
    DeEsser,
    Denoise,
    Reverb,
    Delay,
    Saturation,
    Autotune,
    Limiter,
    Doubler,
    StereoWidth,
    /// A VST3 or CLAP plugin.
    Plugin { format: PluginFormat, path: String, plugin_id: String, name: String },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginFormat {
    Clap,
    Vst3,
}

pub const BUILTIN: [EffectKind; 13] = [
    EffectKind::Gain,
    EffectKind::Eq,
    EffectKind::Compressor,
    EffectKind::Gate,
    EffectKind::DeEsser,
    EffectKind::Denoise,
    EffectKind::Reverb,
    EffectKind::Delay,
    EffectKind::Saturation,
    EffectKind::Autotune,
    EffectKind::Limiter,
    EffectKind::Doubler,
    EffectKind::StereoWidth,
];

impl EffectKind {
    /// Stable short name ("eq", "autotune"), used by the AI tools.
    pub fn key(&self) -> &'static str {
        match self {
            EffectKind::Gain => "gain",
            EffectKind::Eq => "eq",
            EffectKind::Compressor => "compressor",
            EffectKind::Gate => "gate",
            EffectKind::DeEsser => "deesser",
            EffectKind::Denoise => "denoise",
            EffectKind::Reverb => "reverb",
            EffectKind::Delay => "delay",
            EffectKind::Saturation => "saturation",
            EffectKind::Autotune => "autotune",
            EffectKind::Limiter => "limiter",
            EffectKind::Doubler => "doubler",
            EffectKind::StereoWidth => "stereo_width",
            EffectKind::Plugin { .. } => "plugin",
        }
    }
    pub fn from_key(key: &str) -> Option<EffectKind> {
        let k = key.trim().to_lowercase().replace(['-', ' '], "_");
        Some(match k.as_str() {
            "gain" | "volume" | "trim" => EffectKind::Gain,
            "eq" | "equalizer" | "equaliseur" | "égaliseur" | "egaliseur" => EffectKind::Eq,
            "compressor" | "compresseur" | "comp" => EffectKind::Compressor,
            "gate" | "noise_gate" => EffectKind::Gate,
            "deesser" | "de_esser" => EffectKind::DeEsser,
            "denoise" | "debruiteur" | "débruiteur" | "noise_reduction" => EffectKind::Denoise,
            "reverb" | "réverb" | "reverbe" | "réverbe" => EffectKind::Reverb,
            "delay" | "echo" | "écho" => EffectKind::Delay,
            "saturation" | "distortion" | "drive" => EffectKind::Saturation,
            "autotune" | "auto_tune" | "pitch_correction" | "tune" => EffectKind::Autotune,
            "limiter" | "limiteur" => EffectKind::Limiter,
            "doubler" | "doubleur" | "chorus" => EffectKind::Doubler,
            "stereo_width" | "width" | "stereo" | "imager" => EffectKind::StereoWidth,
            _ => return None,
        })
    }
    pub fn is_plugin(&self) -> bool {
        matches!(self, EffectKind::Plugin { .. })
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Unit {
    None,
    Db,
    Hz,
    Ms,
    Ratio,
    Percent,
    /// An index into `choices`.
    Choice,
    Toggle,
}

#[derive(Clone, Copy, Debug)]
pub struct ParamSpec {
    pub id: &'static str,
    pub label: &'static str,
    pub min: f64,
    pub max: f64,
    pub default: f64,
    pub unit: Unit,
    /// Displayed / edited on a log scale (frequencies, times).
    pub log: bool,
    pub choices: &'static [&'static str],
    /// Shown in the simple view.
    pub essential: bool,
}

const fn p(id: &'static str, label: &'static str, min: f64, max: f64, default: f64, unit: Unit) -> ParamSpec {
    ParamSpec { id, label, min, max, default, unit, log: false, choices: &[], essential: true }
}
const fn plog(id: &'static str, label: &'static str, min: f64, max: f64, default: f64, unit: Unit) -> ParamSpec {
    ParamSpec { id, label, min, max, default, unit, log: true, choices: &[], essential: true }
}
const fn adv(mut s: ParamSpec) -> ParamSpec {
    s.essential = false;
    s
}
const fn choice(id: &'static str, label: &'static str, choices: &'static [&'static str], default: f64) -> ParamSpec {
    ParamSpec {
        id,
        label,
        min: 0.0,
        max: (choices.len() - 1) as f64,
        default,
        unit: Unit::Choice,
        log: false,
        choices,
        essential: true,
    }
}

pub struct EffectSpec {
    pub name: &'static str,
    pub description: &'static str,
    pub params: &'static [ParamSpec],
}

pub const KEYS: [&str; 12] = ["Do", "Do#", "Ré", "Ré#", "Mi", "Fa", "Fa#", "Sol", "Sol#", "La", "La#", "Si"];
pub const SCALES: [&str; 6] = ["Majeur", "Mineur", "Chromatique", "Penta mineure", "Penta majeure", "Mineur harmonique"];

/// Semitones (from the root) of each scale in [`SCALES`].
pub fn scale_notes(scale: usize) -> &'static [u8] {
    match scale {
        0 => &[0, 2, 4, 5, 7, 9, 11],
        1 => &[0, 2, 3, 5, 7, 8, 10],
        3 => &[0, 3, 5, 7, 10],
        4 => &[0, 2, 4, 7, 9],
        5 => &[0, 2, 3, 5, 7, 8, 11],
        _ => &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11],
    }
}

static GAIN: [ParamSpec; 1] = [p("gain_db", "Gain", -24.0, 24.0, 0.0, Unit::Db)];
static EQ: [ParamSpec; 15] = [
    plog("hp_freq", "Coupe-bas", 20.0, 500.0, 20.0, Unit::Hz),
    p("low_gain", "Graves", -15.0, 15.0, 0.0, Unit::Db),
    adv(plog("low_freq", "Fréq. graves", 40.0, 400.0, 110.0, Unit::Hz)),
    p("lowmid_gain", "Bas-médiums", -15.0, 15.0, 0.0, Unit::Db),
    adv(plog("lowmid_freq", "Fréq. bas-médiums", 150.0, 1000.0, 350.0, Unit::Hz)),
    adv(p("lowmid_q", "Q bas-médiums", 0.3, 6.0, 1.0, Unit::Ratio)),
    p("mid_gain", "Médiums", -15.0, 15.0, 0.0, Unit::Db),
    adv(plog("mid_freq", "Fréq. médiums", 500.0, 5000.0, 1500.0, Unit::Hz)),
    adv(p("mid_q", "Q médiums", 0.3, 6.0, 1.0, Unit::Ratio)),
    p("presence_gain", "Présence", -15.0, 15.0, 0.0, Unit::Db),
    adv(plog("presence_freq", "Fréq. présence", 2000.0, 9000.0, 4500.0, Unit::Hz)),
    p("air_gain", "Air", -15.0, 15.0, 0.0, Unit::Db),
    adv(plog("air_freq", "Fréq. air", 6000.0, 18000.0, 11000.0, Unit::Hz)),
    adv(plog("lp_freq", "Coupe-haut", 2000.0, 20000.0, 20000.0, Unit::Hz)),
    adv(p("output_db", "Sortie", -12.0, 12.0, 0.0, Unit::Db)),
];
static COMP: [ParamSpec; 7] = [
    p("threshold_db", "Seuil", -60.0, 0.0, -18.0, Unit::Db),
    p("ratio", "Ratio", 1.0, 20.0, 3.0, Unit::Ratio),
    plog("attack_ms", "Attaque", 0.1, 100.0, 8.0, Unit::Ms),
    plog("release_ms", "Relâchement", 10.0, 1000.0, 120.0, Unit::Ms),
    p("makeup_db", "Gain de compensation", 0.0, 24.0, 3.0, Unit::Db),
    adv(p("knee_db", "Genou", 0.0, 12.0, 6.0, Unit::Db)),
    adv(p("mix", "Mélange (parallèle)", 0.0, 1.0, 1.0, Unit::Percent)),
];
static GATE: [ParamSpec; 5] = [
    p("threshold_db", "Seuil", -90.0, -10.0, -55.0, Unit::Db),
    p("range_db", "Atténuation", 0.0, 80.0, 30.0, Unit::Db),
    adv(plog("attack_ms", "Attaque", 0.1, 50.0, 1.0, Unit::Ms)),
    adv(plog("hold_ms", "Maintien", 1.0, 500.0, 40.0, Unit::Ms)),
    adv(plog("release_ms", "Relâchement", 5.0, 1000.0, 120.0, Unit::Ms)),
];
static DEESS: [ParamSpec; 3] = [
    plog("freq", "Fréquence", 3000.0, 11000.0, 6500.0, Unit::Hz),
    p("threshold_db", "Seuil", -60.0, 0.0, -30.0, Unit::Db),
    p("amount_db", "Réduction max", 0.0, 24.0, 8.0, Unit::Db),
];
static DENOISE: [ParamSpec; 2] = [
    p("amount", "Quantité", 0.0, 1.0, 0.5, Unit::Percent),
    adv(p("sensitivity", "Sensibilité", 0.0, 1.0, 0.5, Unit::Percent)),
];
static REVERB: [ParamSpec; 7] = [
    p("size", "Taille", 0.0, 1.0, 0.5, Unit::Percent),
    p("mix", "Quantité", 0.0, 1.0, 0.18, Unit::Percent),
    adv(p("damping", "Amorti", 0.0, 1.0, 0.45, Unit::Percent)),
    adv(plog("predelay_ms", "Pré-délai", 0.0, 200.0, 20.0, Unit::Ms)),
    adv(p("width", "Largeur", 0.0, 1.0, 1.0, Unit::Percent)),
    adv(plog("low_cut", "Coupe-bas", 20.0, 1000.0, 250.0, Unit::Hz)),
    adv(plog("high_cut", "Coupe-haut", 1000.0, 20000.0, 9000.0, Unit::Hz)),
];
static DELAY: [ParamSpec; 6] = [
    plog("time_ms", "Temps", 1.0, 2000.0, 375.0, Unit::Ms),
    choice("sync", "Synchro tempo", &["Libre", "1/16", "1/8", "1/8 pointée", "1/4", "1/2", "1 mesure"], 4.0),
    p("feedback", "Répétitions", 0.0, 0.95, 0.3, Unit::Percent),
    p("mix", "Quantité", 0.0, 1.0, 0.15, Unit::Percent),
    adv(p("ping_pong", "Ping-pong", 0.0, 1.0, 1.0, Unit::Toggle)),
    adv(plog("high_cut", "Coupe-haut", 1000.0, 20000.0, 6000.0, Unit::Hz)),
];
static SAT: [ParamSpec; 3] = [
    p("drive_db", "Drive", 0.0, 30.0, 6.0, Unit::Db),
    p("tone", "Tonalité", -1.0, 1.0, 0.0, Unit::None),
    p("mix", "Quantité", 0.0, 1.0, 0.5, Unit::Percent),
];
static AUTOTUNE: [ParamSpec; 7] = [
    choice("key", "Tonalité", &KEYS, 0.0),
    choice("scale", "Gamme", &SCALES, 1.0),
    p("speed", "Vitesse de correction", 0.0, 1.0, 0.8, Unit::Percent),
    p("amount", "Intensité", 0.0, 1.0, 1.0, Unit::Percent),
    adv(p("humanize", "Humanisation", 0.0, 1.0, 0.2, Unit::Percent)),
    adv(p("mix", "Mélange", 0.0, 1.0, 1.0, Unit::Percent)),
    adv(p("formant", "Préserver le timbre", 0.0, 1.0, 1.0, Unit::Toggle)),
];
static LIMITER: [ParamSpec; 3] = [
    p("gain_db", "Gain d'entrée", 0.0, 24.0, 0.0, Unit::Db),
    p("ceiling_db", "Plafond", -6.0, 0.0, -1.0, Unit::Db),
    adv(plog("release_ms", "Relâchement", 5.0, 500.0, 60.0, Unit::Ms)),
];
static DOUBLER: [ParamSpec; 4] = [
    p("detune_cents", "Désaccord", 0.0, 40.0, 12.0, Unit::None),
    plog("delay_ms", "Décalage", 5.0, 60.0, 18.0, Unit::Ms),
    p("width", "Largeur", 0.0, 1.0, 0.85, Unit::Percent),
    p("mix", "Quantité", 0.0, 1.0, 0.35, Unit::Percent),
];
static WIDTH: [ParamSpec; 2] = [
    p("width", "Largeur", 0.0, 2.0, 1.0, Unit::Ratio),
    adv(plog("bass_mono_freq", "Graves en mono sous", 20.0, 300.0, 120.0, Unit::Hz)),
];
static NONE: [ParamSpec; 0] = [];

pub fn spec(kind: &EffectKind) -> EffectSpec {
    match kind {
        EffectKind::Gain => EffectSpec { name: "Gain", description: "Volume avant / après la chaîne.", params: &GAIN },
        EffectKind::Eq => EffectSpec {
            name: "Égaliseur",
            description: "Équilibre tonal : coupe-bas, graves, médiums, présence, air.",
            params: &EQ,
        },
        EffectKind::Compressor => EffectSpec {
            name: "Compresseur",
            description: "Régule la dynamique : une voix plus constante et plus en avant.",
            params: &COMP,
        },
        EffectKind::Gate => {
            EffectSpec { name: "Noise gate", description: "Coupe les bruits entre les phrases.", params: &GATE }
        }
        EffectKind::DeEsser => {
            EffectSpec { name: "De-esser", description: "Adoucit les « s » et sifflantes.", params: &DEESS }
        }
        EffectKind::Denoise => EffectSpec {
            name: "Débruiteur",
            description: "Réduit le souffle et le bruit de fond de la pièce.",
            params: &DENOISE,
        },
        EffectKind::Reverb => {
            EffectSpec { name: "Réverbe", description: "Espace et profondeur.", params: &REVERB }
        }
        EffectKind::Delay => EffectSpec { name: "Delay", description: "Écho calé sur le tempo.", params: &DELAY },
        EffectKind::Saturation => {
            EffectSpec { name: "Saturation", description: "Chaleur, grain et présence.", params: &SAT }
        }
        EffectKind::Autotune => EffectSpec {
            name: "Autotune",
            description: "Correction de justesse : naturelle (lente) ou effet robotique (rapide).",
            params: &AUTOTUNE,
        },
        EffectKind::Limiter => {
            EffectSpec { name: "Limiteur", description: "Volume maximal sans saturer.", params: &LIMITER }
        }
        EffectKind::Doubler => EffectSpec {
            name: "Doubleur",
            description: "Épaissit la voix avec des copies légèrement désaccordées.",
            params: &DOUBLER,
        },
        EffectKind::StereoWidth => {
            EffectSpec { name: "Largeur stéréo", description: "Élargit ou resserre l'image.", params: &WIDTH }
        }
        EffectKind::Plugin { .. } => {
            EffectSpec { name: "Plugin", description: "Plugin externe (VST3 / CLAP).", params: &NONE }
        }
    }
}

/// The display name of an effect slot's kind (plugins by their name).
pub fn display_name(kind: &EffectKind) -> String {
    match kind {
        EffectKind::Plugin { name, format, .. } => {
            format!("{name} ({})", if *format == PluginFormat::Clap { "CLAP" } else { "VST3" })
        }
        k => spec(k).name.to_string(),
    }
}

impl ParamSpec {
    pub fn clamp(&self, v: f64) -> f64 {
        if self.unit == Unit::Choice || self.unit == Unit::Toggle {
            v.round().clamp(self.min, self.max)
        } else {
            v.clamp(self.min, self.max)
        }
    }
    /// 0..1 position of a value on a slider.
    pub fn to_norm(&self, v: f64) -> f64 {
        if self.log && self.min > 0.0 {
            ((v.max(self.min)).ln() - self.min.ln()) / (self.max.ln() - self.min.ln())
        } else if self.log {
            // log-ish for ranges starting at 0
            ((v - self.min) / (self.max - self.min)).max(0.0).sqrt()
        } else {
            (v - self.min) / (self.max - self.min)
        }
    }
    pub fn from_norm(&self, n: f64) -> f64 {
        let n = n.clamp(0.0, 1.0);
        let v = if self.log && self.min > 0.0 {
            (self.min.ln() + n * (self.max.ln() - self.min.ln())).exp()
        } else if self.log {
            self.min + n * n * (self.max - self.min)
        } else {
            self.min + n * (self.max - self.min)
        };
        self.clamp(v)
    }
    /// A human readable value ("-3.0 dB", "4.5 kHz").
    pub fn format(&self, v: f64) -> String {
        match self.unit {
            Unit::Db if self.min >= 0.0 => format!("{v:.1} dB"),
            Unit::Db => format!("{v:+.1} dB"),
            Unit::Hz if v >= 1000.0 => format!("{:.1} kHz", v / 1000.0),
            Unit::Hz => format!("{v:.0} Hz"),
            Unit::Ms if v >= 1000.0 => format!("{:.2} s", v / 1000.0),
            Unit::Ms => format!("{v:.1} ms"),
            Unit::Ratio => format!("{v:.1}"),
            Unit::Percent => format!("{:.0} %", v * 100.0),
            Unit::Choice => self.choices.get(v.round() as usize).copied().unwrap_or("?").to_string(),
            Unit::Toggle => if v >= 0.5 { "Oui" } else { "Non" }.to_string(),
            Unit::None => format!("{v:.2}"),
        }
    }
}

pub fn param_spec(kind: &EffectKind, id: &str) -> Option<ParamSpec> {
    spec(kind).params.iter().find(|p| p.id == id).copied()
}
