//! Operations: the only way the project changes.
//!
//! [`Op::apply`] mutates a project and returns the inverse operation, so a
//! [`Transaction`] can be undone by applying its inverses in reverse order.
//! Transactions are what the AI produces and what collaborators exchange.

use serde::{Deserialize, Serialize};

use crate::model::*;

/// What an effect chain belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type", content = "id")]
pub enum Target {
    Track(Id),
    Master,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "field", content = "value")]
pub enum TrackField {
    Name(String),
    Kind(TrackKind),
    Color(String),
    VolumeDb(f64),
    Pan(f64),
    Mute(bool),
    Solo(bool),
    Armed(bool),
    Owner(Option<String>),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "field", content = "value")]
pub enum ClipField {
    Name(String),
    Start(f64),
    Offset(f64),
    Length(f64),
    GainDb(f64),
    FadeIn(f64),
    FadeOut(f64),
    Muted(bool),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "field", content = "value")]
pub enum ProjectField {
    Name(String),
    Bpm(f64),
    Key(Option<u8>),
    Minor(bool),
    MasterVolumeDb(f64),
    LoopRegion(Option<(f64, f64)>),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "op")]
pub enum Op {
    SetProject { field: ProjectField },
    AddAsset { asset: AssetInfo },
    RemoveAsset { id: AssetId },

    AddTrack { track: Track, index: usize },
    RemoveTrack { id: Id },
    MoveTrack { id: Id, index: usize },
    SetTrack { id: Id, field: TrackField },

    AddClip { track: Id, clip: Clip },
    RemoveClip { track: Id, clip: Id },
    SetClip { track: Id, clip: Id, field: ClipField },

    AddEffect { target: Target, slot: EffectSlot, index: usize },
    RemoveEffect { target: Target, slot: Id },
    MoveEffect { target: Target, slot: Id, index: usize },
    SetParam { target: Target, slot: Id, param: String, value: f64 },
    SetEffectEnabled { target: Target, slot: Id, enabled: bool },
    SetPluginState { target: Target, slot: Id, state: Option<String> },
    /// Replaces a whole chain (presets, AI chains).
    SetChain { target: Target, chain: Vec<EffectSlot> },
    /// Applies a saved mix to every track it names, and the master.
    ApplyMix { mix: MixSnapshot },

    AddSection { section: LyricSection, index: usize },
    RemoveSection { id: Id },
    MoveSection { id: Id, index: usize },
    SetSection { id: Id, title: String, text: String, start: Option<f64> },

    AddMarker { marker: Marker },
    RemoveMarker { id: Id },

    AddReference { reference: Reference },
    RemoveReference { id: Id },

    AddVariant { variant: MixVariant },
    RemoveVariant { id: Id },
}

#[derive(Debug, Clone, PartialEq)]
pub struct OpError(pub String);

impl std::fmt::Display for OpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for OpError {}

fn err<T>(msg: impl Into<String>) -> Result<T, OpError> {
    Err(OpError(msg.into()))
}

fn chain_mut(p: &mut Project, target: Target) -> Result<&mut Vec<EffectSlot>, OpError> {
    match target {
        Target::Master => Ok(&mut p.master.effects),
        Target::Track(id) => match p.track_mut(id) {
            Some(t) => Ok(&mut t.effects),
            None => err(format!("piste {id} introuvable")),
        },
    }
}

pub fn chain(p: &Project, target: Target) -> Option<&Vec<EffectSlot>> {
    match target {
        Target::Master => Some(&p.master.effects),
        Target::Track(id) => p.track(id).map(|t| &t.effects),
    }
}

