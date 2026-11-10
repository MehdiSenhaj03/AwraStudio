//! Undo / redo over transactions, and the document that owns a project.

use std::path::{Path, PathBuf};

use crate::model::*;
use crate::ops::*;

#[derive(Clone, Debug)]
pub struct Entry {
    pub forward: Transaction,
    pub inverse: Transaction,
    /// Merges quick successive edits of one control (slider drags).
    pub coalesce: Option<String>,
    pub time_ms: u128,
}

#[derive(Default, Debug)]
pub struct History {
    pub undo: Vec<Entry>,
    pub redo: Vec<Entry>,
    pub limit: usize,
}

fn now_ms() -> u128 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0)
}

/// What identifies "the same control" for merging edits.
fn coalesce_key(tx: &Transaction) -> Option<String> {
    if tx.ops.len() != 1 {
        return None;
    }
    match &tx.ops[0] {
        Op::SetParam { target, slot, param, .. } => Some(format!("p{target:?}{slot}{param}")),
        Op::SetTrack { id, field: TrackField::VolumeDb(_) } => Some(format!("v{id}")),
        Op::SetTrack { id, field: TrackField::Pan(_) } => Some(format!("pan{id}")),
        Op::SetProject { field: ProjectField::MasterVolumeDb(_) } => Some("mv".into()),
        Op::SetSection { id, .. } => Some(format!("s{id}")),
        Op::SetClip { track, clip, field: ClipField::Start(_) } => Some(format!("cs{track}{clip}")),
        _ => None,
    }
}

impl History {
    pub fn new() -> History {
        History { undo: Vec::new(), redo: Vec::new(), limit: 500 }
    }

    pub fn record(&mut self, forward: Transaction, inverse: Transaction) {
        let key = coalesce_key(&forward);
        let t = now_ms();
        if let (Some(k), Some(last)) = (&key, self.undo.last_mut()) {
            if last.coalesce.as_ref() == Some(k) && t.saturating_sub(last.time_ms) < 1500 && last.forward.actor == forward.actor
            {
                last.forward = forward;
                last.time_ms = t;
                self.redo.clear();
                return;
            }
        }
        self.undo.push(Entry { forward, inverse, coalesce: key, time_ms: t });
        if self.undo.len() > self.limit.max(1) {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }
}

/// A project, its history and where it lives on disk.
#[derive(Debug)]
pub struct Document {
    pub project: Project,
    pub history: History,
    /// The project folder (`<name>.awra/`), None until saved.
    pub dir: Option<PathBuf>,
    pub dirty: bool,
    /// Incremented on every change (views compare it to refresh).
    pub revision: u64,
    /// The inverse of the transaction the last undo/redo applied
    /// (collaboration rebases with it).
    pub last_inverse: Option<Transaction>,
}

impl Document {
    pub fn new(project: Project) -> Document {
        Document { project, history: History::new(), dir: None, dirty: false, revision: 0, last_inverse: None }
    }

    /// Applies a transaction and records it for undo. Returns the
    /// transaction as applied (to broadcast to collaborators).
    pub fn commit(&mut self, tx: Transaction) -> Result<Transaction, OpError> {
        let inverse = tx.apply(&mut self.project)?;
        self.history.record(tx.clone(), inverse);
        self.touch();
        Ok(tx)
    }

    /// Like [`Document::commit`], also returning the inverse transaction
    /// (collaboration rebases unacknowledged changes with it).
    pub fn commit_inv(&mut self, tx: Transaction) -> Result<(Transaction, Transaction), OpError> {
        let inverse = tx.apply(&mut self.project)?;
        self.history.record(tx.clone(), inverse.clone());
        self.touch();
        Ok((tx, inverse))
    }

    /// Applies a transaction coming from a collaborator: not undoable
    /// locally (each participant undoes their own work).
    pub fn apply_remote(&mut self, tx: &Transaction) -> Result<(), OpError> {
        tx.apply(&mut self.project)?;
        self.touch();
        Ok(())
    }

    fn touch(&mut self) {
        self.dirty = true;
        self.revision += 1;
    }

    /// Undoes the last transaction; returns what was applied.
    pub fn undo(&mut self) -> Option<Transaction> {
        let e = self.history.undo.pop()?;
        match e.inverse.apply(&mut self.project) {
            Ok(inv) => {
                self.touch();
                self.last_inverse = Some(inv);
                let applied = e.inverse.clone();
                self.history.redo.push(e);
                Some(applied)
            }
            Err(_) => None,
        }
    }

    pub fn redo(&mut self) -> Option<Transaction> {
        let e = self.history.redo.pop()?;
        match e.forward.apply(&mut self.project) {
            Ok(inv) => {
                self.touch();
                self.last_inverse = Some(inv.clone());
                let applied = e.forward.clone();
                self.history.undo.push(Entry { inverse: inv, ..e });
                Some(applied)
            }
            Err(_) => None,
        }
    }

