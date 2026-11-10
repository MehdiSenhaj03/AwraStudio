//! The studio: the document, the engine, plugins, collaboration and the
//! AI, tied together. Everything that changes the song goes through
//! [`Studio::commit`].

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crossbeam_channel::{unbounded, Receiver, Sender};
use studio_ai::host::{LayoutCommand, PluginDesc};
use studio_ai::{ChatMessage, ProviderKind};
use studio_collab::{Event, PeerInfo, Replica, Session, TransportMsg};
use studio_core::model::*;
use studio_core::ops::{Op, Transaction};
use studio_core::Document;
use studio_dsp::graph::{AssetStore, MixGraph, ProcessorFactory};
use studio_dsp::AudioBuffer;
use studio_engine::{Cmd, Engine};
use studio_plugins::PluginManager;

use crate::aihost::{self, AiMsg, Request};
use crate::assets::AssetCache;
use crate::settings::Settings;

/// A line of the assistant's conversation.
#[derive(Clone, Debug)]
pub struct AiEntry {
    pub user: bool,
    pub text: String,
    /// (transaction id, label): what can be undone from here.
    pub actions: Vec<(Id, String)>,
    pub undone: Vec<Id>,
    pub note: Option<String>,
}

pub struct Studio {
    pub doc: Document,
    pub assets: AssetCache,
    pub engine: Engine,
    pub plugins: PluginManager,
    pub settings: Settings,
    pub selected: Option<Id>,
    pub selected_clip: Option<(Id, Id)>,
    /// Shown in the effects panel: a track's chain or the master's.
    pub show_master_chain: bool,
    // collaboration
    pub session: Option<Session>,
    pub replica: Replica,
    pub peers: Vec<PeerInfo>,
    pub chat: Vec<(String, String)>,
    // the engine's view of slots (see MixGraph::create_new)
    engine_slots: Vec<Id>,
    // AI
    pub ai_entries: Vec<AiEntry>,
    pub ai_history: Vec<ChatMessage>,
    pub ai_busy: Option<String>,
    req_tx: Sender<Request>,
    req_rx: Receiver<Request>,
    ai_tx: Sender<AiMsg>,
    ai_rx: Receiver<AiMsg>,
    // things for the UI to show or do
    pub toasts: Vec<String>,
    pub layout_requests: Vec<LayoutCommand>,
    pub recording: Option<Id>,
    pub rec_start: f64,
    pub zoom: f64,
    pub scroll_x: f64,
    pub scroll_y: f64,
    /// Bumped when the UI must rebuild panels.
    pub ui_revision: u64,
    pub last_export: Option<(PathBuf, f64, f64)>,
    pub export_busy: bool,
    export_rx: Option<Receiver<Result<(PathBuf, f64, f64), String>>>,
    pub scan_busy: bool,
    join_rx: Option<Receiver<Result<Session, String>>>,
    last_autosave: f64,
    scan_rx: Option<Receiver<Vec<studio_plugins::PluginDesc>>>,
}

fn factory(pm: &PluginManager) -> impl Fn(&EffectSlot, f64, usize) -> Option<Box<dyn studio_dsp::Processor>> + 'static {
    let pm = pm.clone();
    move |s, sr, mb| pm.create(s, sr, mb)
}

impl Studio {
    pub fn new(doc: Document, settings: Settings) -> Studio {
        let plugins = PluginManager::new();
        let g = MixGraph::build(&doc.project, &AssetStore::new(), None, true);
        let mut engine = Engine::new(doc.project.sample_rate, g);
        engine.latency_ms = settings.latency_ms;
        let (req_tx, req_rx) = unbounded();
        let (ai_tx, ai_rx) = unbounded();
        let mut s = Studio {
            doc,
            assets: AssetCache::new(),
            engine,
            plugins,
            settings,
            selected: None,
            selected_clip: None,
            show_master_chain: false,
            session: None,
            replica: Replica::new(0),
            peers: Vec::new(),
            chat: Vec::new(),
            engine_slots: Vec::new(),
            ai_entries: Vec::new(),
            ai_history: Vec::new(),
            ai_busy: None,
            req_tx,
            req_rx,
            ai_tx,
            ai_rx,
            toasts: Vec::new(),
            layout_requests: Vec::new(),
            recording: None,
            rec_start: 0.0,
            zoom: 60.0,
            scroll_x: 0.0,
            scroll_y: 0.0,
            ui_revision: 1,
            last_export: None,
            export_busy: false,
            export_rx: None,
            scan_busy: false,
            join_rx: None,
            last_autosave: awra::now_ms(),
            scan_rx: None,
        };
        s.engine.send(Cmd::Metronome(s.settings.metronome));
        s.engine.send(Cmd::Bpm(s.doc.project.bpm));
        s.load_assets();
        s.rebuild_graph();
        s.selected = s.doc.project.tracks.iter().find(|t| t.kind != TrackKind::Instrumental).or(s.doc.project.tracks.first()).map(|t| t.id);
        s.ai_entries.push(AiEntry {
            user: false,
            text: "Salut ! Je suis ton ingénieur du son. Importe une instru, enregistre-toi, puis demande-moi ce que tu veux : « rends ma voix plus propre », « mets un autotune », « mixe nos voix », « finalise le son »…".into(),
            actions: Vec::new(),
            undone: Vec::new(),
            note: None,
        });
        s
    }

