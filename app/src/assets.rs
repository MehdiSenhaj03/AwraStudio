//! Decoded audio of the project's assets, loaded in the background, with
//! waveform overviews for drawing.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use crossbeam_channel::{unbounded, Receiver, Sender};
use studio_dsp::graph::AssetStore;
use studio_dsp::{AudioBuffer, Peaks};

pub struct AssetCache {
    pub store: AssetStore,
    pub peaks: HashMap<String, Arc<Peaks>>,
    loading: HashSet<String>,
    tx: Sender<(String, Result<AudioBuffer, String>)>,
    rx: Receiver<(String, Result<AudioBuffer, String>)>,
    pub errors: Vec<String>,
}

impl AssetCache {
    pub fn new() -> AssetCache {
        let (tx, rx) = unbounded();
        AssetCache { store: AssetStore::new(), peaks: HashMap::new(), loading: HashSet::new(), tx, rx, errors: Vec::new() }
    }

    pub fn has(&self, id: &str) -> bool {
        self.store.contains_key(id)
    }

    pub fn is_loading(&self) -> bool {
        !self.loading.is_empty()
    }

    /// Starts decoding a file (no-op when loaded or loading).
    pub fn request(&mut self, id: &str, path: PathBuf, rate: u32) {
        if self.store.contains_key(id) || self.loading.contains(id) {
            return;
        }
        self.loading.insert(id.to_string());
        let tx = self.tx.clone();
        let id = id.to_string();
        std::thread::spawn(move || {
            let r = studio_dsp::io::load(&path, rate).map_err(|e| e.to_string());
            let _ = tx.send((id, r));
        });
    }

    /// Inserts an already decoded buffer (a fresh take).
    pub fn insert(&mut self, id: &str, buf: AudioBuffer) {
        self.peaks.insert(id.to_string(), Arc::new(Peaks::of(&buf)));
        self.store.insert(id.to_string(), Arc::new(buf));
    }

    /// Collects finished loads; true when something new arrived.
    pub fn poll(&mut self) -> bool {
        let mut any = false;
        while let Ok((id, r)) = self.rx.try_recv() {
            self.loading.remove(&id);
            match r {
                Ok(buf) => {
                    self.insert(&id, buf);
                    any = true;
                }
                Err(e) => self.errors.push(e),
            }
        }
        any
    }
}
