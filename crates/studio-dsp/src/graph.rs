//! The mix graph: clips → track chains → volume/pan → master chain.
//!
//! The same graph renders in real time (the engine) and offline (export,
//! AI analysis of the mix). It is built from a project and the decoded
//! assets; processors of effect slots that already exist can be taken
//! over from a previous graph (`adopt`) so that a rebuild keeps reverb
//! tails and plugin instances.

use std::collections::HashMap;
use std::sync::Arc;

use studio_core::model::*;
use studio_core::ops::{Op, ProjectField, Target, TrackField};

use crate::buffer::AudioBuffer;
use crate::effects::{create, Processor};
use crate::filters::Smoothed;

/// Decoded assets at the project rate.
pub type AssetStore = HashMap<AssetId, Arc<AudioBuffer>>;

/// Makes processors for effect kinds the DSP doesn't know (plugins).
pub type ProcessorFactory = dyn Fn(&EffectSlot, f64, usize) -> Option<Box<dyn Processor>>;

pub struct SlotNode {
    pub id: Id,
    pub enabled: bool,
    pub proc: Option<Box<dyn Processor>>,
}

pub struct ClipNode {
    pub buffer: Arc<AudioBuffer>,
    /// Timeline start, asset offset and length in frames.
    pub start: i64,
    pub offset: i64,
    pub len: i64,
    pub gain: f32,
    pub fade_in: i64,
    pub fade_out: i64,
}

pub struct TrackNode {
    pub id: Id,
    pub clips: Vec<ClipNode>,
    pub chain: Vec<SlotNode>,
    pub volume: Smoothed,
    pub pan: Smoothed,
    pub mute: bool,
    pub solo: bool,
    /// Live input monitored through this track (recording).
    pub monitor: bool,
}

/// Levels after a block, for meters.
#[derive(Clone, Debug, Default)]
pub struct Meters {
    /// (track id, peak L, peak R)
    pub tracks: Vec<(Id, f32, f32)>,
    pub master: (f32, f32),
    /// Gain reduction per effect slot (dB).
    pub reductions: Vec<(Id, f32)>,
}

pub struct MixGraph {
    pub sr: f64,
    pub bpm: f64,
    pub tracks: Vec<TrackNode>,
    pub master: Vec<SlotNode>,
    pub master_volume: Smoothed,
    scratch_l: Vec<f32>,
    scratch_r: Vec<f32>,
    pub meters: Meters,
    /// False while stopped: only live input (monitoring) and tails play.
    pub play_clips: bool,
    /// Every slot's parameters, pushed into adopted processors.
    params: Vec<(Id, Vec<(String, f64)>)>,
}

pub const MAX_BLOCK: usize = 4096;

fn pan_gains(pan: f64) -> (f32, f32) {
    // constant power, 0 dB at the center for each side's own content
    let p = (pan.clamp(-1.0, 1.0) + 1.0) * std::f64::consts::FRAC_PI_4;
    ((p.cos() * std::f64::consts::SQRT_2) as f32, (p.sin() * std::f64::consts::SQRT_2) as f32)
}

impl MixGraph {
    /// Builds a graph. With `fresh` false, slots get no processor and must
    /// be adopted from a previous graph (or created later with
    /// [`MixGraph::create_missing`]).
    pub fn build(project: &Project, assets: &AssetStore, factory: Option<&ProcessorFactory>, fresh: bool) -> MixGraph {
        let sr = project.sample_rate as f64;
        let to_frames = |s: f64| (s * sr).round() as i64;
        let make_chain = |chain: &[EffectSlot]| -> Vec<SlotNode> {
            chain
                .iter()
                .map(|s| SlotNode {
                    id: s.id,
                    enabled: s.enabled,
                    proc: if fresh { make_proc(s, sr, project.bpm, factory) } else { None },
                })
                .collect()
        };
        let tracks = project
            .tracks
            .iter()
            .map(|t| TrackNode {
                id: t.id,
                clips: t
                    .clips
                    .iter()
                    .filter(|c| !c.muted)
                    .filter_map(|c| {
                        let buffer = assets.get(&c.asset)?.clone();
                        Some(ClipNode {
                            buffer,
                            start: to_frames(c.start),
                            offset: to_frames(c.offset),
                            len: to_frames(c.length),
                            gain: db_to_gain(c.gain_db) as f32,
                            fade_in: to_frames(c.fade_in),
                            fade_out: to_frames(c.fade_out),
                        })
                    })
                    .collect(),
                chain: make_chain(&t.effects),
                volume: Smoothed::new(db_to_gain(t.volume_db), 480),
                pan: Smoothed::new(t.pan, 480),
                mute: t.mute,
                solo: t.solo,
                monitor: false,
            })
            .collect();
        MixGraph {
            sr,
            bpm: project.bpm,
            tracks,
            master: make_chain(&project.master.effects),
            master_volume: Smoothed::new(db_to_gain(project.master.volume_db), 480),
            scratch_l: vec![0.0; MAX_BLOCK],
            scratch_r: vec![0.0; MAX_BLOCK],
            meters: Meters::default(),
            play_clips: true,
            params: project
                .tracks
                .iter()
                .flat_map(|t| t.effects.iter())
                .chain(project.master.effects.iter())
                .map(|s| (s.id, s.params.iter().map(|(k, v)| (k.clone(), *v)).collect()))
                .collect(),
        }
    }

