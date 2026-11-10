//! The project model: tracks, clips, effect chains, lyrics, references.
//!
//! Everything here is plain data (serde). It is only ever mutated through
//! [`crate::ops::Op`] so that undo, the AI agent and collaboration share a
//! single path.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::effects::EffectKind;

/// A 64-bit identifier, random so that peers can create objects without
/// coordination.
pub type Id = u64;

pub fn new_id() -> Id {
    loop {
        let v = fastrand::u64(..);
        if v != 0 {
            return v;
        }
    }
}

/// An asset is identified by the BLAKE3 hash of its file (hex).
pub type AssetId = String;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TrackKind {
    /// The beat / instrumental.
    Instrumental,
    /// A main vocal.
    #[default]
    Vocal,
    /// Ad-libs, backs, doubles.
    Adlib,
    /// Any other audio.
    Audio,
}

impl TrackKind {
    pub fn label(self) -> &'static str {
        match self {
            TrackKind::Instrumental => "Instru",
            TrackKind::Vocal => "Voix",
            TrackKind::Adlib => "Adlibs",
            TrackKind::Audio => "Audio",
        }
    }
    pub fn parse(s: &str) -> Option<TrackKind> {
        let s = s.to_lowercase();
        Some(match s.as_str() {
            "instrumental" | "instru" | "beat" | "prod" => TrackKind::Instrumental,
            "vocal" | "voix" | "voice" | "lead" => TrackKind::Vocal,
            "adlib" | "adlibs" | "backs" | "back" | "double" => TrackKind::Adlib,
            "audio" => TrackKind::Audio,
            _ => return None,
        })
    }
}

