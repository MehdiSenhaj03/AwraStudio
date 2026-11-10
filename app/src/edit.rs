//! Everyday editing: split, duplicate, copy/cut/paste, trim, fades, gain,
//! normalize, move to another track, markers, view. Shared by the timeline
//! (menus, gestures), the keyboard and the agent; every change is one
//! undoable step.

use studio_core::model::*;
use studio_core::ops::{ClipField, Op, ProjectField, TrackField};

use crate::studio::Studio;

/// Shortest clip a trim leaves (s).
pub const MIN_CLIP: f64 = 0.05;

fn clip(s: &Studio, track: Id, clip: Id) -> Option<Clip> {
    s.doc.project.track(track).and_then(|t| t.clip(clip)).cloned()
}

/// The asset's length: how far a clip can be stretched open.
pub fn asset_len(s: &Studio, c: &Clip) -> f64 {
    s.doc.project.assets.get(&c.asset).map(|a| a.duration).filter(|d| *d > 0.0).unwrap_or(c.offset + c.length)
}

/// What a clip laid on [a, b) of a track covers is cut away (as in any
/// DAW: the new clip replaces what was under it, nothing plays twice).
pub fn make_room(p: &Project, track: Id, a: f64, b: f64, except: Id) -> Vec<Op> {
    let mut ops = Vec::new();
    let Some(t) = p.track(track) else { return ops };
    for c in t.clips.iter().filter(|c| c.id != except && c.start < b && c.end() > a) {
        let (cs, ce) = (c.start, c.end());
        if cs >= a && ce <= b {
            ops.push(Op::RemoveClip { track, clip: c.id });
        } else if cs < a && ce > b {
            // the new clip lands in the middle: two pieces remain
            ops.push(Op::SetClip { track, clip: c.id, field: ClipField::Length(a - cs) });
            let right = Clip { id: new_id(), start: b, offset: c.offset + (b - cs), length: ce - b, ..c.clone() };
            ops.push(Op::AddClip { track, clip: right });
        } else if cs < a {
            ops.push(Op::SetClip { track, clip: c.id, field: ClipField::Length(a - cs) });
        } else {
            ops.push(Op::SetClip { track, clip: c.id, field: ClipField::Offset(c.offset + (b - cs)) });
            ops.push(Op::SetClip { track, clip: c.id, field: ClipField::Length(ce - b) });
            ops.push(Op::SetClip { track, clip: c.id, field: ClipField::Start(b) });
        }
    }
    ops
}

/// Lays a clip on a track over what was there, in one step.
fn lay(s: &mut Studio, label: &str, track: Id, clip: Clip) {
    let mut ops = make_room(&s.doc.project, track, clip.start, clip.end(), clip.id);
    ops.push(Op::AddClip { track, clip });
    s.commit_ops(label, ops);
}

/// Moves a clip to `start` over what was there.
pub fn move_clip(s: &mut Studio, track: Id, clip_id: Id, start: f64) {
    let Some(c) = clip(s, track, clip_id) else { return };
    let mut ops = make_room(&s.doc.project, track, start, start + c.length, clip_id);
    ops.push(Op::SetClip { track, clip: clip_id, field: ClipField::Start(start) });
    s.commit_ops("Déplacement du clip", ops);
}

/// Cuts a clip in two at `at` (timeline seconds): two clips, glued by
/// tiny fades so the cut doesn't click.
pub fn split_at(s: &mut Studio, track: Id, clip_id: Id, at: f64) -> bool {
    let Some(c) = clip(s, track, clip_id) else { return false };
    if at <= c.start + MIN_CLIP || at >= c.end() - MIN_CLIP {
        return false;
    }
    let left = at - c.start;
    let right = Clip { id: new_id(), start: at, offset: c.offset + left, length: c.length - left, fade_in: 0.005, fade_out: c.fade_out, ..c.clone() };
    s.commit_ops(
        "Couper le clip",
        vec![
            Op::SetClip { track, clip: clip_id, field: ClipField::Length(left) },
            Op::SetClip { track, clip: clip_id, field: ClipField::FadeOut(0.005) },
            Op::AddClip { track, clip: right },
        ],
    );
    true
}

/// Splits whatever clip of the track sits under the playhead (or the
/// selected clip's track; every track when none is selected).
pub fn split_at_playhead(s: &mut Studio) -> usize {
    let at = s.engine.position();
    let tracks: Vec<Id> = match s.selected_clip.map(|c| c.0).or(s.selected) {
        Some(t) => vec![t],
        None => s.doc.project.tracks.iter().map(|t| t.id).collect(),
    };
    let mut n = 0;
    for t in tracks {
        let under = s.doc.project.track(t).and_then(|tr| tr.clips.iter().find(|c| c.start < at && c.end() > at)).map(|c| c.id);
        if let Some(c) = under {
            if split_at(s, t, c, at) {
                n += 1;
            }
        }
    }
    if n == 0 {
        s.toasts.push("Aucun clip sous la tête de lecture sur cette piste".into());
    }
    n
}

