//! The agent runs on a worker thread; what it does to the studio goes
//! through requests answered on the UI thread. Listening (rendering and
//! analysis) happens on the worker, from snapshots.

use crossbeam_channel::{bounded, Receiver, Sender};
use studio_ai::host::{LayoutCommand, PluginDesc, StudioHost};
use studio_ai::{ChatMessage, ProviderConfig, TurnResult};
use studio_analysis::Analysis;
use studio_core::model::*;
use studio_core::ops::Transaction;
use studio_dsp::graph::{render_project, render_track, AssetStore};

pub enum Request {
    Snapshot(Sender<(Project, AssetStore)>),
    Commit(Transaction, Sender<Result<Id, String>>),
    Layout(LayoutCommand, Sender<Result<String, String>>),
    UndoAi(Sender<Result<String, String>>),
}

pub enum AiMsg {
    Progress(String),
    Done(TurnResult),
    /// A spontaneous remark (after a recording).
    Note(String),
}

pub struct ProxyHost {
    pub req: Sender<Request>,
    pub plugins: Vec<PluginDesc>,
    cache: Option<(Project, AssetStore)>,
}

impl ProxyHost {
    pub fn new(req: Sender<Request>, plugins: Vec<PluginDesc>) -> ProxyHost {
        ProxyHost { req, plugins, cache: None }
    }

    fn snapshot(&mut self) -> Result<(Project, AssetStore), String> {
        let (tx, rx) = bounded(1);
        self.req.send(Request::Snapshot(tx)).map_err(|_| "studio fermé".to_string())?;
        let s = rx.recv().map_err(|_| "studio fermé".to_string())?;
        self.cache = Some(s.clone());
        Ok(s)
    }

    fn ask<T>(&self, make: impl FnOnce(Sender<Result<T, String>>) -> Request) -> Result<T, String> {
        let (tx, rx) = bounded(1);
        self.req.send(make(tx)).map_err(|_| "studio fermé".to_string())?;
        rx.recv().map_err(|_| "studio fermé".to_string())?
    }
}

fn is_vocal(p: &Project, id: Id) -> bool {
    p.track(id).is_some_and(|t| t.kind != TrackKind::Instrumental)
}

impl StudioHost for ProxyHost {
    fn project(&self) -> Project {
        let (tx, rx) = bounded(1);
        if self.req.send(Request::Snapshot(tx)).is_ok() {
            if let Ok((p, _)) = rx.recv() {
                return p;
            }
        }
        self.cache.as_ref().map(|c| c.0.clone()).unwrap_or_default()
    }

    fn analyze_track(&mut self, track: Id, processed: bool) -> Result<Analysis, String> {
        let (p, assets) = self.snapshot()?;
        let buf = render_track(&p, &assets, track, processed, None).ok_or("cette piste est vide")?;
        Ok(studio_analysis::analyze(&buf, is_vocal(&p, track)))
    }

    fn analyze_mix(&mut self) -> Result<Analysis, String> {
        let (p, assets) = self.snapshot()?;
        if p.duration() <= 0.0 {
            return Err("le projet est vide".into());
        }
        Ok(studio_analysis::analyze(&render_project(&p, &assets, None), false))
    }

    fn analyze_reference(&mut self, reference: Id) -> Result<Analysis, String> {
        let (p, assets) = self.snapshot()?;
        let r = p.references.iter().find(|r| r.id == reference).ok_or("référence introuvable")?;
        let buf = assets.get(&r.asset).ok_or("la référence est encore en chargement")?;
        // the whole song is long: a minute from its middle tells enough
        let d = buf.duration();
        let b = if d > 70.0 { buf.slice(d / 2.0 - 30.0, 60.0) } else { (**buf).clone() };
        Ok(studio_analysis::analyze(&b, true))
    }

    fn measure_lufs(&mut self, project: &Project) -> Result<f64, String> {
        let assets = match &self.cache {
            Some(c) => c.1.clone(),
            None => self.snapshot()?.1,
        };
        let b = render_project(project, &assets, None);
        let (l, r) = b.lr();
        Ok(studio_dsp::loudness::integrated(&[l, r], b.sample_rate))
    }

    fn commit(&mut self, tx: Transaction) -> Result<Id, String> {
        self.ask(|s| Request::Commit(tx, s))
    }

    fn layout(&mut self, cmd: LayoutCommand) -> Result<String, String> {
        self.ask(|s| Request::Layout(cmd, s))
    }

    fn plugins(&self) -> Vec<PluginDesc> {
        self.plugins.clone()
    }

    fn undo_ai(&mut self) -> Result<String, String> {
        self.ask(Request::UndoAi)
    }
}

/// Runs one turn of the agent on a worker thread.
pub fn spawn_turn(
    cfg: ProviderConfig,
    history: Vec<ChatMessage>,
    request: String,
    selected: Option<Id>,
    plugins: Vec<PluginDesc>,
    req: Sender<Request>,
    out: Sender<AiMsg>,
) {
    std::thread::Builder::new()
        .name("awra-studio-ai".into())
        .spawn(move || {
            let mut host = ProxyHost::new(req, plugins);
            let o2 = out.clone();
            let mut progress = move |s: &str| {
                let _ = o2.send(AiMsg::Progress(s.to_string()));
            };
            let r = studio_ai::run_turn(&cfg, &history, &request, &mut host, selected, &mut progress);
            let _ = out.send(AiMsg::Done(r));
        })
        .expect("ai thread");
}

/// Listens to a fresh take and makes a remark.
pub fn spawn_take_note(take: studio_dsp::AudioBuffer, track_name: String, out: Sender<AiMsg>) {
    std::thread::spawn(move || {
        let a = studio_analysis::analyze(&take, true);
        let _ = out.send(AiMsg::Note(studio_ai::agent::after_recording_note(&track_name, &a)));
    });
}

pub type AiChannels = (Sender<Request>, Receiver<Request>, Sender<AiMsg>, Receiver<AiMsg>);