impl Op {
    /// Applies the operation, returning its inverse.
    pub fn apply(&self, p: &mut Project) -> Result<Op, OpError> {
        Ok(match self.clone() {
            Op::SetProject { field } => {
                let old = match field {
                    ProjectField::Name(v) => ProjectField::Name(std::mem::replace(&mut p.name, v)),
                    ProjectField::Bpm(v) => ProjectField::Bpm(std::mem::replace(&mut p.bpm, v.clamp(20.0, 400.0))),
                    ProjectField::Key(v) => ProjectField::Key(std::mem::replace(&mut p.key, v.map(|k| k % 12))),
                    ProjectField::Minor(v) => ProjectField::Minor(std::mem::replace(&mut p.minor, v)),
                    ProjectField::MasterVolumeDb(v) => {
                        ProjectField::MasterVolumeDb(std::mem::replace(&mut p.master.volume_db, v))
                    }
                    ProjectField::LoopRegion(v) => ProjectField::LoopRegion(std::mem::replace(&mut p.loop_region, v)),
                };
                Op::SetProject { field: old }
            }
            Op::AddAsset { asset } => {
                let id = asset.id.clone();
                match p.assets.insert(id.clone(), asset) {
                    // re-adding the same asset (two peers importing one file) is harmless
                    Some(old) => Op::AddAsset { asset: old },
                    None => Op::RemoveAsset { id },
                }
            }
            Op::RemoveAsset { id } => match p.assets.remove(&id) {
                Some(asset) => Op::AddAsset { asset },
                None => return err("asset introuvable"),
            },
            Op::AddTrack { track, index } => {
                if p.track(track.id).is_some() {
                    return err("piste déjà présente");
                }
                let id = track.id;
                let index = index.min(p.tracks.len());
                p.tracks.insert(index, track);
                Op::RemoveTrack { id }
            }
            Op::RemoveTrack { id } => {
                let Some(index) = p.track_index(id) else { return err("piste introuvable") };
                let track = p.tracks.remove(index);
                Op::AddTrack { track, index }
            }
            Op::MoveTrack { id, index } => {
                let Some(from) = p.track_index(id) else { return err("piste introuvable") };
                let t = p.tracks.remove(from);
                p.tracks.insert(index.min(p.tracks.len()), t);
                Op::MoveTrack { id, index: from }
            }
            Op::SetTrack { id, field } => {
                let Some(t) = p.track_mut(id) else { return err("piste introuvable") };
                let old = match field {
                    TrackField::Name(v) => TrackField::Name(std::mem::replace(&mut t.name, v)),
                    TrackField::Kind(v) => TrackField::Kind(std::mem::replace(&mut t.kind, v)),
                    TrackField::Color(v) => TrackField::Color(std::mem::replace(&mut t.color, v)),
                    TrackField::VolumeDb(v) => {
                        TrackField::VolumeDb(std::mem::replace(&mut t.volume_db, v.clamp(-96.0, 12.0)))
                    }
                    TrackField::Pan(v) => TrackField::Pan(std::mem::replace(&mut t.pan, v.clamp(-1.0, 1.0))),
                    TrackField::Mute(v) => TrackField::Mute(std::mem::replace(&mut t.mute, v)),
                    TrackField::Solo(v) => TrackField::Solo(std::mem::replace(&mut t.solo, v)),
                    TrackField::Armed(v) => TrackField::Armed(std::mem::replace(&mut t.armed, v)),
                    TrackField::Owner(v) => TrackField::Owner(std::mem::replace(&mut t.owner, v)),
                };
                Op::SetTrack { id, field: old }
            }
            Op::AddClip { track, clip } => {
                let Some(t) = p.track_mut(track) else { return err("piste introuvable") };
                let id = clip.id;
                if t.clip(id).is_some() {
                    return err("clip déjà présent");
                }
                t.clips.push(clip);
                t.clips.sort_by(|a, b| a.start.total_cmp(&b.start));
                Op::RemoveClip { track, clip: id }
            }
            Op::RemoveClip { track, clip } => {
                let Some(t) = p.track_mut(track) else { return err("piste introuvable") };
                let Some(i) = t.clips.iter().position(|c| c.id == clip) else { return err("clip introuvable") };
                Op::AddClip { track, clip: t.clips.remove(i) }
            }
            Op::SetClip { track, clip, field } => {
                let Some(t) = p.track_mut(track) else { return err("piste introuvable") };
                let Some(c) = t.clips.iter_mut().find(|c| c.id == clip) else { return err("clip introuvable") };
                let old = match field {
                    ClipField::Name(v) => ClipField::Name(std::mem::replace(&mut c.name, v)),
                    ClipField::Start(v) => ClipField::Start(std::mem::replace(&mut c.start, v.max(0.0))),
                    ClipField::Offset(v) => ClipField::Offset(std::mem::replace(&mut c.offset, v.max(0.0))),
                    ClipField::Length(v) => ClipField::Length(std::mem::replace(&mut c.length, v.max(0.001))),
                    ClipField::GainDb(v) => ClipField::GainDb(std::mem::replace(&mut c.gain_db, v)),
                    ClipField::FadeIn(v) => ClipField::FadeIn(std::mem::replace(&mut c.fade_in, v.max(0.0))),
                    ClipField::FadeOut(v) => ClipField::FadeOut(std::mem::replace(&mut c.fade_out, v.max(0.0))),
                    ClipField::Muted(v) => ClipField::Muted(std::mem::replace(&mut c.muted, v)),
                };
                t.clips.sort_by(|a, b| a.start.total_cmp(&b.start));
                Op::SetClip { track, clip, field: old }
            }
            Op::AddEffect { target, slot, index } => {
                let ch = chain_mut(p, target)?;
                if ch.iter().any(|s| s.id == slot.id) {
                    return err("effet déjà présent");
                }
                let id = slot.id;
                ch.insert(index.min(ch.len()), slot);
                Op::RemoveEffect { target, slot: id }
            }
            Op::RemoveEffect { target, slot } => {
                let ch = chain_mut(p, target)?;
                let Some(i) = ch.iter().position(|s| s.id == slot) else { return err("effet introuvable") };
                Op::AddEffect { target, slot: ch.remove(i), index: i }
            }
            Op::MoveEffect { target, slot, index } => {
                let ch = chain_mut(p, target)?;
                let Some(i) = ch.iter().position(|s| s.id == slot) else { return err("effet introuvable") };
                let s = ch.remove(i);
                ch.insert(index.min(ch.len()), s);
                Op::MoveEffect { target, slot, index: i }
            }
            Op::SetParam { target, slot, param, value } => {
                let ch = chain_mut(p, target)?;
                let Some(s) = ch.iter_mut().find(|s| s.id == slot) else { return err("effet introuvable") };
                let value = match crate::effects::param_spec(&s.kind, &param) {
                    Some(spec) => spec.clamp(value),
                    None if s.kind.is_plugin() => value,
                    None => return err(format!("paramètre inconnu : {param}")),
                };
                let old = s.params.insert(param.clone(), value);
                let old = old.unwrap_or_else(|| {
                    crate::effects::param_spec(&s.kind, &param).map(|p| p.default).unwrap_or(0.0)
                });
                Op::SetParam { target, slot, param, value: old }
            }
            Op::SetEffectEnabled { target, slot, enabled } => {
                let ch = chain_mut(p, target)?;
                let Some(s) = ch.iter_mut().find(|s| s.id == slot) else { return err("effet introuvable") };
                Op::SetEffectEnabled { target, slot, enabled: std::mem::replace(&mut s.enabled, enabled) }
            }
            Op::SetPluginState { target, slot, state } => {
                let ch = chain_mut(p, target)?;
                let Some(s) = ch.iter_mut().find(|s| s.id == slot) else { return err("effet introuvable") };
                Op::SetPluginState { target, slot, state: std::mem::replace(&mut s.plugin_state, state) }
            }
            Op::SetChain { target, chain } => {
                let ch = chain_mut(p, target)?;
                Op::SetChain { target, chain: std::mem::replace(ch, chain) }
            }
            Op::ApplyMix { mix } => {
                let mut old = MixSnapshot { tracks: Default::default(), master: p.master.clone() };
                for (id, m) in mix.tracks {
                    if let Some(t) = p.track_mut(id) {
                        old.tracks.insert(
                            id,
                            TrackMix { volume_db: t.volume_db, pan: t.pan, mute: t.mute, effects: t.effects.clone() },
                        );
                        t.volume_db = m.volume_db;
                        t.pan = m.pan;
                        t.mute = m.mute;
                        t.effects = m.effects;
                    }
                }
                p.master = mix.master;
                Op::ApplyMix { mix: old }
            }
            Op::AddSection { section, index } => {
                if p.lyrics.iter().any(|s| s.id == section.id) {
                    return err("section déjà présente");
                }
                let id = section.id;
                p.lyrics.insert(index.min(p.lyrics.len()), section);
                Op::RemoveSection { id }
            }
            Op::RemoveSection { id } => {
                let Some(i) = p.lyrics.iter().position(|s| s.id == id) else { return err("section introuvable") };
                Op::AddSection { section: p.lyrics.remove(i), index: i }
            }
            Op::MoveSection { id, index } => {
                let Some(i) = p.lyrics.iter().position(|s| s.id == id) else { return err("section introuvable") };
                let s = p.lyrics.remove(i);
                p.lyrics.insert(index.min(p.lyrics.len()), s);
                Op::MoveSection { id, index: i }
            }
            Op::SetSection { id, title, text, start } => {
                let Some(s) = p.lyrics.iter_mut().find(|s| s.id == id) else { return err("section introuvable") };
                Op::SetSection {
                    id,
                    title: std::mem::replace(&mut s.title, title),
                    text: std::mem::replace(&mut s.text, text),
                    start: std::mem::replace(&mut s.start, start),
                }
            }
            Op::AddMarker { marker } => {
                let id = marker.id;
                p.markers.push(marker);
                p.markers.sort_by(|a, b| a.time.total_cmp(&b.time));
                Op::RemoveMarker { id }
            }
            Op::RemoveMarker { id } => {
                let Some(i) = p.markers.iter().position(|m| m.id == id) else { return err("marqueur introuvable") };
                Op::AddMarker { marker: p.markers.remove(i) }
            }
            Op::AddReference { reference } => {
                let id = reference.id;
                p.references.push(reference);
                Op::RemoveReference { id }
            }
            Op::RemoveReference { id } => {
                let Some(i) = p.references.iter().position(|m| m.id == id) else {
                    return err("référence introuvable");
                };
                Op::AddReference { reference: p.references.remove(i) }
            }
            Op::AddVariant { variant } => {
                let id = variant.id;
                p.variants.push(variant);
                Op::RemoveVariant { id }
            }
            Op::RemoveVariant { id } => {
                let Some(i) = p.variants.iter().position(|m| m.id == id) else { return err("variante introuvable") };
                Op::AddVariant { variant: p.variants.remove(i) }
            }
        })
    }