/// Who made a change (shown in the history, used to undo AI work).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case", tag = "type", content = "name")]
pub enum Actor {
    #[default]
    User,
    Peer(String),
    Ai,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EffectSlot {
    pub id: Id,
    pub kind: EffectKind,
    pub enabled: bool,
    /// Parameter values by id (see [`crate::effects::spec`]).
    pub params: BTreeMap<String, f64>,
    /// For plugin effects: the plugin's state blob (base64), if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plugin_state: Option<String>,
    #[serde(default)]
    pub added_by: Actor,
}

impl EffectSlot {
    /// A slot with the default parameters of its kind.
    pub fn new(kind: EffectKind) -> EffectSlot {
        let params = crate::effects::spec(&kind)
            .params
            .iter()
            .map(|p| (p.id.to_string(), p.default))
            .collect();
        EffectSlot { id: new_id(), kind, enabled: true, params, plugin_state: None, added_by: Actor::User }
    }
    pub fn with(mut self, param: &str, value: f64) -> EffectSlot {
        self.params.insert(param.to_string(), value);
        self
    }
    pub fn by(mut self, actor: Actor) -> EffectSlot {
        self.added_by = actor;
        self
    }
    pub fn param(&self, id: &str) -> f64 {
        self.params.get(id).copied().unwrap_or_else(|| {
            crate::effects::spec(&self.kind).params.iter().find(|p| p.id == id).map(|p| p.default).unwrap_or(0.0)
        })
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Clip {
    pub id: Id,
    pub asset: AssetId,
    pub name: String,
    /// Where the clip starts on the timeline (s).
    pub start: f64,
    /// Where in the asset it starts (s).
    pub offset: f64,
    /// Its length (s).
    pub length: f64,
    #[serde(default)]
    pub gain_db: f64,
    #[serde(default)]
    pub fade_in: f64,
    #[serde(default)]
    pub fade_out: f64,
    #[serde(default)]
    pub muted: bool,
}

impl Clip {
    pub fn end(&self) -> f64 {
        self.start + self.length
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Track {
    pub id: Id,
    pub name: String,
    pub kind: TrackKind,
    /// RGB hex, "#e86a5c".
    pub color: String,
    pub volume_db: f64,
    /// -1 (left) .. 1 (right).
    pub pan: f64,
    pub mute: bool,
    pub solo: bool,
    /// Armed for recording.
    pub armed: bool,
    pub effects: Vec<EffectSlot>,
    pub clips: Vec<Clip>,
    /// The participant this track belongs to (collaboration), if any.
    #[serde(default)]
    pub owner: Option<String>,
}

pub const TRACK_COLORS: [&str; 8] =
    ["#e86a5c", "#5c9ce8", "#e8b25c", "#7bd389", "#b77be8", "#e87bb7", "#5cd6e8", "#c9e85c"];

impl Track {
    pub fn new(name: &str, kind: TrackKind) -> Track {
        let color = TRACK_COLORS[fastrand::usize(..TRACK_COLORS.len())].to_string();
        Track {
            id: new_id(),
            name: name.to_string(),
            kind,
            color,
            volume_db: 0.0,
            pan: 0.0,
            mute: false,
            solo: false,
            armed: false,
            effects: Vec::new(),
            clips: Vec::new(),
            owner: None,
        }
    }
    pub fn end(&self) -> f64 {
        self.clips.iter().map(Clip::end).fold(0.0, f64::max)
    }
    pub fn clip(&self, id: Id) -> Option<&Clip> {
        self.clips.iter().find(|c| c.id == id)
    }
    pub fn effect(&self, id: Id) -> Option<&EffectSlot> {
        self.effects.iter().find(|e| e.id == id)
    }
}

/// The master bus: the mastering chain and the output level.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct Master {
    pub volume_db: f64,
    pub effects: Vec<EffectSlot>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AssetKind {
    #[default]
    Import,
    Take,
    Render,
    Reference,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AssetInfo {
    pub id: AssetId,
    pub name: String,
    /// File name inside the project's `assets/` folder.
    pub file: String,
    pub kind: AssetKind,
    pub duration: f64,
    pub channels: u16,
    pub sample_rate: u32,
    /// Who recorded / imported it.
    #[serde(default)]
    pub author: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LyricSection {
    pub id: Id,
    /// "Couplet 1", "Refrain"...
    pub title: String,
    pub text: String,
    /// Optional link to the timeline (s).
    #[serde(default)]
    pub start: Option<f64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Marker {
    pub id: Id,
    pub name: String,
    pub time: f64,
}

/// A reference track the AI can learn an aesthetic from.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Reference {
    pub id: Id,
    pub asset: AssetId,
    pub name: String,
    /// What the user wants from it ("autotune, reverb, ambiance").
    #[serde(default)]
    pub note: String,
}

/// A saved mix: every track's level, pan, mute and chain, and the master.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct MixSnapshot {
    pub tracks: BTreeMap<Id, TrackMix>,
    pub master: Master,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TrackMix {
    pub volume_db: f64,
    pub pan: f64,
    pub mute: bool,
    pub effects: Vec<EffectSlot>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MixVariant {
    pub id: Id,
    pub name: String,
    pub snapshot: MixSnapshot,
    #[serde(default)]
    pub actor: Actor,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Project {
    pub id: Id,
    pub name: String,
    pub bpm: f64,
    pub sample_rate: u32,
    /// Musical key, 0 = C .. 11 = B; None unknown.
    #[serde(default)]
    pub key: Option<u8>,
    /// 0 major, 1 minor (see `effects::Scale`).
    #[serde(default)]
    pub minor: bool,
    pub tracks: Vec<Track>,
    pub master: Master,
    pub assets: BTreeMap<AssetId, AssetInfo>,
    pub lyrics: Vec<LyricSection>,
    pub markers: Vec<Marker>,
    pub references: Vec<Reference>,
    pub variants: Vec<MixVariant>,
    /// Loop region (s).
    #[serde(default)]
    pub loop_region: Option<(f64, f64)>,
}

impl Default for Project {
    fn default() -> Self {
        Project::new("Sans titre")
    }
}

pub const NOTE_NAMES: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];
pub const NOTE_NAMES_FR: [&str; 12] = ["Do", "Do#", "Ré", "Ré#", "Mi", "Fa", "Fa#", "Sol", "Sol#", "La", "La#", "Si"];

impl Project {
    pub fn new(name: &str) -> Project {
        Project {
            id: new_id(),
            name: name.to_string(),
            bpm: 120.0,
            sample_rate: 48_000,
            key: None,
            minor: false,
            tracks: Vec::new(),
            master: Master::default(),
            assets: BTreeMap::new(),
            lyrics: Vec::new(),
            markers: Vec::new(),
            references: Vec::new(),
            variants: Vec::new(),
            loop_region: None,
        }
    }

    pub fn track(&self, id: Id) -> Option<&Track> {
        self.tracks.iter().find(|t| t.id == id)
    }
    pub fn track_mut(&mut self, id: Id) -> Option<&mut Track> {
        self.tracks.iter_mut().find(|t| t.id == id)
    }
    pub fn track_index(&self, id: Id) -> Option<usize> {
        self.tracks.iter().position(|t| t.id == id)
    }
    /// Finds a track by id, exact name, then name prefix / substring
    /// (case-insensitive): what users and the AI call tracks by.
    pub fn find_track(&self, query: &str) -> Option<&Track> {
        if let Ok(id) = query.parse::<Id>() {
            if let Some(t) = self.track(id) {
                return Some(t);
            }
        }
        let q = query.trim().to_lowercase();
        self.tracks
            .iter()
            .find(|t| t.name.to_lowercase() == q)
            .or_else(|| self.tracks.iter().find(|t| t.name.to_lowercase().starts_with(&q)))
            .or_else(|| self.tracks.iter().find(|t| t.name.to_lowercase().contains(&q)))
    }
    /// The end of the last clip (s).
    pub fn duration(&self) -> f64 {
        self.tracks.iter().map(Track::end).fold(0.0, f64::max)
    }
    pub fn key_name(&self) -> Option<String> {
        self.key.map(|k| format!("{} {}", NOTE_NAMES[k as usize % 12], if self.minor { "minor" } else { "major" }))
    }
    pub fn snapshot_mix(&self) -> MixSnapshot {
        MixSnapshot {
            tracks: self
                .tracks
                .iter()
                .map(|t| {
                    (t.id, TrackMix { volume_db: t.volume_db, pan: t.pan, mute: t.mute, effects: t.effects.clone() })
                })
                .collect(),
            master: self.master.clone(),
        }
    }
    /// Tracks that are heard (solo rules).
    pub fn audible(&self, track: &Track) -> bool {
        if track.mute {
            return false;
        }
        let any_solo = self.tracks.iter().any(|t| t.solo);
        !any_solo || track.solo
    }
    /// Seconds per beat.
    pub fn beat(&self) -> f64 {
        60.0 / self.bpm.max(1.0)
    }
}

/// Decibels to a linear gain.
pub fn db_to_gain(db: f64) -> f64 {
    if db <= -96.0 {
        0.0
    } else {
        10f64.powf(db / 20.0)
    }
}

pub fn gain_to_db(g: f64) -> f64 {
    if g <= 1e-6 {
        -120.0
    } else {
        20.0 * g.log10()
    }
}