    /// Replaces the document (open, new, join).
    pub fn set_document(&mut self, doc: Document) {
        self.engine.stop();
        self.doc = doc;
        self.assets = AssetCache::new();
        self.engine_slots.clear();
        self.plugins.retain(&[]);
        self.load_assets();
        self.rebuild_graph();
        self.selected = self.doc.project.tracks.iter().find(|t| t.kind != TrackKind::Instrumental).map(|t| t.id);
        self.selected_clip = None;
        self.engine.seek(0.0);
        self.engine.send(Cmd::Bpm(self.doc.project.bpm));
        if let Some(s) = &self.session {
            if s.role == studio_collab::Role::Host {
                // everyone switches to the new song (files follow on request)
                s.reset(self.doc.project.clone());
                for (id, info) in self.doc.project.assets.clone() {
                    if let Some(p) = self.doc.asset_path(&id) {
                        if let Ok(bytes) = std::fs::read(p) {
                            s.share_asset(&id, &info.file, Arc::new(bytes));
                        }
                    }
                }
            }
        }
        self.ui_revision += 1;
    }

    pub fn load_assets(&mut self) {
        let rate = self.doc.project.sample_rate;
        let ids: Vec<String> = self.doc.project.assets.keys().cloned().collect();
        for id in ids {
            if let Some(path) = self.doc.asset_path(&id) {
                if path.exists() {
                    self.assets.request(&id, path, rate);
                } else if let Some(s) = &self.session {
                    s.request_asset(&id);
                }
            }
        }
    }

    pub fn rebuild_graph(&mut self) {
        let f = factory(&self.plugins);
        let fref: &ProcessorFactory = &f;
        let mut g = MixGraph::build(&self.doc.project, &self.assets.store, Some(fref), false);
        g.create_new(&self.doc.project, &self.engine_slots, Some(fref));
        self.engine_slots = MixGraph::all_slot_ids(&self.doc.project);
        self.plugins.retain(&self.engine_slots);
        if let Some(t) = self.recording {
            g.set_monitor(Some(t));
        }
        self.engine.set_graph(g);
    }

    /// What follows any change: the engine and the views.
    fn after_change(&mut self, tx: &Transaction) {
        let mut rebuild = false;
        for op in &tx.ops {
            match op {
                Op::AddAsset { asset } => {
                    if let Some(p) = self.doc.asset_path(&asset.id) {
                        if p.exists() {
                            self.assets.request(&asset.id, p, self.doc.project.sample_rate);
                        } else if let Some(s) = &self.session {
                            s.request_asset(&asset.id);
                        }
                    }
                }
                Op::SetProject { field: studio_core::ops::ProjectField::Bpm(b) } => {
                    self.engine.send(Cmd::Bpm(*b));
                }
                _ => {}
            }
            if !rebuild && is_light(op) {
                continue;
            }
            rebuild = true;
        }
        if rebuild {
            self.rebuild_graph();
        } else {
            for op in &tx.ops {
                if op.touches_audio() {
                    self.engine.send(Cmd::Light(op.clone()));
                }
            }
        }
        if let Some(sel) = self.selected {
            if self.doc.project.track(sel).is_none() {
                self.selected = self.doc.project.tracks.first().map(|t| t.id);
            }
        }
        if let Op::SetProject { field: studio_core::ops::ProjectField::LoopRegion(r) } = tx.ops.first().unwrap_or(&Op::RemoveMarker { id: 0 }) {
            let sr = self.doc.project.sample_rate as f64;
            self.engine.send(Cmd::Loop(r.map(|(a, b)| ((a * sr) as i64, (b * sr) as i64))));
        }
    }