/// A copy right after the clip.
pub fn duplicate(s: &mut Studio, track: Id, clip_id: Id) -> Option<Id> {
    let c = clip(s, track, clip_id)?;
    let copy = Clip { id: new_id(), start: c.end(), ..c };
    let id = copy.id;
    lay(s, "Dupliquer le clip", track, copy);
    s.selected_clip = Some((track, id));
    Some(id)
}

pub fn copy(s: &mut Studio, track: Id, clip_id: Id) {
    if let Some(c) = clip(s, track, clip_id) {
        s.clipboard = Some((track, c));
        s.toasts.push("Clip copié : Ctrl+V le colle à la tête de lecture".into());
    }
}

pub fn cut(s: &mut Studio, track: Id, clip_id: Id) {
    if let Some(c) = clip(s, track, clip_id) {
        s.clipboard = Some((track, c));
        s.commit_ops("Couper (presse-papiers)", vec![Op::RemoveClip { track, clip: clip_id }]);
        s.selected_clip = None;
    }
}

/// Pastes at `at` (default: the playhead) on `track` (default: the
/// selected track, else the one it was copied from).
pub fn paste(s: &mut Studio, track: Option<Id>, at: Option<f64>) -> bool {
    let Some((from, c)) = s.clipboard.clone() else {
        s.toasts.push("Rien à coller : copie d'abord un clip (clic droit ou Ctrl+C)".into());
        return false;
    };
    let p = &s.doc.project;
    let track = track.or(s.selected).filter(|t| p.track(*t).is_some()).or(Some(from).filter(|t| p.track(*t).is_some()));
    let Some(track) = track else { return false };
    let start = at.unwrap_or_else(|| s.engine.position()).max(0.0);
    let new = Clip { id: new_id(), start, ..c };
    let id = new.id;
    lay(s, "Coller le clip", track, new);
    s.selected_clip = Some((track, id));
    s.selected = Some(track);
    true
}

pub fn delete(s: &mut Studio, track: Id, clip_id: Id) {
    s.commit_ops("Supprimer le clip", vec![Op::RemoveClip { track, clip: clip_id }]);
    if s.selected_clip == Some((track, clip_id)) {
        s.selected_clip = None;
    }
}

pub fn toggle_mute(s: &mut Studio, track: Id, clip_id: Id) {
    if let Some(c) = clip(s, track, clip_id) {
        s.commit_ops(if c.muted { "Clip réactivé" } else { "Clip muet" }, vec![Op::SetClip { track, clip: clip_id, field: ClipField::Muted(!c.muted) }]);
    }
}

pub fn set_gain(s: &mut Studio, track: Id, clip_id: Id, db: f64) {
    s.commit_ops("Gain du clip", vec![Op::SetClip { track, clip: clip_id, field: ClipField::GainDb(db.clamp(-48.0, 24.0)) }]);
}

/// Brings the clip's peak to -1 dBFS.
pub fn normalize(s: &mut Studio, track: Id, clip_id: Id) -> Option<f64> {
    let c = clip(s, track, clip_id)?;
    let Some(pk) = s.assets.peaks.get(&c.asset) else {
        s.toasts.push("Le son de ce clip est encore en chargement".into());
        return None;
    };
    let (mn, mx) = pk.range(c.offset, c.offset + c.length);
    let peak = mn.abs().max(mx.abs()) as f64;
    if peak < 1e-5 {
        s.toasts.push("Ce clip est silencieux".into());
        return None;
    }
    let db = (-1.0 - 20.0 * peak.log10()).clamp(-48.0, 24.0);
    set_gain(s, track, clip_id, db);
    s.toasts.push(format!("Clip normalisé ({:+.1} dB)", db));
    Some(db)
}

pub fn set_fades(s: &mut Studio, track: Id, clip_id: Id, fade_in: f64, fade_out: f64) {
    let Some(c) = clip(s, track, clip_id) else { return };
    let half = c.length / 2.0;
    s.commit_ops(
        "Fondus",
        vec![
            Op::SetClip { track, clip: clip_id, field: ClipField::FadeIn(fade_in.clamp(0.0, half)) },
            Op::SetClip { track, clip: clip_id, field: ClipField::FadeOut(fade_out.clamp(0.0, half)) },
        ],
    );
}