    /// Undoes the most recent transaction made by `actor` (e.g. the AI),
    /// even if others came after it, when its inverse still applies.
    pub fn undo_last_by(&mut self, actor: &Actor) -> Option<Transaction> {
        let i = self.history.undo.iter().rposition(|e| &e.forward.actor == actor)?;
        let e = self.history.undo.remove(i);
        match e.inverse.apply(&mut self.project) {
            Ok(inv) => {
                self.touch();
                self.last_inverse = Some(inv);
                Some(e.inverse)
            }
            Err(_) => {
                self.history.undo.insert(i, e);
                None
            }
        }
    }

    /// Undoes a specific transaction (by the id of its forward
    /// transaction), e.g. from the assistant's "Annuler" button.
    pub fn undo_transaction(&mut self, id: Id) -> Option<Transaction> {
        let i = self.history.undo.iter().rposition(|e| e.forward.id == id)?;
        let e = self.history.undo.remove(i);
        match e.inverse.apply(&mut self.project) {
            Ok(inv) => {
                self.touch();
                self.last_inverse = Some(inv);
                Some(e.inverse)
            }
            Err(_) => {
                self.history.undo.insert(i, e);
                None
            }
        }
    }

    pub fn assets_dir(&self) -> Option<PathBuf> {
        self.dir.as_ref().map(|d| d.join("assets"))
    }

    pub fn asset_path(&self, id: &str) -> Option<PathBuf> {
        let info = self.project.assets.get(id)?;
        Some(self.assets_dir()?.join(&info.file))
    }

    /// Saves `project.json` in the folder (created if needed).
    pub fn save_to(&mut self, dir: &Path) -> anyhow::Result<()> {
        std::fs::create_dir_all(dir.join("assets"))?;
        // assets imported before the first save live in a temporary folder
        if let Some(old) = self.dir.clone() {
            if old != dir {
                for info in self.project.assets.values() {
                    let src = old.join("assets").join(&info.file);
                    let dst = dir.join("assets").join(&info.file);
                    if src.exists() && !dst.exists() {
                        std::fs::copy(&src, &dst)?;
                    }
                }
            }
        }
        let json = serde_json::to_string_pretty(&self.project)?;
        let tmp = dir.join("project.json.tmp");
        std::fs::write(&tmp, json)?;
        std::fs::rename(&tmp, dir.join("project.json"))?;
        self.dir = Some(dir.to_path_buf());
        self.dirty = false;
        Ok(())
    }

    pub fn save(&mut self) -> anyhow::Result<()> {
        let dir = self.dir.clone().ok_or_else(|| anyhow::anyhow!("projet jamais enregistré"))?;
        self.save_to(&dir)
    }

    pub fn open(dir: &Path) -> anyhow::Result<Document> {
        let json = std::fs::read_to_string(dir.join("project.json"))?;
        let project: Project = serde_json::from_str(&json)?;
        let mut d = Document::new(project);
        d.dir = Some(dir.to_path_buf());
        Ok(d)
    }

    /// Copies a file into the project's assets (named by its hash) and
    /// returns the asset id and the stored file name. The caller fills
    /// in the audio facts and commits an `AddAsset`.
    pub fn store_asset_file(&mut self, src: &Path) -> anyhow::Result<(AssetId, String)> {
        let dir = self.ensure_dir()?;
        let bytes = std::fs::read(src)?;
        let id = blake3::hash(&bytes).to_hex().to_string();
        let ext = src.extension().and_then(|e| e.to_str()).unwrap_or("bin").to_lowercase();
        let file = format!("{}.{ext}", &id[..24]);
        let dst = dir.join("assets").join(&file);
        if !dst.exists() {
            std::fs::write(&dst, &bytes)?;
        }
        Ok((id, file))
    }

    /// Stores bytes as an asset (recorded takes, collaborators' files).
    pub fn store_asset_bytes(&mut self, bytes: &[u8], ext: &str) -> anyhow::Result<(AssetId, String)> {
        let dir = self.ensure_dir()?;
        let id = blake3::hash(bytes).to_hex().to_string();
        let file = format!("{}.{ext}", &id[..24]);
        let dst = dir.join("assets").join(&file);
        if !dst.exists() {
            std::fs::write(&dst, bytes)?;
        }
        Ok((id, file))
    }

    /// The project folder, a temporary one until the first save.
    pub fn ensure_dir(&mut self) -> anyhow::Result<PathBuf> {
        if self.dir.is_none() {
            let d = std::env::temp_dir().join(format!("awra-studio-{:x}.awra", self.project.id));
            self.dir = Some(d);
        }
        let d = self.dir.clone().unwrap();
        std::fs::create_dir_all(d.join("assets"))?;
        Ok(d)
    }

    /// Whether the folder is a temporary one.
    pub fn is_temporary(&self) -> bool {
        self.dir.as_ref().map_or(true, |d| d.starts_with(std::env::temp_dir()))
    }
}
