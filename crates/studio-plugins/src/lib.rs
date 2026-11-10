//! Plugin hosting: VST3 and CLAP effects in Awra Studio's chains.
//!
//! [`PluginManager`] scans the usual folders (each file in a child
//! process, so a broken plugin can't take the studio down), loads
//! libraries once, and makes processors for the mix graph. It keeps the
//! instances by effect slot so the interface can list their parameters,
//! open their windows (CLAP) and save their state.
//!
//! SPDX-License-Identifier: GPL-3.0-or-later

pub mod clap;
pub mod vst3host;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Weak};

use base64::Engine;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use studio_core::effects::{EffectKind, PluginFormat};
use studio_core::model::{EffectSlot, Id};
use studio_dsp::Processor;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PluginDesc {
    pub name: String,
    pub vendor: String,
    pub format: PluginFormat,
    pub path: String,
    pub id: String,
    pub category: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ParamInfo {
    pub id: String,
    pub name: String,
    pub min: f64,
    pub max: f64,
    pub default: f64,
    pub value: f64,
    pub stepped: bool,
}

/// The folders plugins are installed in.
pub fn search_paths(format: PluginFormat) -> Vec<PathBuf> {
    let mut v = Vec::new();
    let sub = if format == PluginFormat::Clap { "CLAP" } else { "VST3" };
    if cfg!(windows) {
        if let Ok(c) = std::env::var("COMMONPROGRAMFILES") {
            v.push(PathBuf::from(c).join(sub));
        }
        if let Some(l) = dirs::data_local_dir() {
            v.push(l.join("Programs").join("Common").join(sub));
        }
    } else {
        let lower = sub.to_lowercase();
        if let Some(h) = dirs::home_dir() {
            v.push(h.join(format!(".{lower}")));
        }
        for base in ["/usr/lib", "/usr/lib64", "/usr/local/lib"] {
            v.push(PathBuf::from(base).join(&lower));
        }
    }
    let env = if format == PluginFormat::Clap { "CLAP_PATH" } else { "VST3_PATH" };
    if let Ok(extra) = std::env::var(env) {
        v.extend(std::env::split_paths(&extra));
    }
    v
}

/// Plugin files (.clap files, .vst3 bundles or files) under the folders.
pub fn find_files() -> Vec<PathBuf> {
    let mut out = Vec::new();
    fn walk(dir: &Path, ext: &str, out: &mut Vec<PathBuf>, depth: u32) {
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        for e in rd.flatten() {
            let p = e.path();
            let is_ext = p.extension().and_then(|x| x.to_str()).map(|x| x.eq_ignore_ascii_case(ext)).unwrap_or(false);
            if is_ext {
                out.push(p);
            } else if p.is_dir() && depth < 4 {
                walk(&p, ext, out, depth + 1);
            }
        }
    }
    for f in [PluginFormat::Clap, PluginFormat::Vst3] {
        let ext = if f == PluginFormat::Clap { "clap" } else { "vst3" };
        for d in search_paths(f) {
            walk(&d, ext, &mut out, 0);
        }
    }
    out.sort();
    out.dedup();
    out
}

/// Scans one file in this process (what the child process runs).
pub fn scan_file(path: &Path) -> anyhow::Result<Vec<PluginDesc>> {
    let ext = path.extension().and_then(|x| x.to_str()).unwrap_or("").to_lowercase();
    match ext.as_str() {
        "clap" => Ok(clap::ClapLibrary::load(path)?.descriptors()),
        "vst3" => Ok(vst3host::Vst3Module::load(path)?.descriptors()),
        _ => Ok(Vec::new()),
    }
}

/// Scans every installed plugin, each file through `exe --scan-plugin
/// <file>` (which prints JSON), with a timeout.
pub fn scan_all(exe: &Path) -> Vec<PluginDesc> {
    let mut out = Vec::new();
    for f in find_files() {
        let child = std::process::Command::new(exe)
            .arg("--scan-plugin")
            .arg(&f)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn();
        let Ok(mut child) = child else { continue };
        let start = std::time::Instant::now();
        loop {
            match child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) if start.elapsed().as_secs() < 15 => std::thread::sleep(std::time::Duration::from_millis(20)),
                _ => {
                    let _ = child.kill();
                    break;
                }
            }
        }
        if let Ok(o) = child.wait_with_output() {
            if let Ok(list) = serde_json::from_slice::<Vec<PluginDesc>>(&o.stdout) {
                out.extend(list);
            }
        }
    }
    out
}

#[derive(Clone)]
pub enum Instance {
    Clap(Arc<clap::ClapShared>),
    Vst3(Arc<vst3host::Vst3Shared>),
}