/// Trims the clip to [start, start + length) of the timeline, keeping its
/// sound in place (the left edge moves the offset too).
pub fn trim(s: &mut Studio, track: Id, clip_id: Id, start: f64, length: f64) {
    let Some(c) = clip(s, track, clip_id) else { return };
    let offset = c.offset + (start - c.start);
    // opened over a neighbour: the neighbour gives way
    let mut ops = make_room(&s.doc.project, track, start, start + length, clip_id);
    ops.extend([
            Op::SetClip { track, clip: clip_id, field: ClipField::Start(start) },
            Op::SetClip { track, clip: clip_id, field: ClipField::Offset(offset) },
            Op::SetClip { track, clip: clip_id, field: ClipField::Length(length) },
    ]);
    s.commit_ops("Rogner le clip", ops);
}

pub fn move_to_track(s: &mut Studio, from: Id, clip_id: Id, to: Id) {
    let Some(c) = clip(s, from, clip_id) else { return };
    if from == to {
        return;
    }
    let mut ops = vec![Op::RemoveClip { track: from, clip: clip_id }];
    ops.extend(make_room(&s.doc.project, to, c.start, c.end(), clip_id));
    ops.push(Op::AddClip { track: to, clip: c });
    s.commit_ops("Déplacer vers une autre piste", ops);
    s.selected_clip = Some((to, clip_id));
    s.selected = Some(to);
}

/// Its start on the nearest bar.
pub fn snap_to_bar(s: &mut Studio, track: Id, clip_id: Id) {
    let Some(c) = clip(s, track, clip_id) else { return };
    let bar = s.doc.project.beat() * 4.0;
    let st = (c.start / bar).round() * bar;
    s.commit_ops("Caler sur la mesure", vec![Op::SetClip { track, clip: clip_id, field: ClipField::Start(st.max(0.0)) }]);
}

pub fn set_loop(s: &mut Studio, a: f64, b: f64) {
    if b - a > 0.05 {
        s.commit_ops("Boucle", vec![Op::SetProject { field: ProjectField::LoopRegion(Some((a, b))) }]);
        s.seek(a);
    }
}

/// The bar under `t`, as (start, end).
pub fn bar_at(s: &Studio, t: f64) -> (f64, f64) {
    let bar = s.doc.project.beat() * 4.0;
    let a = (t / bar).floor() * bar;
    (a, a + bar)
}

pub fn add_marker(s: &mut Studio, t: f64, name: Option<String>) {
    let n = s.doc.project.markers.len() + 1;
    let marker = Marker { id: new_id(), name: name.unwrap_or_else(|| format!("Repère {n}")), time: t.max(0.0) };
    s.commit_ops("Repère ajouté", vec![Op::AddMarker { marker }]);
}

pub fn duplicate_track(s: &mut Studio, track: Id) -> Option<Id> {
    let p = &s.doc.project;
    let index = p.tracks.iter().position(|t| t.id == track)?;
    let mut t = p.tracks[index].clone();
    t.id = new_id();
    t.name = format!("{} (copie)", t.name);
    t.armed = false;
    for c in t.clips.iter_mut() {
        c.id = new_id();
    }
    for e in t.effects.iter_mut() {
        e.id = new_id();
    }
    let id = t.id;
    s.commit_ops("Dupliquer la piste", vec![Op::AddTrack { track: t, index: index + 1 }]);
    s.selected = Some(id);
    s.flash = Some((id, awra::now_ms()));
    Some(id)
}

/// Up (-1) or down (+1) in the list.
pub fn move_track(s: &mut Studio, track: Id, delta: i32) {
    let p = &s.doc.project;
    let Some(i) = p.tracks.iter().position(|t| t.id == track) else { return };
    let j = (i as i32 + delta).clamp(0, p.tracks.len() as i32 - 1) as usize;
    if i != j {
        s.commit_ops("Ordre des pistes", vec![Op::MoveTrack { id: track, index: j }]);
    }
}

pub fn toggle_track(s: &mut Studio, track: Id, solo: bool) {
    let Some(t) = s.doc.project.track(track).cloned() else { return };
    if solo {
        s.commit_ops("Solo", vec![Op::SetTrack { id: track, field: TrackField::Solo(!t.solo) }]);
    } else {
        s.commit_ops("Muet", vec![Op::SetTrack { id: track, field: TrackField::Mute(!t.mute) }]);
    }
}

/// The whole song in view.
pub fn zoom_to_fit(s: &mut Studio) {
    let width = s.ui_info.get("timeline_width").and_then(|v| v.as_f64()).unwrap_or(900.0);
    let song = s.doc.project.duration().max(8.0) * 1.05;
    s.zoom = (width / song).clamp(2.0, 800.0);
    s.scroll_x = 0.0;
    s.ui_revision += 1;
}

/// The playhead by beats (or bars): arrows.
pub fn nudge_playhead(s: &mut Studio, beats: f64) {
    let beat = s.doc.project.beat();
    let t = ((s.engine.position() / beat).round() + beats) * beat;
    s.seek(t.max(0.0));
}