    /// The slot ids that have a processor.
    pub fn slot_ids(&self) -> Vec<Id> {
        self.tracks
            .iter()
            .flat_map(|t| t.chain.iter())
            .chain(self.master.iter())
            .filter(|s| s.proc.is_some())
            .map(|s| s.id)
            .collect()
    }

    /// Takes the processors of `old` for slots that have none (no
    /// allocation: safe on the audio thread), and keeps levels gliding.
    pub fn adopt(&mut self, old: &mut MixGraph) {
        let mut take = |id: Id| -> Option<Box<dyn Processor>> {
            for t in old.tracks.iter_mut() {
                for s in t.chain.iter_mut() {
                    if s.id == id {
                        return s.proc.take();
                    }
                }
            }
            for s in old.master.iter_mut() {
                if s.id == id {
                    return s.proc.take();
                }
            }
            None
        };
        for t in self.tracks.iter_mut() {
            for s in t.chain.iter_mut() {
                if s.proc.is_none() {
                    s.proc = take(s.id);
                }
            }
        }
        for s in self.master.iter_mut() {
            if s.proc.is_none() {
                s.proc = take(s.id);
            }
        }
        for t in self.tracks.iter_mut() {
            if let Some(o) = old.tracks.iter().find(|o| o.id == t.id) {
                let (v, p) = (t.volume.target, t.pan.target);
                t.volume = o.volume;
                t.volume.set(v);
                t.pan = o.pan;
                t.pan.set(p);
                t.monitor = o.monitor;
            }
        }
        let mv = self.master_volume.target;
        self.master_volume = old.master_volume;
        self.master_volume.set(mv);
        self.play_clips = old.play_clips;
        // adopted processors take the new graph's values (presets, variants)
        let params = std::mem::take(&mut self.params);
        for (id, ps) in &params {
            let node = self
                .tracks
                .iter_mut()
                .flat_map(|t| t.chain.iter_mut())
                .chain(self.master.iter_mut())
                .find(|n| n.id == *id);
            if let Some(p) = node.and_then(|n| n.proc.as_mut()) {
                p.set_bpm(self.bpm);
                for (k, v) in ps {
                    p.set_param(k, *v);
                }
            }
        }
        self.params = params;
    }

    /// Creates processors for the slots whose ids are not in `existing`
    /// (on the UI thread, before sending the graph to the engine).
    pub fn create_new(&mut self, project: &Project, existing: &[Id], factory: Option<&ProcessorFactory>) {
        let sr = self.sr;
        let bpm = project.bpm;
        let fill = |nodes: &mut Vec<SlotNode>, slots: &[EffectSlot]| {
            for (n, s) in nodes.iter_mut().zip(slots) {
                if n.proc.is_none() && !existing.contains(&s.id) {
                    n.proc = make_proc(s, sr, bpm, factory);
                }
            }
        };
        for (t, pt) in self.tracks.iter_mut().zip(project.tracks.iter()) {
            fill(&mut t.chain, &pt.effects);
        }
        fill(&mut self.master, &project.master.effects);
    }