    /// Whether the audio graph must be rebuilt after this op (otherwise the
    /// engine can take a lighter parameter update).
    pub fn touches_audio(&self) -> bool {
        !matches!(
            self,
            Op::AddSection { .. }
                | Op::RemoveSection { .. }
                | Op::MoveSection { .. }
                | Op::SetSection { .. }
                | Op::AddMarker { .. }
                | Op::RemoveMarker { .. }
                | Op::AddReference { .. }
                | Op::RemoveReference { .. }
                | Op::AddVariant { .. }
                | Op::RemoveVariant { .. }
                | Op::SetProject { field: ProjectField::Name(_) }
                | Op::SetTrack { field: TrackField::Name(_) | TrackField::Color(_) | TrackField::Owner(_), .. }
        )
    }
}

/// A group of operations made as one action ("Nettoyer la voix").
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Transaction {
    pub id: Id,
    pub label: String,
    pub actor: Actor,
    pub ops: Vec<Op>,
}

impl Transaction {
    pub fn new(label: impl Into<String>, actor: Actor, ops: Vec<Op>) -> Transaction {
        Transaction { id: new_id(), label: label.into(), actor, ops }
    }

    /// Applies every op; on failure, rolls back what was applied. Returns
    /// the inverse transaction.
    pub fn apply(&self, p: &mut Project) -> Result<Transaction, OpError> {
        let mut inverses = Vec::with_capacity(self.ops.len());
        for op in &self.ops {
            match op.apply(p) {
                Ok(inv) => inverses.push(inv),
                Err(e) => {
                    for inv in inverses.iter().rev() {
                        let _ = inv.apply(p);
                    }
                    return Err(e);
                }
            }
        }
        inverses.reverse();
        Ok(Transaction { id: new_id(), label: self.label.clone(), actor: self.actor.clone(), ops: inverses })
    }

    pub fn touches_audio(&self) -> bool {
        self.ops.iter().any(Op::touches_audio)
    }
}
