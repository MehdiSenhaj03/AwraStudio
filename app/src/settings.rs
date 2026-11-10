//! User settings, kept in the configuration folder.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use studio_ai::ProviderConfig;
use studio_plugins::PluginDesc;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// The name shown to collaborators.
    pub user_name: String,
    pub ai: ProviderConfig,
    /// The workspace layout (Dock text form).
    pub layout: String,
    /// Recording latency compensation (ms).
    pub latency_ms: f64,
    pub metronome: bool,
    /// Plugins found by the last scan.
    pub plugins: Vec<PluginDesc>,
    pub recent: Vec<String>,
    pub try_upnp: bool,
    pub sync_transport: bool,
    /// One bar before the take when recording from the middle.
    pub count_in: bool,
    pub appearance: crate::ui::appearance::Appearance,
    /// The microphone and the output by name (None: the system's).
    pub audio_input: Option<String>,
    pub audio_output: Option<String>,
    /// No microphone opened at all.
    pub no_input: bool,
    /// Frames per sound-card callback (0: the card's default).
    pub buffer_frames: u32,
    /// How the voice is heard while singing: 0 straight (fastest), 1
    /// through low-latency effects, 2 through every effect at full quality.
    pub monitor_mode: u8,
    /// Hear the microphone as soon as a track is armed (not only while
    /// recording). Off by default: speakers could whistle.
    pub monitor_armed: bool,
}

/// Buffer sizes offered (frames per callback), with what they mean.
pub const BUFFER_SIZES: [(u32, &str); 5] = [
    (64, "64 — le plus rapide (machine puissante)"),
    (128, "128 — rapide (recommandé)"),
    (256, "256 — équilibré"),
    (512, "512 — sûr (moins de craquements)"),
    (0, "Taille par défaut de la carte"),
];

impl Default for Settings {
    fn default() -> Self {
        let user = std::env::var("USER").or_else(|_| std::env::var("USERNAME")).unwrap_or_else(|_| "Artiste".into());
        let mut name = user;
        if let Some(c) = name.get_mut(0..1) {
            c.make_ascii_uppercase();
        }
        Settings {
            user_name: name,
            ai: ProviderConfig { proactive: true, ..Default::default() },
            layout: String::new(),
            latency_ms: 20.0,
            metronome: false,
            plugins: Vec::new(),
            recent: Vec::new(),
            try_upnp: true,
            sync_transport: true,
            count_in: true,
            appearance: Default::default(),
            audio_input: None,
            audio_output: None,
            no_input: false,
            buffer_frames: 128,
            monitor_mode: 1,
            monitor_armed: false,
        }
    }
}

pub fn config_dir() -> PathBuf {
    if let Ok(d) = std::env::var("AWRA_STUDIO_CONFIG") {
        return PathBuf::from(d);
    }
    dirs::config_dir().unwrap_or_else(std::env::temp_dir).join("awra-studio")
}

impl Settings {
    pub fn load() -> Settings {
        let p = config_dir().join("settings.json");
        std::fs::read_to_string(p).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
    }

    pub fn save(&self) {
        let d = config_dir();
        let _ = std::fs::create_dir_all(&d);
        if let Ok(s) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(d.join("settings.json"), s);
        }
    }

    pub fn device_choice(&self) -> studio_engine::DeviceChoice {
        studio_engine::DeviceChoice {
            input: self.audio_input.clone(),
            output: self.audio_output.clone(),
            no_input: self.no_input,
            buffer_frames: self.buffer_frames,
        }
    }

    pub fn add_recent(&mut self, path: &str) {
        self.recent.retain(|p| p != path);
        self.recent.insert(0, path.to_string());
        self.recent.truncate(8);
    }
}

/// Where songs go by default.
#[allow(dead_code)]
pub fn music_dir() -> PathBuf {
    dirs::audio_dir().or_else(dirs::home_dir).unwrap_or_else(std::env::temp_dir).join("Awra Studio")
}