    pub fn all_slot_ids(project: &Project) -> Vec<Id> {
        project.tracks.iter().flat_map(|t| t.effects.iter()).chain(project.master.effects.iter()).map(|s| s.id).collect()
    }

    /// Creates processors for slots that still have none (new slots), and
    /// pushes every parameter of the project into them.
    pub fn create_missing(&mut self, project: &Project, factory: Option<&ProcessorFactory>) {
        let sr = self.sr;
        let bpm = project.bpm;
        let fill = |nodes: &mut Vec<SlotNode>, slots: &[EffectSlot]| {
            for (n, s) in nodes.iter_mut().zip(slots) {
                if n.proc.is_none() {
                    n.proc = make_proc(s, sr, bpm, factory);
                }
            }
        };
        for (t, pt) in self.tracks.iter_mut().zip(project.tracks.iter()) {
            fill(&mut t.chain, &pt.effects);
        }
        fill(&mut self.master, &project.master.effects);
    }

    /// Sends every parameter value of the project to the processors.
    pub fn sync_params(&mut self, project: &Project) {
        let push = |nodes: &mut Vec<SlotNode>, slots: &[EffectSlot]| {
            for (n, s) in nodes.iter_mut().zip(slots) {
                n.enabled = s.enabled;
                if let Some(p) = n.proc.as_mut() {
                    p.set_bpm(project.bpm);
                    for (k, v) in &s.params {
                        p.set_param(k, *v);
                    }
                }
            }
        };
        for (t, pt) in self.tracks.iter_mut().zip(project.tracks.iter()) {
            push(&mut t.chain, &pt.effects);
        }
        push(&mut self.master, &project.master.effects);
    }

    /// Applies an op that doesn't change the graph's shape. Returns false
    /// when the graph must be rebuilt.
    pub fn apply_light(&mut self, op: &Op) -> bool {
        match op {
            Op::SetParam { target, slot, param, value } => {
                if let Some(n) = self.slot_mut(*target, *slot) {
                    if let Some(p) = n.proc.as_mut() {
                        p.set_param(param, *value);
                    }
                    return true;
                }
                false
            }
            Op::SetEffectEnabled { target, slot, enabled } => {
                if let Some(n) = self.slot_mut(*target, *slot) {
                    n.enabled = *enabled;
                    return true;
                }
                false
            }
            Op::SetTrack { id, field } => {
                let Some(t) = self.tracks.iter_mut().find(|t| t.id == *id) else { return false };
                match field {
                    TrackField::VolumeDb(v) => t.volume.set(db_to_gain(*v)),
                    TrackField::Pan(v) => t.pan.set(*v),
                    TrackField::Mute(v) => t.mute = *v,
                    TrackField::Solo(v) => t.solo = *v,
                    TrackField::Name(_) | TrackField::Color(_) | TrackField::Owner(_) | TrackField::Kind(_) => {}
                    TrackField::Armed(_) => {}
                }
                true
            }
            Op::SetProject { field: ProjectField::MasterVolumeDb(v) } => {
                self.master_volume.set(db_to_gain(*v));
                true
            }
            Op::SetProject { field: ProjectField::Bpm(b) } => {
                self.bpm = *b;
                for n in self.tracks.iter_mut().flat_map(|t| t.chain.iter_mut()).chain(self.master.iter_mut()) {
                    if let Some(p) = n.proc.as_mut() {
                        p.set_bpm(*b);
                    }
                }
                true
            }
            op => !op.touches_audio(),
        }
    }

    fn slot_mut(&mut self, target: Target, slot: Id) -> Option<&mut SlotNode> {
        match target {
            Target::Master => self.master.iter_mut().find(|s| s.id == slot),
            Target::Track(id) => self.tracks.iter_mut().find(|t| t.id == id)?.chain.iter_mut().find(|s| s.id == slot),
        }
    }

    pub fn reset(&mut self) {
        for n in self.tracks.iter_mut().flat_map(|t| t.chain.iter_mut()).chain(self.master.iter_mut()) {
            if let Some(p) = n.proc.as_mut() {
                p.reset();
            }
        }
    }

