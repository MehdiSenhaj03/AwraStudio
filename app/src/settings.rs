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
}

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