    /// Applies a change made here (by the user or the AI) and shares it.
    pub fn commit(&mut self, tx: Transaction) -> Result<Id, String> {
        let id = tx.id;
        let applied = if self.session.is_some() {
            self.replica.local(&mut self.doc, tx).map_err(|e| e.to_string())?
        } else {
            self.doc.commit(tx).map_err(|e| e.to_string())?
        };
        self.after_change(&applied);
        if let Some(s) = &self.session {
            s.propose(applied);
        }
        Ok(id)
    }

    pub fn commit_ops(&mut self, label: &str, ops: Vec<Op>) {
        if let Err(e) = self.commit(Transaction::new(label, Actor::User, ops)) {
            self.toasts.push(format!("Impossible : {e}"));
        }
    }

    fn share_applied(&mut self, applied: Transaction) {
        self.after_change(&applied);
        if let Some(s) = &self.session {
            // the undo of a change is a new change for the others
            if let Some(inv) = self.doc.last_inverse.take() {
                self.replica.local_applied(applied.clone(), inv);
            }
            s.propose(applied);
        }
    }

    pub fn undo(&mut self) {
        match self.doc.undo() {
            Some(t) => {
                self.toasts.push(format!("Annulé : {}", t.label));
                self.share_applied(t);
            }
            None => self.toasts.push("Rien à annuler".into()),
        }
    }

    pub fn redo(&mut self) {
        if let Some(t) = self.doc.redo() {
            self.toasts.push(format!("Rétabli : {}", t.label));
            self.share_applied(t);
        }
    }

    /// Undoes one transaction (the assistant's "Annuler" buttons).
    pub fn undo_transaction(&mut self, id: Id) -> bool {
        match self.doc.undo_transaction(id) {
            Some(t) => {
                self.share_applied(t);
                for e in &mut self.ai_entries {
                    if e.actions.iter().any(|a| a.0 == id) {
                        e.undone.push(id);
                    }
                }
                true
            }
            None => false,
        }
    }

    // ------------------------------------------------------------------
    // Files

    /// Imports audio files: the first into an instrumental when there is
    /// none, the others as new tracks (or into `track` at `at`).
    pub fn import_files(&mut self, paths: &[PathBuf], track: Option<Id>, at: f64) {
        for p in paths {
            if !studio_dsp::io::is_audio_file(p) {
                self.toasts.push(format!("{} n'est pas un fichier audio", p.display()));
                continue;
            }
            match self.import_one(p, track, at) {
                Ok(name) => self.toasts.push(format!("« {name} » importé")),
                Err(e) => self.toasts.push(format!("Import impossible : {e}")),
            }
        }
    }

    fn store_audio(&mut self, path: &Path, kind: AssetKind) -> anyhow::Result<(AssetInfo, AudioBuffer)> {
        let buf = studio_dsp::io::load(path, self.doc.project.sample_rate)?;
        let (id, file) = self.doc.store_asset_file(path)?;
        let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let info = AssetInfo {
            id,
            name,
            file,
            kind,
            duration: buf.duration(),
            channels: buf.channel_count() as u16,
            sample_rate: buf.sample_rate,
            author: Some(self.settings.user_name.clone()),
        };
        Ok((info, buf))
    }