    /// Renders `out_l.len()` frames starting at timeline frame `pos`
    /// (additively into out, which the caller clears). `input` is the live
    /// input (mono) for monitored tracks.
    pub fn render(&mut self, pos: i64, out_l: &mut [f32], out_r: &mut [f32], input: Option<&[f32]>) {
        let n = out_l.len().min(MAX_BLOCK);
        let any_solo = self.tracks.iter().any(|t| t.solo);
        self.meters.tracks.clear();
        self.meters.reductions.clear();
        for t in self.tracks.iter_mut() {
            let l = &mut self.scratch_l[..n];
            let r = &mut self.scratch_r[..n];
            l.fill(0.0);
            r.fill(0.0);
            let audible = !t.mute && (!any_solo || t.solo);
            let mut has_signal = false;
            if audible && self.play_clips {
                for c in &t.clips {
                    has_signal |= mix_clip(c, pos, l, r);
                }
            }
            if audible {
                if t.monitor {
                    if let Some(inp) = input {
                        for i in 0..n.min(inp.len()) {
                            l[i] += inp[i];
                            r[i] += inp[i];
                        }
                        has_signal = true;
                    }
                }
            }
            let _ = has_signal;
            // the chain runs even on silence so that tails ring out
            if audible {
                for s in t.chain.iter_mut() {
                    if s.enabled {
                        if let Some(p) = s.proc.as_mut() {
                            p.process(l, r);
                            let gr = p.reduction_db();
                            if gr > 0.0 {
                                self.meters.reductions.push((s.id, gr));
                            }
                        }
                    }
                }
            }
            let mut pk = (0.0f32, 0.0f32);
            for i in 0..n {
                let v = t.volume.next() as f32;
                let (gl, gr) = pan_gains(t.pan.next());
                let a = l[i] * v * gl;
                let b = r[i] * v * gr;
                pk.0 = pk.0.max(a.abs());
                pk.1 = pk.1.max(b.abs());
                if audible {
                    out_l[i] += a;
                    out_r[i] += b;
                }
            }
            self.meters.tracks.push((t.id, pk.0, pk.1));
        }
        for s in self.master.iter_mut() {
            if s.enabled {
                if let Some(p) = s.proc.as_mut() {
                    p.process(&mut out_l[..n], &mut out_r[..n]);
                    let gr = p.reduction_db();
                    if gr > 0.0 {
                        self.meters.reductions.push((s.id, gr));
                    }
                }
            }
        }
        let mut pk = (0.0f32, 0.0f32);
        for i in 0..n {
            let v = self.master_volume.next() as f32;
            out_l[i] *= v;
            out_r[i] *= v;
            pk.0 = pk.0.max(out_l[i].abs());
            pk.1 = pk.1.max(out_r[i].abs());
        }
        self.meters.master = pk;
    }

    /// The monitored track for live input (None: none).
    pub fn set_monitor(&mut self, track: Option<Id>) {
        for t in self.tracks.iter_mut() {
            t.monitor = Some(t.id) == track;
        }
    }
}

fn make_proc(s: &EffectSlot, sr: f64, bpm: f64, factory: Option<&ProcessorFactory>) -> Option<Box<dyn Processor>> {
    create(&s.kind, sr, &s.params, bpm).or_else(|| factory.and_then(|f| f(s, sr, MAX_BLOCK)))
}

/// Adds a clip's part of the block; true if it overlapped.
fn mix_clip(c: &ClipNode, pos: i64, l: &mut [f32], r: &mut [f32]) -> bool {
    let n = l.len() as i64;
    let a = c.start.max(pos);
    let b = (c.start + c.len).min(pos + n);
    if a >= b {
        return false;
    }
    let (sl, sr) = c.buffer.lr();
    let frames = sl.len() as i64;
    for t in a..b {
        let rel = t - c.start;
        let src = c.offset + rel;
        if src < 0 || src >= frames {
            continue;
        }
        let mut g = c.gain;
        if c.fade_in > 0 && rel < c.fade_in {
            g *= rel as f32 / c.fade_in as f32;
        }
        let to_end = c.len - rel;
        if c.fade_out > 0 && to_end < c.fade_out {
            g *= to_end as f32 / c.fade_out as f32;
        }
        let i = (t - pos) as usize;
        l[i] += sl[src as usize] * g;
        r[i] += sr[src as usize] * g;
    }
    true
}

