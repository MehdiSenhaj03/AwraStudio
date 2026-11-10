//! What the agent can do in the studio: the application implements this.

use serde::{Deserialize, Serialize};
use studio_analysis::Analysis;
use studio_core::model::*;
use studio_core::ops::Transaction;

/// Panels of the workspace (the customizable interface).
pub const PANELS: [(&str, &str); 10] = [
    ("timeline", "Timeline multipiste"),
    ("lyrics", "Paroles"),
    ("assistant", "Ingénieur IA"),
    ("effects", "Effets de la piste"),
    ("mixer", "Table de mixage"),
    ("master", "Mastering"),
    ("session", "Session à plusieurs"),
    ("browser", "Fichiers et références"),
    ("plugins", "Plugins VST3 / CLAP"),
    ("history", "Historique"),
];

/// Interface changes the agent can ask for.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "action")]
pub enum LayoutCommand {
    Show { panel: String },
    Hide { panel: String },
    /// Moves a panel to an area: "left", "right", "bottom", "center".
    Move { panel: String, area: String },
    /// Grows (> 1) or shrinks (< 1) a panel's share.
    Resize { panel: String, factor: f64 },
    /// A ready-made workspace: "default", "recording", "writing", "mixing",
    /// "minimal".
    Preset { name: String },
    /// Keep only these panels.
    Only { panels: Vec<String> },
    /// Shows the effect controls of a track (under it / in the effects panel).
    FocusTrack { track: String },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PluginDesc {
    pub name: String,
    pub vendor: String,
    pub format: studio_core::effects::PluginFormat,
    pub path: String,
    pub id: String,
    pub category: String,
}

pub trait StudioHost {
    /// A snapshot of the project.
    fn project(&self) -> Project;
    /// Analyzes a track, raw (without its effects) or processed.
    fn analyze_track(&mut self, track: Id, processed: bool) -> Result<Analysis, String>;
    /// Analyzes the whole mix as it plays now.
    fn analyze_mix(&mut self) -> Result<Analysis, String>;
    fn analyze_reference(&mut self, reference: Id) -> Result<Analysis, String>;
    /// Integrated loudness (LUFS) of a variant of the project rendered
    /// offline (used to calibrate mastering).
    fn measure_lufs(&mut self, project: &Project) -> Result<f64, String>;
    /// Applies a transaction; returns its id (for undo).
    fn commit(&mut self, tx: Transaction) -> Result<Id, String>;
    fn layout(&mut self, cmd: LayoutCommand) -> Result<String, String>;
    fn plugins(&self) -> Vec<PluginDesc>;
    /// Undoes the agent's last transaction.
    fn undo_ai(&mut self) -> Result<String, String>;
}