    fn import_one(&mut self, path: &Path, track: Option<Id>, at: f64) -> anyhow::Result<String> {
        let (info, buf) = self.store_audio(path, AssetKind::Import)?;
        let stem = path.file_stem().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let has_beat = self.doc.project.tracks.iter().any(|t| t.kind == TrackKind::Instrumental);
        let clip = Clip {
            id: new_id(),
            asset: info.id.clone(),
            name: stem.clone(),
            start: at.max(0.0),
            offset: 0.0,
            length: buf.duration(),
            gain_db: 0.0,
            fade_in: 0.0,
            fade_out: 0.0,
            muted: false,
        };
        self.assets.insert(&info.id, buf.clone());
        let mut ops = vec![Op::AddAsset { asset: info.clone() }];
        let is_beat;
        match track.filter(|t| self.doc.project.track(*t).is_some()) {
            Some(t) => {
                ops.push(Op::AddClip { track: t, clip });
                is_beat = false;
            }
            None => {
                let kind = if has_beat { TrackKind::Audio } else { TrackKind::Instrumental };
                is_beat = kind == TrackKind::Instrumental;
                let mut t = Track::new(if is_beat { "Instru" } else { &stem }, kind);
                if is_beat {
                    t.color = "#e8b25c".into();
                }
                t.clips.push(clip);
                let index = if is_beat { 0 } else { self.doc.project.tracks.len() };
                ops.push(Op::AddTrack { track: t, index });
            }
        }
        self.commit(Transaction::new(format!("Import de « {stem} »"), Actor::User, ops)).map_err(|e| anyhow::anyhow!(e))?;
        if let Some(s) = &self.session {
            if let Ok(bytes) = std::fs::read(path) {
                s.share_asset(&info.id, &info.file, Arc::new(bytes));
            }
        }
        if is_beat && self.doc.project.key.is_none() {
            // tempo and key of the beat, quietly in the background
            self.ask_ai_local("Trouve la tonalité et le tempo de l'instru");
        }
        Ok(stem)
    }

    pub fn add_reference(&mut self, path: &Path) {
        match self.store_audio(path, AssetKind::Reference) {
            Ok((info, buf)) => {
                self.assets.insert(&info.id, buf);
                let name = path.file_stem().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                let r = Reference { id: new_id(), asset: info.id.clone(), name: name.clone(), note: String::new() };
                self.commit_ops("Référence ajoutée", vec![Op::AddAsset { asset: info }, Op::AddReference { reference: r }]);
                self.toasts.push(format!("Référence « {name} » prête : demande « inspire-toi de la référence »"));
            }
            Err(e) => self.toasts.push(format!("Référence impossible : {e}")),
        }
    }

    pub fn new_track(&mut self, kind: TrackKind) -> Id {
        let n = self.doc.project.tracks.iter().filter(|t| t.kind == kind).count() + 1;
        let name = match kind {
            TrackKind::Vocal => format!("Voix {n}"),
            TrackKind::Adlib => "Adlibs".to_string(),
            TrackKind::Instrumental => "Instru".to_string(),
            TrackKind::Audio => format!("Audio {n}"),
        };
        let mut t = Track::new(&name, kind);
        if self.session.is_some() {
            t.owner = Some(self.settings.user_name.clone());
        }
        let id = t.id;
        let index = self.doc.project.tracks.len();
        self.commit_ops("Nouvelle piste", vec![Op::AddTrack { track: t, index }]);
        self.selected = Some(id);
        id
    }

    pub fn save(&mut self, dir: Option<PathBuf>) -> Result<PathBuf, String> {
        // plugin states go into their slots
        for (slot, state) in self.plugins.states() {
            for ch in self.doc.project.tracks.iter_mut().map(|t| &mut t.effects).chain(std::iter::once(&mut self.doc.project.master.effects)) {
                if let Some(s) = ch.iter_mut().find(|s| s.id == slot) {
                    s.plugin_state = Some(state.clone());
                }
            }
        }
        let r = match dir {
            Some(d) => self.doc.save_to(&d).map(|_| d),
            None => self.doc.save().map(|_| self.doc.dir.clone().unwrap_or_default()),
        };
        match r {
            Ok(d) => {
                self.settings.add_recent(&d.to_string_lossy());
                self.settings.save();
                Ok(d)
            }
            Err(e) => Err(e.to_string()),
        }
    }

    // ------------------------------------------------------------------
    // Transport and recording

    pub fn play_pause(&mut self) {
        if self.engine.is_playing() {
            if self.recording.is_some() {
                self.stop_recording();
                return;
            }
            self.engine.stop();
            self.share_transport(TransportMsg::Stop { pos: self.engine.position() });
        } else {
            self.engine.play();
            self.share_transport(TransportMsg::Play { pos: self.engine.position(), at_ms: 0.0 });
        }
    }

    #[allow(dead_code)]
    pub fn stop(&mut self) {
        if self.recording.is_some() {
            self.stop_recording();
        }
        self.engine.stop();
        self.engine.seek(0.0);
        self.share_transport(TransportMsg::Stop { pos: 0.0 });
    }