/// Renders a range of the project offline (fresh processors).
pub fn render_range(
    project: &Project,
    assets: &AssetStore,
    factory: Option<&ProcessorFactory>,
    start: f64,
    end: f64,
) -> AudioBuffer {
    let mut g = MixGraph::build(project, assets, factory, true);
    let sr = project.sample_rate;
    let a = (start * sr as f64) as i64;
    let total = (((end - start) * sr as f64).max(0.0)) as usize;
    let mut l = vec![0.0f32; total];
    let mut r = vec![0.0f32; total];
    let block = 1024;
    let mut i = 0;
    while i < total {
        let n = block.min(total - i);
        g.render(a + i as i64, &mut l[i..i + n], &mut r[i..i + n], None);
        i += n;
    }
    AudioBuffer::from_stereo(sr, l, r)
}

/// Renders the whole song plus a tail for reverbs and delays.
pub fn render_project(project: &Project, assets: &AssetStore, factory: Option<&ProcessorFactory>) -> AudioBuffer {
    let end = project.duration() + 2.0;
    render_range(project, assets, factory, 0.0, end)
}

/// Renders a single track through its chain (no volume/pan, no master):
/// what the AI listens to when it analyzes a processed track.
pub fn render_track(
    project: &Project,
    assets: &AssetStore,
    track: Id,
    with_effects: bool,
    factory: Option<&ProcessorFactory>,
) -> Option<AudioBuffer> {
    let t = project.track(track)?;
    let mut solo = project.clone();
    let mut t2 = t.clone();
    t2.volume_db = 0.0;
    t2.pan = 0.0;
    t2.mute = false;
    t2.solo = false;
    if !with_effects {
        t2.effects.clear();
    }
    solo.tracks = vec![t2];
    solo.master = Master::default();
    let start = t.clips.iter().map(|c| c.start).fold(f64::MAX, f64::min);
    if start == f64::MAX {
        return None;
    }
    Some(render_range(&solo, assets, factory, start, t.end() + if with_effects { 1.0 } else { 0.0 }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use studio_core::effects::EffectKind;

    fn project_with_tone() -> (Project, AssetStore) {
        let mut p = Project::new("t");
        let sr = p.sample_rate;
        let tone: Vec<f32> = (0..sr).map(|i| (i as f32 * 0.05).sin() * 0.5).collect();
        let mut assets = AssetStore::new();
        assets.insert("a".into(), Arc::new(AudioBuffer::from_mono(sr, tone)));
        let mut t = Track::new("Voix", TrackKind::Vocal);
        t.clips.push(Clip {
            id: new_id(),
            asset: "a".into(),
            name: "c".into(),
            start: 0.5,
            offset: 0.0,
            length: 1.0,
            gain_db: 0.0,
            fade_in: 0.0,
            fade_out: 0.0,
            muted: false,
        });
        p.tracks.push(t);
        (p, assets)
    }

    #[test]
    fn clip_lands_at_its_start() {
        let (p, a) = project_with_tone();
        let out = render_range(&p, &a, None, 0.0, 2.0);
        let sr = p.sample_rate as usize;
        assert!(out.channels[0][..sr / 2 - 10].iter().all(|s| *s == 0.0));
        let pk = out.channels[0][sr / 2..sr].iter().fold(0.0f32, |m, s| m.max(s.abs()));
        assert!((pk - 0.5).abs() < 0.02, "{pk}");
    }

    #[test]
    fn mute_and_solo() {
        let (mut p, a) = project_with_tone();
        p.tracks[0].mute = true;
        assert_eq!(render_range(&p, &a, None, 0.0, 2.0).peak(), 0.0);
        p.tracks[0].mute = false;
        let mut t2 = Track::new("Autre", TrackKind::Audio);
        t2.solo = true;
        p.tracks.push(t2);
        assert_eq!(render_range(&p, &a, None, 0.0, 2.0).peak(), 0.0);
    }

    #[test]
    fn adopt_keeps_processors() {
        let (mut p, a) = project_with_tone();
        p.master.effects.push(EffectSlot::new(EffectKind::Reverb));
        let mut g1 = MixGraph::build(&p, &a, None, true);
        let mut g2 = MixGraph::build(&p, &a, None, false);
        assert!(g2.master[0].proc.is_none());
        g2.adopt(&mut g1);
        assert!(g2.master[0].proc.is_some());
        assert!(g1.master[0].proc.is_none());
    }
}