impl Instance {
    pub fn name(&self) -> String {
        match self {
            Instance::Clap(c) => c.name.clone(),
            Instance::Vst3(v) => v.name.clone(),
        }
    }
    pub fn params(&self) -> Vec<ParamInfo> {
        match self {
            Instance::Clap(c) => c.params(),
            Instance::Vst3(v) => v.params(),
        }
    }
    pub fn value_text(&self, id: &str, v: f64) -> Option<String> {
        match self {
            Instance::Clap(c) => c.value_text(id, v),
            Instance::Vst3(x) => x.value_text(id, v),
        }
    }
    pub fn save_state(&self) -> Option<Vec<u8>> {
        match self {
            Instance::Clap(c) => c.save_state(),
            Instance::Vst3(v) => v.save_state(),
        }
    }
    pub fn has_gui(&self) -> bool {
        matches!(self, Instance::Clap(c) if c.has_gui())
    }
    pub fn open_gui(&self) -> anyhow::Result<()> {
        match self {
            Instance::Clap(c) => c.open_gui(),
            Instance::Vst3(_) => Err(anyhow::anyhow!("les réglages de ce plugin VST3 sont affichés dans Awra Studio")),
        }
    }
}

enum Lib {
    Clap(Arc<clap::ClapLibrary>),
    Vst3(Arc<vst3host::Vst3Module>),
}

#[derive(Default)]
struct Inner {
    libs: HashMap<String, Lib>,
    instances: HashMap<Id, Instance>,
    clap_weak: Vec<Weak<clap::ClapShared>>,
}

/// Loads plugins and tracks their instances (UI thread).
#[derive(Clone, Default)]
pub struct PluginManager {
    inner: Arc<Mutex<Inner>>,
    pub available: Arc<Mutex<Vec<PluginDesc>>>,
}

pub fn encode_state(b: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(b)
}

pub fn decode_state(s: &str) -> Option<Vec<u8>> {
    base64::engine::general_purpose::STANDARD.decode(s).ok()
}

impl PluginManager {
    pub fn new() -> PluginManager {
        PluginManager::default()
    }

    /// A processor for a plugin slot (the mix graph's factory).
    pub fn create(&self, slot: &EffectSlot, sr: f64, max_block: usize) -> Option<Box<dyn Processor>> {
        self.create_inner(slot, sr, max_block, true)
    }

    /// A processor not tracked by the manager (offline renders).
    pub fn create_detached(&self, slot: &EffectSlot, sr: f64, max_block: usize) -> Option<Box<dyn Processor>> {
        self.create_inner(slot, sr, max_block, false)
    }

    fn create_inner(&self, slot: &EffectSlot, sr: f64, max_block: usize, register: bool) -> Option<Box<dyn Processor>> {
        let EffectKind::Plugin { format, path, plugin_id, .. } = &slot.kind else { return None };
        let mut inner = self.inner.lock();
        let lib_key = path.clone();
        if !inner.libs.contains_key(&lib_key) {
            let lib = match format {
                PluginFormat::Clap => clap::ClapLibrary::load(Path::new(path)).map(Lib::Clap),
                PluginFormat::Vst3 => vst3host::Vst3Module::load(Path::new(path)).map(Lib::Vst3),
            };
            match lib {
                Ok(l) => {
                    inner.libs.insert(lib_key.clone(), l);
                }
                Err(e) => {
                    eprintln!("awra-studio: plugin {path} : {e}");
                    return None;
                }
            }
        }
        let state = slot.plugin_state.as_deref().and_then(decode_state);
        let (inst, mut proc_): (Instance, Box<dyn Processor>) = match inner.libs.get(&lib_key)? {
            Lib::Clap(l) => {
                let shared = clap::ClapShared::create(l.clone(), plugin_id, sr, max_block as u32).ok()?;
                if let Some(s) = &state {
                    shared.load_state(s);
                }
                inner.clap_weak.push(Arc::downgrade(&shared));
                (Instance::Clap(shared.clone()), Box::new(clap::ClapProcessor::new(shared, max_block)))
            }
            Lib::Vst3(m) => {
                let shared = vst3host::Vst3Shared::create(m.clone(), plugin_id, sr, max_block as u32).ok()?;
                if let Some(s) = &state {
                    shared.load_state(s);
                }
                (Instance::Vst3(shared.clone()), Box::new(vst3host::Vst3Processor::new(shared, max_block)?))
            }
        };
        for (k, v) in &slot.params {
            proc_.set_param(k, *v);
            if let Instance::Vst3(s) = &inst {
                s.set_controller_param(k, *v);
            }
        }
        if register {
            inner.instances.insert(slot.id, inst);
        }
        Some(proc_)
    }

    pub fn instance(&self, slot: Id) -> Option<Instance> {
        self.inner.lock().instances.get(&slot).cloned()
    }

    /// States of every live instance (saved into the project on save).
    pub fn states(&self) -> Vec<(Id, String)> {
        self.inner
            .lock()
            .instances
            .iter()
            .filter_map(|(id, i)| i.save_state().map(|s| (*id, encode_state(&s))))
            .collect()
    }

    /// Forgets instances of slots that no longer exist.
    pub fn retain(&self, live: &[Id]) {
        self.inner.lock().instances.retain(|id, _| live.contains(id));
    }

    /// Main-thread housekeeping for CLAP plugins (timers, callbacks).
    pub fn idle(&self) {
        let list: Vec<Arc<clap::ClapShared>> = {
            let mut inner = self.inner.lock();
            inner.clap_weak.retain(|w| w.strong_count() > 0);
            inner.clap_weak.iter().filter_map(Weak::upgrade).collect()
        };
        for c in list {
            c.idle();
        }
    }
}