    pub fn seek(&mut self, t: f64) {
        self.engine.seek(t.max(0.0));
        if !self.engine.is_playing() {
            self.share_transport(TransportMsg::Seek { pos: t.max(0.0) });
        }
    }

    fn share_transport(&self, msg: TransportMsg) {
        if self.settings.sync_transport {
            if let Some(s) = &self.session {
                s.transport(msg);
            }
        }
    }

    pub fn toggle_record(&mut self) {
        if self.recording.is_some() {
            self.stop_recording();
            return;
        }
        let track = match self.selected.filter(|t| self.doc.project.track(*t).is_some_and(|t| t.kind != TrackKind::Instrumental)) {
            Some(t) => t,
            None => self.new_track(TrackKind::Vocal),
        };
        // a short pre-roll when starting at 0 helps singers
        self.rec_start = self.engine.position();
        match self.engine.start_recording(track) {
            Ok(()) => {
                self.recording = Some(track);
                self.rebuild_graph();
                if let Some(s) = &self.session {
                    s.set_status("recording");
                }
            }
            Err(e) => self.toasts.push(format!("Enregistrement impossible : {e}")),
        }
    }

    pub fn stop_recording(&mut self) {
        let Some(track) = self.recording.take() else { return };
        self.engine.monitor(None);
        if let Some(s) = &self.session {
            s.set_status("idle");
        }
        let Some(take) = self.engine.stop_recording() else {
            self.toasts.push("Prise vide (aucun son reçu du micro)".into());
            return;
        };
        let n = self.doc.project.assets.values().filter(|a| a.kind == AssetKind::Take).count() + 1;
        let track_name = self.doc.project.track(track).map(|t| t.name.clone()).unwrap_or_default();
        let name = format!("Prise {n} — {track_name}");
        let bytes = match studio_dsp::io::wav_bytes(&take.buffer, studio_dsp::io::WavFormat::Pcm24) {
            Ok(b) => b,
            Err(e) => {
                self.toasts.push(format!("Prise perdue : {e}"));
                return;
            }
        };
        let Ok((id, file)) = self.doc.store_asset_bytes(&bytes, "wav") else {
            self.toasts.push("Impossible d'écrire la prise".into());
            return;
        };
        let info = AssetInfo {
            id: id.clone(),
            name: name.clone(),
            file: file.clone(),
            kind: AssetKind::Take,
            duration: take.buffer.duration(),
            channels: 1,
            sample_rate: take.buffer.sample_rate,
            author: Some(self.settings.user_name.clone()),
        };
        let clip = Clip {
            id: new_id(),
            asset: id.clone(),
            name: name.clone(),
            start: take.start,
            offset: 0.0,
            length: take.buffer.duration(),
            gain_db: 0.0,
            fade_in: 0.005,
            fade_out: 0.02,
            muted: false,
        };
        self.assets.insert(&id, take.buffer.clone());
        self.commit_ops(&format!("Enregistrement « {name} »"), vec![Op::AddAsset { asset: info }, Op::AddClip { track, clip }]);
        if let Some(s) = &self.session {
            s.share_asset(&id, &file, Arc::new(bytes));
        }
        if self.settings.ai.proactive {
            aihost::spawn_take_note(take.buffer, track_name, self.ai_tx.clone());
        }
    }

    // ------------------------------------------------------------------
    // AI

    pub fn ask_ai(&mut self, text: &str) {
        let text = text.trim();
        if text.is_empty() || self.ai_busy.is_some() {
            return;
        }
        self.ai_entries.push(AiEntry { user: true, text: text.to_string(), actions: Vec::new(), undone: Vec::new(), note: None });
        self.run_ai(text, self.settings.ai.clone());
    }

    /// Runs a request with the local engineer (buttons of the panels).
    pub fn ask_ai_local(&mut self, text: &str) {
        if self.ai_busy.is_some() {
            return;
        }
        let mut cfg = self.settings.ai.clone();
        cfg.kind = ProviderKind::Local;
        self.ai_entries.push(AiEntry { user: true, text: text.to_string(), actions: Vec::new(), undone: Vec::new(), note: None });
        self.run_ai(text, cfg);
    }

    fn run_ai(&mut self, text: &str, cfg: studio_ai::ProviderConfig) {
        self.ai_busy = Some("Je réfléchis…".into());
        aihost::spawn_turn(
            cfg,
            self.ai_history.clone(),
            text.to_string(),
            self.selected,
            self.settings.plugins.iter().map(to_ai_desc).collect(),
            self.req_tx.clone(),
            self.ai_tx.clone(),
        );
        self.ai_history.push(ChatMessage { role: "user".into(), text: text.to_string() });
        self.ui_revision += 1;
    }

    /// Serves the agent's requests and collects its answers.
    fn pump_ai(&mut self) -> bool {
        let mut changed = false;
        while let Ok(r) = self.req_rx.try_recv() {
            match r {
                Request::Snapshot(tx) => {
                    let _ = tx.send((self.doc.project.clone(), self.assets.store.clone()));
                }
                Request::Commit(t, tx) => {
                    let _ = tx.send(self.commit(t));
                    changed = true;
                }
                Request::Layout(cmd, tx) => {
                    self.layout_requests.push(cmd);
                    let _ = tx.send(Ok("interface modifiée".into()));
                }
                Request::UndoAi(tx) => {
                    let r = self.doc.undo_last_by(&Actor::Ai).map(|t| {
                        let label = t.label.clone();
                        self.share_applied(t);
                        format!("annulé : {label}")
                    });
                    let _ = tx.send(r.ok_or_else(|| "aucune intervention de l'IA à annuler".to_string()));
                    changed = true;
                }
            }
        }
        while let Ok(m) = self.ai_rx.try_recv() {
            changed = true;
            match m {
                AiMsg::Progress(p) => self.ai_busy = Some(p),
                AiMsg::Done(r) => {
                    self.ai_busy = None;
                    let note = r.fallback_reason.map(|f| format!("Modèle indisponible ({f}) : l'ingénieur local a répondu."));
                    self.ai_history.push(ChatMessage { role: "assistant".into(), text: r.reply.clone() });
                    self.ai_entries.push(AiEntry { user: false, text: r.reply, actions: r.committed, undone: Vec::new(), note });
                }
                AiMsg::Note(text) => {
                    self.ai_entries.push(AiEntry { user: false, text, actions: Vec::new(), undone: Vec::new(), note: None });
                }
            }
        }
        changed
    }

    // ------------------------------------------------------------------
    // Collaboration

    pub fn host_session(&mut self) {
        match Session::host(&self.settings.user_name, 0, self.settings.try_upnp) {
            Ok(s) => {
                self.replica = Replica::new(0);
                self.peers = vec![PeerInfo { id: 1, name: self.settings.user_name.clone(), color: "#e86a5c".into(), status: "idle".into() }];
                self.toasts.push(if s.upnp {
                    "Session ouverte (accessible par Internet grâce à l'UPnP de ta box)".into()
                } else {
                    "Session ouverte : partage le code d'invitation".into()
                });
                self.session = Some(s);
            }
            Err(e) => self.toasts.push(format!("Impossible d'héberger : {e}")),
        }
        self.ui_revision += 1;
    }

    /// Joins in the background (connecting can take a few seconds).
    pub fn join_session(&mut self, code: &str) {
        if self.join_rx.is_some() {
            return;
        }
        let (tx, rx) = unbounded();
        let (code, name) = (code.to_string(), self.settings.user_name.clone());
        std::thread::spawn(move || {
            let _ = tx.send(Session::join(&code, &name).map_err(|e| e.to_string()));
        });
        self.join_rx = Some(rx);
        self.toasts.push("Connexion à la session…".into());
        self.ui_revision += 1;
    }

    pub fn leave_session(&mut self) {
        if let Some(s) = self.session.take() {
            s.leave();
        }
        self.peers.clear();
        self.ui_revision += 1;
    }

    pub fn send_chat(&mut self, text: &str) {
        if let Some(s) = &self.session {
            s.chat(text);
            self.chat.push((self.settings.user_name.clone(), text.to_string()));
            self.ui_revision += 1;
        }
    }

    fn pump_session(&mut self) -> bool {
        let Some(s) = &self.session else { return false };
        let events = s.poll();
        if events.is_empty() {
            return false;
        }
        for e in events {
            match e {
                Event::Welcome { project, seq, .. } => {
                    let mut doc = Document::new(*project);
                    let _ = doc.ensure_dir();
                    self.replica = Replica::new(seq);
                    self.set_document(doc);
                    self.toasts.push("Projet de la session reçu".into());
                }
                Event::Remote { seq, author, tx } => {
                    let pending = self.replica.pending() > 0;
                    let ok = self.replica.remote(&mut self.doc, seq, &tx);
                    if ok || pending {
                        if pending {
                            self.rebuild_graph();
                        } else {
                            self.after_change(&tx);
                        }
                    }
                    if author != 0 && tx.ops.iter().any(|o| matches!(o, Op::AddClip { .. })) {
                        let who = self.peers.iter().find(|p| p.id == author).map(|p| p.name.clone()).unwrap_or_default();
                        self.toasts.push(format!("{who} : {}", tx.label));
                    }
                }
                Event::Ack { seq, tx } => self.replica.ack(seq, tx),
                Event::Reset { project, seq } => {
                    let mut doc = Document::new(*project);
                    let _ = doc.ensure_dir();
                    self.replica = Replica::new(seq);
                    self.set_document(doc);
                }
                Event::Peers(p) => self.peers = p,
                Event::PeerJoined(p) => self.toasts.push(format!("{} a rejoint la session", p.name)),
                Event::PeerLeft { name, .. } => self.toasts.push(format!("{name} est parti")),
                Event::Chat { name, text, .. } => self.chat.push((name, text)),
                Event::Transport { msg, delay_ms, from } => {
                    if Some(from) == self.session.as_ref().map(|s| s.me) {
                        continue;
                    }
                    match msg {
                        TransportMsg::Play { pos, .. } => {
                            // start together: wait for the agreed moment, or
                            // catch up if it has passed
                            let late = (-delay_ms).max(0.0) / 1000.0;
                            self.engine.seek(pos + late);
                            let engine_cmd = self.engine_play_later(delay_ms.max(0.0));
                            if !engine_cmd {
                                self.engine.play();
                            }
                        }
                        TransportMsg::Stop { pos } => {
                            self.engine.stop();
                            self.engine.seek(pos);
                        }
                        TransportMsg::Seek { pos } => self.engine.seek(pos),
                    }
                }
                Event::WantsProject { peer } => {
                    if let Some(s) = &self.session {
                        s.welcome(peer, self.doc.project.clone(), self.replica.seq);
                    }
                }
                Event::WantsAsset { peer, id } => {
                    if let (Some(s), Some(path)) = (&self.session, self.doc.asset_path(&id)) {
                        if let Ok(bytes) = std::fs::read(&path) {
                            let file = self.doc.project.assets.get(&id).map(|a| a.file.clone()).unwrap_or_default();
                            s.send_asset_to(peer, &id, &file, Arc::new(bytes));
                        }
                    }
                }
                Event::Asset { id, name, bytes } => {
                    // name is the asset's file name
                    let ext = Path::new(&name).extension().and_then(|e| e.to_str()).unwrap_or("wav").to_string();
                    if let Ok(dir) = self.doc.ensure_dir() {
                        let file = self.doc.project.assets.get(&id).map(|a| a.file.clone()).unwrap_or_else(|| format!("{}.{ext}", &id[..id.len().min(24)]));
                        let path = dir.join("assets").join(&file);
                        if std::fs::write(&path, &*bytes).is_ok() {
                            self.assets.request(&id, path, self.doc.project.sample_rate);
                        }
                    }
                }
                Event::AssetProgress { .. } => {}
                Event::Disconnected { reason } => {
                    self.toasts.push(format!("Session terminée : {reason}"));
                    self.session = None;
                    self.peers.clear();
                }
                Event::Info(t) => self.toasts.push(t),
            }
        }
        self.ui_revision += 1;
        true
    }

    /// Schedules a synchronized start (true when scheduled).
    fn engine_play_later(&mut self, delay_ms: f64) -> bool {
        if delay_ms < 5.0 {
            return false;
        }
        PENDING_PLAY.with(|p| p.set(Some(awra::now_ms() + delay_ms)));
        true
    }

    // ------------------------------------------------------------------
    // Export

    pub fn export(&mut self, path: PathBuf, fmt: studio_dsp::io::WavFormat) {
        if self.export_busy {
            return;
        }
        self.export_busy = true;
        let project = self.doc.project.clone();
        let assets = self.assets.store.clone();
        let pm = self.plugins.clone();
        let (tx, rx) = unbounded();
        self.export_rx = Some(rx);
        std::thread::spawn(move || {
            let f = move |s: &EffectSlot, sr: f64, mb: usize| pm.create_detached(s, sr, mb);
            let buf = studio_dsp::graph::render_project(&project, &assets, Some(&f));
            let (l, r) = buf.lr();
            let lufs = studio_dsp::loudness::integrated(&[l, r], buf.sample_rate);
            let tp = studio_dsp::loudness::true_peak_db(&[l, r]);
            let res = match path.extension().and_then(|e| e.to_str()) {
                _ => studio_dsp::io::write_wav(&path, &buf, fmt),
            };
            let _ = tx.send(res.map(|_| (path, lufs, tp)).map_err(|e| e.to_string()));
        });
    }

    pub fn scan_plugins(&mut self) {
        if self.scan_busy {
            return;
        }
        self.scan_busy = true;
        let (tx, rx) = unbounded();
        self.scan_rx = Some(rx);
        let exe = std::env::current_exe().unwrap_or_default();
        std::thread::spawn(move || {
            let _ = tx.send(studio_plugins::scan_all(&exe));
        });
    }

    /// Everything that happens between frames. True when the views must
    /// refresh.
    pub fn tick(&mut self) -> bool {
        let mut changed = false;
        if self.assets.poll() {
            self.rebuild_graph();
            changed = true;
        }
        for e in std::mem::take(&mut self.assets.errors) {
            self.toasts.push(format!("Lecture audio impossible : {e}"));
        }
        changed |= self.pump_ai();
        if let Some(rx) = &self.join_rx {
            if let Ok(r) = rx.try_recv() {
                self.join_rx = None;
                match r {
                    Ok(s) => {
                        self.session = Some(s);
                        self.toasts.push("Connecté : réception du projet…".into());
                    }
                    Err(e) => self.toasts.push(format!("Connexion impossible : {e}")),
                }
                changed = true;
            }
        }
        changed |= self.pump_session();
        self.engine.pump_recording();
        self.engine.collect_garbage();
        self.plugins.idle();
        if let Some(due) = PENDING_PLAY.with(|p| p.get()) {
            if awra::now_ms() >= due {
                PENDING_PLAY.with(|p| p.set(None));
                self.engine.play();
            }
        }
        if let Some(rx) = &self.export_rx {
            if let Ok(r) = rx.try_recv() {
                self.export_busy = false;
                self.export_rx = None;
                match r {
                    Ok((p, lufs, tp)) => {
                        self.toasts.push(format!("Exporté : {} ({lufs:.1} LUFS, crête {tp:.1} dBTP)", p.display()));
                        self.last_export = Some((p, lufs, tp));
                    }
                    Err(e) => self.toasts.push(format!("Export impossible : {e}")),
                }
                changed = true;
            }
        }
        if let Some(rx) = &self.scan_rx {
            if let Ok(list) = rx.try_recv() {
                self.scan_busy = false;
                self.scan_rx = None;
                self.toasts.push(format!("{} plugin(s) trouvé(s)", list.len()));
                self.settings.plugins = list;
                self.settings.save();
                changed = true;
            }
        }
        // autosave a saved project every two minutes
        let now = awra::now_ms();
        if now - self.last_autosave > 120_000.0 {
            self.last_autosave = now;
            if self.doc.dirty && !self.doc.is_temporary() && self.recording.is_none() {
                let _ = self.save(None);
            }
        }
        if changed {
            self.ui_revision += 1;
        }
        changed
    }
}

thread_local! {
    static PENDING_PLAY: std::cell::Cell<Option<f64>> = const { std::cell::Cell::new(None) };
}

fn is_light(op: &Op) -> bool {
    use studio_core::ops::{ProjectField, TrackField};
    matches!(
        op,
        Op::SetParam { .. }
            | Op::SetEffectEnabled { .. }
            | Op::SetTrack { field: TrackField::VolumeDb(_) | TrackField::Pan(_) | TrackField::Mute(_) | TrackField::Solo(_), .. }
            | Op::SetProject { field: ProjectField::MasterVolumeDb(_) | ProjectField::Bpm(_) }
    ) || !op.touches_audio()
}

pub fn to_ai_desc(p: &studio_plugins::PluginDesc) -> PluginDesc {
    PluginDesc {
        name: p.name.clone(),
        vendor: p.vendor.clone(),
        format: p.format,
        path: p.path.clone(),
        id: p.id.clone(),
        category: p.category.clone(),
    }
}
