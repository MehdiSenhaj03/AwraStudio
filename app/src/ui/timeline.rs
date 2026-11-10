//! The multitrack timeline, drawn on one canvas: ruler, track headers
//! (mute, solo, record, level), clips with their waveforms, the playhead.
//! Click to place the playhead, drag clips, Ctrl+wheel to zoom (eased),
//! drop audio files from outside on a track or below the tracks (a new
//! one), with a preview of where they land.

use std::cell::RefCell;
use std::rc::Rc;

use awra::gdk::RGBA;
use awra::prelude::*;
use awra::{sig, DrawingArea, StyleManager, Typography, Widget};
use studio_core::model::*;
use studio_core::ops::{ClipField, Op, TrackField};

use super::{Panel, S};
use crate::studio::{Studio, UiAction};

pub const HEAD_W: f64 = 196.0;
pub const RULER_H: f64 = 42.0;
pub const TRACK_H: f64 = 76.0;
const ADD_H: f64 = 54.0;
/// The "+ Ajouter une piste" pill, inside the header column.
const PILL_X: f64 = 12.0;
const PILL_W: f64 = HEAD_W - 24.0;
const PILL_H: f64 = 30.0;
/// The ghost of a dropped file before its length is known (s).
const GHOST_LEN: f64 = 4.0;

#[derive(Clone, Copy, Debug)]
enum Drag {
    Seek,
    Loop { from: f64 },
    Clip { track: Id, clip: Id, from: f64, press_x: f64, moved: bool },
    Volume { track: Id },
    Scroll { press_x: f64, from: f64 },
}

/// What is under the pointer.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Hit {
    None,
    Ruler,
    Button(Id, u8),
    Volume(Id),
    Header(Id),
    Clip(Id, Id),
    Lane(Id),
    AddPill,
    Empty(usize),
}

/// What a press asks of the interface (popovers need the widget).
enum Action {
    None,
    TrackMenu(Id),
    AddMenu,
}

#[derive(Default)]
struct View {
    drag: Option<Drag>,
    preview: Option<(Id, f64)>,
    preview_loop: Option<(f64, f64)>,
    width: f64,
    height: f64,
    hover: Option<Hit>,
    /// A file drag over the timeline (x, y).
    drop: Option<(f64, f64)>,
    /// Eased motion: zoom (target, anchor time, anchor x) and scrolling.
    zoom_target: Option<(f64, f64, f64)>,
    scroll_target: Option<f64>,
    scroll_y_target: Option<f64>,
    /// Empty-state buttons, as drawn (x, y, w, h).
    empty_buttons: Vec<(f64, f64, f64, f64)>,
}

pub struct TimelinePanel {
    area: DrawingArea,
    view: Rc<RefCell<View>>,
}

fn color(hex: &str) -> RGBA {
    RGBA::parse(hex).unwrap_or(RGBA::new(0.4, 0.6, 0.9, 1.0))
}

fn style(name: &str, fallback: RGBA) -> RGBA {
    StyleManager::default().color(name).unwrap_or(fallback)
}

fn x_of(s: &Studio, t: f64) -> f64 {
    HEAD_W + (t - s.scroll_x) * s.zoom
}

fn t_of(s: &Studio, x: f64) -> f64 {
    ((x - HEAD_W) / s.zoom + s.scroll_x).max(0.0)
}

fn track_y(s: &Studio, i: usize) -> f64 {
    RULER_H + i as f64 * TRACK_H - s.scroll_y
}

/// Rows of files being imported into new tracks.
fn pending_new(s: &Studio) -> usize {
    s.pending_imports.iter().filter(|p| p.track.is_none()).count()
}

/// The top of the add row (after the tracks and the new ones coming).
fn add_row_y(s: &Studio) -> f64 {
    track_y(s, s.doc.project.tracks.len() + pending_new(s))
}

fn vol_pos(db: f64) -> f64 {
    ((db + 40.0) / 46.0).clamp(0.0, 1.0)
}

fn vol_from(p: f64) -> f64 {
    (p.clamp(0.0, 1.0) * 46.0 - 40.0).clamp(-40.0, 6.0)
}

/// Header buttons M, S, R: x offsets at y + 48.
const BTN: [(&str, f64); 3] = [("M", 20.0), ("S", 48.0), ("R", 76.0)];
const VOL_X: f64 = 112.0;

fn ease(t: f64) -> f64 {
    t * t * (3.0 - 2.0 * t)
}

fn now() -> f64 {
    awra::now_ms()
}

fn track_at(s: &Studio, y: f64) -> Option<(usize, Id)> {
    if y < RULER_H {
        return None;
    }
    let i = ((y - RULER_H + s.scroll_y) / TRACK_H).floor();
    if i < 0.0 {
        return None;
    }
    s.doc.project.tracks.get(i as usize).map(|t| (i as usize, t.id))
}

fn clip_at(s: &Studio, track: Id, t: f64) -> Option<Clip> {
    s.doc.project.track(track)?.clips.iter().rev().find(|c| t >= c.start && t <= c.end()).cloned()
}

fn snap(s: &Studio, t: f64) -> f64 {
    // a sixteenth note
    let g = s.doc.project.beat() / 4.0;
    (t / g).round() * g
}

fn in_rect(x: f64, y: f64, r: (f64, f64, f64, f64)) -> bool {
    x >= r.0 && x <= r.0 + r.2 && y >= r.1 && y <= r.1 + r.3
}

fn hit(s: &Studio, v: &View, x: f64, y: f64) -> Hit {
    if s.doc.project.tracks.is_empty() {
        for (i, r) in v.empty_buttons.iter().enumerate() {
            if in_rect(x, y, *r) {
                return Hit::Empty(i);
            }
        }
    }
    if y < RULER_H {
        return if x > HEAD_W { Hit::Ruler } else { Hit::None };
    }
    let ay = add_row_y(s);
    if !s.doc.project.tracks.is_empty() && in_rect(x, y, (PILL_X, ay + 12.0, PILL_W, PILL_H)) {
        return Hit::AddPill;
    }
    let Some((i, track)) = track_at(s, y) else { return Hit::None };
    let ty = track_y(s, i);
    if x < HEAD_W {
        if y >= ty + 46.0 && y <= ty + 68.0 {
            for (k, (_, bx)) in BTN.iter().enumerate() {
                if x >= *bx && x <= bx + 24.0 {
                    return Hit::Button(track, k as u8);
                }
            }
            if x >= VOL_X {
                return Hit::Volume(track);
            }
        }
        return Hit::Header(track);
    }
    match clip_at(s, track, t_of(s, x)) {
        Some(c) => Hit::Clip(track, c.id),
        None => Hit::Lane(track),
    }
}

/// Where dropped files go: a track and a time, or a new track.
fn drop_target(s: &Studio, x: f64, y: f64) -> (Option<Id>, f64) {
    match track_at(s, y) {
        Some((_, id)) if x > HEAD_W => (Some(id), snap(s, t_of(s, x))),
        Some((_, id)) => (Some(id), 0.0),
        None => (None, 0.0),
    }
}

fn pill(c: &awra::Canvas, x: f64, y: f64, text: &str, bg: &RGBA, fg: &RGBA) -> f64 {
    let w = c.text_width(text, Typography::Label) + 20.0;
    c.round_rect(x, y, w, 22.0, 11.0, Some(bg), 0.0, None);
    c.text(x + 10.0, y + 3.0, text, Typography::Label, fg);
    w
}

/// A moving sheen while audio is on its way.
fn shimmer(c: &awra::Canvas, x: f64, y: f64, w: f64, col: &RGBA, t: f64) {
    if w <= 8.0 {
        return;
    }
    let n = ((w - 6.0) / 6.0) as usize;
    for i in 0..n {
        let phase = (i as f64 * 0.35 - t / 160.0).sin() * 0.5 + 0.5;
        let hh = 4.0 + 18.0 * phase;
        c.round_rect(x + 3.0 + i as f64 * 6.0, y - hh / 2.0, 3.0, hh, 1.5, Some(&col.with_alpha(0.25 + 0.35 * phase as f32)), 0.0, None);
    }
}

fn draw(s: &Studio, view: &mut View, c: &awra::Canvas, w: f64, h: f64) {
    let t_now = now();
    let dark = StyleManager::default().is_dark();
    let fg = style("foreground", RGBA::new(0.1, 0.1, 0.1, 1.0));
    let muted = style("muted", fg.with_alpha(0.6));
    let accent = style("accent", RGBA::new(0.35, 0.5, 0.95, 1.0));
    let on_accent = style("on-accent", RGBA::new(1.0, 1.0, 1.0, 1.0));
    let sep = style("separator", fg.with_alpha(0.12));
    let danger = style("danger", RGBA::new(0.9, 0.3, 0.3, 1.0));
    let canvas = style("canvas", RGBA::new(0.5, 0.5, 0.5, 1.0));
    let lanes = super::appearance::effective("lanes") as f32;
    let well = if lanes > 0.05 {
        canvas.with_alpha(lanes)
    } else if dark {
        RGBA::new(1.0, 1.0, 1.0, lanes)
    } else {
        RGBA::new(0.0, 0.0, 0.0, lanes)
    };
    let p = &s.doc.project;
    let hover = view.hover.unwrap_or(Hit::None);
    let white = RGBA::new(1.0, 1.0, 1.0, 1.0);

    c.fill_rect(HEAD_W, 0.0, w - HEAD_W, h, &well);

    // grid and ruler
    let beat = p.beat();
    let bar = beat * 4.0;
    let px_bar = bar * s.zoom;
    let step_bars = if px_bar < 28.0 { 4.0 } else if px_bar < 56.0 { 2.0 } else { 1.0 };
    let mut b = (s.scroll_x / bar).floor().max(0.0);
    loop {
        let t = b * bar;
        let x = x_of(s, t);
        if x > w {
            break;
        }
        if x >= HEAD_W {
            let major = (b as i64) % (step_bars as i64) == 0;
            c.fill_rect(x, RULER_H, 1.0, h - RULER_H, &sep.with_alpha(if major { sep.alpha() } else { sep.alpha() * 0.45 }));
            if major {
                c.text(x + 4.0, 3.0, &format!("{}", b as i64 + 1), Typography::Label, &muted);
            }
            if px_bar > 90.0 {
                for k in 1..4 {
                    let bx = x_of(s, t + k as f64 * beat);
                    c.fill_rect(bx, RULER_H - 6.0, 1.0, 6.0, &sep);
                    c.fill_rect(bx, RULER_H, 1.0, h - RULER_H, &sep.with_alpha(sep.alpha() * 0.25));
                }
            }
        }
        b += 1.0;
    }
    if hover == Hit::Ruler {
        c.fill_rect(HEAD_W, 0.0, w - HEAD_W, RULER_H - 1.0, &fg.with_alpha(0.03));
    }
    c.fill_rect(HEAD_W, RULER_H - 1.0, w - HEAD_W, 1.0, &sep);
    for l in &p.lyrics {
        if let Some(t) = l.start {
            let x = x_of(s, t);
            if x >= HEAD_W && x < w {
                c.round_rect(x, 21.0, c.text_width(&l.title, Typography::Label) + 10.0, 17.0, 5.0, Some(&accent.with_alpha(0.16)), 0.0, None);
                c.fill_rect(x, 21.0, 1.5, 17.0, &accent);
                c.text(x + 5.0, 21.0, &l.title, Typography::Label, &accent);
            }
        }
    }
    if let Some((a, bb)) = view.preview_loop.or(p.loop_region) {
        let (xa, xb) = (x_of(s, a).max(HEAD_W), x_of(s, bb));
        if xb > xa {
            c.fill_rect(xa, 0.0, xb - xa, RULER_H - 2.0, &accent.with_alpha(0.14));
            c.fill_rect(xa, RULER_H - 3.0, xb - xa, 2.0, &accent);
        }
    }

    // empty project: a welcome with actions
    view.empty_buttons.clear();
    if p.tracks.is_empty() && s.pending_imports.is_empty() {
        let cx = HEAD_W + (w - HEAD_W) / 2.0;
        let cy = (h + RULER_H) / 2.0;
        let breathe = 0.7 + 0.3 * ((t_now / 900.0).sin() * 0.5 + 0.5);
        c.icon("awra-studio-wave-symbolic", cx - 18.0, cy - 100.0, 36.0, &accent.with_alpha(breathe as f32));
        let msg = "Glisse ton instru ici pour commencer";
        let tw = c.text_width(msg, Typography::Title2);
        c.text(cx - tw / 2.0, cy - 54.0, msg, Typography::Title2, &fg);
        let sub = "MP3, WAV, FLAC, OGG, M4A — elle devient la première piste";
        let sw = c.text_width(sub, Typography::Body);
        c.text(cx - sw / 2.0, cy - 20.0, sub, Typography::Body, &muted);
        let labels = ["Importer une instru…", "Essayer la démo"];
        let widths: Vec<f64> = labels.iter().map(|l| c.text_width(l, Typography::Body) + 36.0).collect();
        let total = widths.iter().sum::<f64>() + 12.0;
        let mut x = cx - total / 2.0;
        for (i, l) in labels.iter().enumerate() {
            let r = (x, cy + 14.0, widths[i], 36.0);
            let hov = hover == Hit::Empty(i);
            let (bg, tc) = if i == 0 { (accent.with_alpha(if hov { 1.0 } else { 0.88 }), on_accent) } else { (fg.with_alpha(if hov { 0.13 } else { 0.07 }), fg) };
            c.round_rect(r.0, r.1 - if hov { 1.0 } else { 0.0 }, r.2, r.3, 10.0, Some(&bg), 0.0, None);
            c.text(r.0 + 18.0, r.1 + 9.0 - if hov { 1.0 } else { 0.0 }, l, Typography::Body, &tc);
            view.empty_buttons.push(r);
            x += widths[i] + 12.0;
        }
    }

    // tracks
    for (i, t) in p.tracks.iter().enumerate() {
        let y = track_y(s, i);
        if y > h || y + TRACK_H < RULER_H {
            continue;
        }
        let tc = color(&t.color);
        let selected = s.selected == Some(t.id) && !s.show_master_chain;
        if selected {
            c.fill_rect(0.0, y, w, TRACK_H, &accent.with_alpha(if dark { 0.10 } else { 0.07 }));
        } else if matches!(hover, Hit::Header(id) | Hit::Button(id, _) | Hit::Volume(id) if id == t.id) {
            c.fill_rect(0.0, y, HEAD_W, TRACK_H, &fg.with_alpha(0.04));
        }
        if let Some((id, since)) = s.flash {
            let k = 1.0 - (t_now - since) / 1400.0;
            if id == t.id && k > 0.0 {
                c.fill_rect(0.0, y, w, TRACK_H, &accent.with_alpha((0.22 * ease(k)) as f32));
            }
        }
        let armed = s.recording == Some(t.id);
        let pulse = ((t_now / 260.0).sin() * 0.5 + 0.5) as f32;
        // header
        c.round_rect(8.0, y + 8.0, 4.0, TRACK_H - 16.0, 2.0, Some(&tc), 0.0, None);
        c.text(20.0, y + 6.0, &t.name, Typography::Body, &fg);
        let mut sub: Vec<String> = Vec::new();
        if !t.name.to_lowercase().contains(&t.kind.label().to_lowercase()) {
            sub.push(t.kind.label().to_string());
        }
        if let Some(o) = &t.owner {
            sub.push(o.clone());
        }
        c.text(20.0, y + 24.0, &sub.join(" · "), Typography::Label, &muted);
        if armed {
            c.circle(HEAD_W - 18.0, y + 15.0, 4.0 + pulse as f64, &danger.with_alpha(0.6 + 0.4 * pulse));
        }
        for (k, (lbl, bx)) in BTN.iter().enumerate() {
            let on = match *lbl {
                "M" => t.mute,
                "S" => t.solo,
                _ => armed || t.armed,
            };
            let hov = hover == Hit::Button(t.id, k as u8);
            let bg = if on {
                let base = match *lbl {
                    "M" => style("warning", RGBA::new(0.95, 0.7, 0.2, 1.0)),
                    "S" => accent,
                    _ => danger,
                };
                if *lbl == "R" && armed {
                    base.with_alpha(0.65 + 0.35 * pulse)
                } else {
                    base
                }
            } else {
                fg.with_alpha(if hov { 0.16 } else { 0.08 })
            };
            c.round_rect(*bx, y + 48.0, 24.0, 18.0, 5.0, Some(&bg), 0.0, None);
            let tw = c.text_width(lbl, Typography::Label);
            c.text(bx + (24.0 - tw) / 2.0, y + 49.0, lbl, Typography::Label, if on { &white } else if hov { &fg } else { &muted });
        }
        // volume bar
        let vw = HEAD_W - VOL_X - 14.0;
        let hov_v = hover == Hit::Volume(t.id) || matches!(view.drag, Some(Drag::Volume { track }) if track == t.id);
        c.round_rect(VOL_X, y + 54.0, vw, 6.0, 3.0, Some(&fg.with_alpha(if hov_v { 0.16 } else { 0.1 })), 0.0, None);
        c.round_rect(VOL_X, y + 54.0, vw * vol_pos(t.volume_db), 6.0, 3.0, Some(&tc.with_alpha(0.9)), 0.0, None);
        if hov_v {
            c.circle(VOL_X + vw * vol_pos(t.volume_db), y + 57.0, 6.0, &tc);
        }
        let vt = format!("{:+.1} dB", t.volume_db);
        c.text(VOL_X + vw - c.text_width(&vt, Typography::Label), y + 36.0, &vt, Typography::Label, &muted);
        if let Some(m) = s.engine.shared.meters.try_lock() {
            if let Some((_, l, r)) = m.tracks.iter().find(|x| x.0 == t.id) {
                let lv = l.max(*r) as f64;
                let p = if lv > 1e-5 { ((20.0 * lv.log10() + 60.0) / 63.0).clamp(0.0, 1.0) } else { 0.0 };
                let mh = (TRACK_H - 16.0) * p;
                c.fill_rect(HEAD_W - 6.0, y + TRACK_H - 8.0 - mh, 3.0, mh, &if lv > 0.98 { danger } else { style("success", RGBA::new(0.3, 0.8, 0.4, 1.0)) });
            }
        }
        c.fill_rect(0.0, y + TRACK_H - 1.0, w, 1.0, &sep);

        // clips
        for clip in &t.clips {
            let dragging = matches!(view.preview, Some((id, _)) if id == clip.id);
            let start = match view.preview {
                Some((id, st)) if id == clip.id => st,
                _ => clip.start,
            };
            let x0 = x_of(s, start);
            let x1 = x_of(s, start + clip.length);
            if x1 < HEAD_W || x0 > w {
                continue;
            }
            let cx0 = x0.max(HEAD_W);
            let cx1 = x1.min(w);
            let sel = s.selected_clip.map(|c| c.1) == Some(clip.id);
            let hov = hover == Hit::Clip(t.id, clip.id);
            if dragging {
                // where it was, and a shadow under the moving one
                let ox = x_of(s, clip.start).max(HEAD_W);
                let ow = (x_of(s, clip.start + clip.length).min(w) - ox).max(0.0);
                c.round_rect(ox, y + 5.0, ow, TRACK_H - 10.0, 7.0, None, 1.0, Some(&tc.with_alpha(0.35)));
                c.round_rect(cx0 + 3.0, y + 9.0, cx1 - cx0, TRACK_H - 10.0, 8.0, Some(&RGBA::new(0.0, 0.0, 0.0, 0.18)), 0.0, None);
                c.fill_rect(x0.max(HEAD_W) - 1.0, RULER_H, 2.0, h - RULER_H, &accent.with_alpha(0.7));
            }
            let fill = tc.with_alpha(if clip.muted { 0.08 } else if sel || dragging { 0.36 } else if hov { 0.28 } else { 0.22 });
            let bw = if sel || dragging { 2.0 } else { 1.0 };
            c.round_rect(cx0, y + 5.0, cx1 - cx0, TRACK_H - 10.0, 7.0, Some(&fill), bw, Some(&tc.with_alpha(if sel || hov || dragging { 1.0 } else { 0.6 })));
            if let Some(pk) = s.assets.peaks.get(&clip.asset) {
                let fade = s.assets.loaded_at.get(&clip.asset).map(|t0| ((t_now - t0) / 350.0).clamp(0.0, 1.0)).unwrap_or(1.0);
                let mid = y + TRACK_H / 2.0 + 6.0;
                let amp = (TRACK_H - 30.0) / 2.0 * (0.6 + 0.4 * ease(fade));
                let gain = db_to_gain(clip.gain_db) as f32;
                let wave = tc.with_alpha(0.85 * ease(fade) as f32);
                let mut x = cx0.floor();
                while x < cx1 {
                    let ta = clip.offset + (x - x0) / s.zoom;
                    let (mn, mx) = pk.range(ta, ta + 1.0 / s.zoom);
                    let top = mid - (mx * gain).clamp(-1.0, 1.0) as f64 * amp;
                    let bot = mid - (mn * gain).clamp(-1.0, 1.0) as f64 * amp;
                    c.fill_rect(x, top, 1.0, (bot - top).max(1.0), &wave);
                    x += 1.0;
                }
            } else {
                shimmer(c, cx0, y + TRACK_H / 2.0 + 6.0, cx1 - cx0, &tc, t_now);
            }
            if x0 >= HEAD_W - 2.0 && cx1 - cx0 > 60.0 {
                c.text(cx0 + 8.0, y + 8.0, &clip.name, Typography::Label, &fg);
            }
        }
        // files being imported into this track
        for pend in s.pending_imports.iter().filter(|p| p.track == Some(t.id)) {
            let x0 = x_of(s, pend.at).max(HEAD_W);
            let x1 = x_of(s, pend.at + GHOST_LEN).min(w);
            if x1 > x0 {
                c.round_rect(x0, y + 5.0, x1 - x0, TRACK_H - 10.0, 7.0, Some(&accent.with_alpha(0.14)), 1.0, Some(&accent.with_alpha(0.7)));
                shimmer(c, x0, y + TRACK_H / 2.0 + 6.0, x1 - x0, &accent, t_now);
                c.text(x0 + 8.0, y + 8.0, &format!("{} — import…", pend.name), Typography::Label, &accent);
            }
        }
        if armed {
            let x0 = x_of(s, s.rec_start).max(HEAD_W);
            let x1 = x_of(s, s.engine.position()).min(w);
            if x1 > x0 {
                c.round_rect(x0, y + 5.0, x1 - x0, TRACK_H - 10.0, 7.0, Some(&danger.with_alpha(0.18 + 0.1 * pulse)), 1.0, Some(&danger));
                c.text(x0 + 8.0, y + 8.0, "● Enregistrement", Typography::Label, &danger);
            }
        }
    }

    // new tracks being imported
    for (k, pend) in s.pending_imports.iter().filter(|p| p.track.is_none()).enumerate() {
        let y = track_y(s, p.tracks.len() + k);
        c.round_rect(8.0, y + 8.0, 4.0, TRACK_H - 16.0, 2.0, Some(&accent), 0.0, None);
        c.text(20.0, y + 6.0, &pend.name, Typography::Body, &fg);
        c.text(20.0, y + 24.0, "Décodage…", Typography::Label, &muted);
        let x0 = x_of(s, pend.at).max(HEAD_W);
        let x1 = x_of(s, pend.at + GHOST_LEN * 2.0).min(w);
        if x1 > x0 {
            c.round_rect(x0, y + 5.0, x1 - x0, TRACK_H - 10.0, 7.0, Some(&accent.with_alpha(0.12)), 1.0, Some(&accent.with_alpha(0.6)));
            shimmer(c, x0, y + TRACK_H / 2.0 + 6.0, x1 - x0, &accent, t_now);
        }
        c.fill_rect(0.0, y + TRACK_H - 1.0, w, 1.0, &sep);
    }

    // add-track pill, inside the header column
    let ay = add_row_y(s);
    let dropping_new = view.drop.is_some_and(|(dx, dy)| drop_target(s, dx, dy).0.is_none());
    if ay < h && !p.tracks.is_empty() && !dropping_new {
        let hov = hover == Hit::AddPill;
        let bg = if hov { accent.with_alpha(0.16) } else { fg.with_alpha(0.06) };
        let border = if hov { accent.with_alpha(0.5) } else { sep };
        c.round_rect(PILL_X, ay + 12.0, PILL_W, PILL_H, 9.0, Some(&bg), 1.0, Some(&border));
        let lbl = "+  Ajouter une piste";
        let tw = c.text_width(lbl, Typography::Label);
        c.text(PILL_X + (PILL_W - tw) / 2.0, ay + 18.0, lbl, Typography::Label, if hov { &accent } else { &muted });
    }

    // a file dragged over: where it will land
    if let Some((dx, dy)) = view.drop {
        let glow = (0.75 + 0.25 * (t_now / 300.0).sin()) as f32;
        let (target, at) = drop_target(s, dx, dy);
        match target.and_then(|id| p.tracks.iter().position(|t| t.id == id).map(|i| (i, id))) {
            Some((i, id)) => {
                let y = track_y(s, i);
                c.fill_rect(0.0, y, w, TRACK_H, &accent.with_alpha(0.08));
                c.round_rect(1.0, y + 1.0, w - 2.0, TRACK_H - 2.0, 8.0, None, 2.0, Some(&accent.with_alpha(0.8 * glow)));
                let x0 = x_of(s, at).max(HEAD_W);
                let x1 = x_of(s, at + GHOST_LEN).min(w);
                if x1 > x0 {
                    c.round_rect(x0, y + 5.0, x1 - x0, TRACK_H - 10.0, 7.0, Some(&accent.with_alpha(0.28 * glow)), 1.5, Some(&accent));
                }
                c.fill_rect(x0 - 1.0, RULER_H, 2.0, h - RULER_H, &accent.with_alpha(0.9));
                let name = p.track(id).map(|t| t.name.clone()).unwrap_or_default();
                let label = format!("Ajouter à « {name} » · {}", super::fmt_time(at));
                let lx = (x0 + 8.0).min(w - c.text_width(&label, Typography::Label) - 30.0).max(HEAD_W + 4.0);
                pill(c, lx, y + TRACK_H - 30.0, &label, &accent, &on_accent);
            }
            None => {
                let y = add_row_y(s);
                c.round_rect(6.0, y + 4.0, w - 12.0, TRACK_H - 8.0, 10.0, Some(&accent.with_alpha(0.12 * glow)), 2.0, Some(&accent.with_alpha(0.8 * glow)));
                let has_beat = p.tracks.iter().any(|t| t.kind == TrackKind::Instrumental);
                let label = if has_beat { "+  Nouvelle piste" } else { "+  Nouvelle piste — l'instru" };
                let tw = c.text_width(label, Typography::Title3);
                c.text((w - tw) / 2.0, y + TRACK_H / 2.0 - 12.0, label, Typography::Title3, &accent);
            }
        }
    } else if s.drag_over {
        // files over the window, not yet here: invite them in
        c.round_rect(2.0, RULER_H + 2.0, w - 4.0, h - RULER_H - 4.0, 12.0, None, 2.0, Some(&accent.with_alpha(0.5)));
    }

    c.fill_rect(HEAD_W - 1.0, 0.0, 1.0, h, &sep);

    // playhead
    let px = x_of(s, s.engine.position());
    if px >= HEAD_W && px <= w {
        let col = if s.recording.is_some() { danger } else { accent };
        c.fill_rect(px - 3.0, 0.0, 6.0, h, &col.with_alpha(0.10));
        c.fill_rect(px - 0.75, 0.0, 1.5, h, &col);
        c.round_rect(px - 6.0, RULER_H - 12.0, 12.0, 12.0, 3.0, Some(&col), 0.0, None);
    }
}

fn press(s: &mut Studio, view: &mut View, x: f64, y: f64, button: i32, n_press: i32) -> Action {
    if button == 2 && x > HEAD_W {
        view.drag = Some(Drag::Scroll { press_x: x, from: s.scroll_x });
        return Action::None;
    }
    match hit(s, view, x, y) {
        Hit::Empty(0) => s.ui_actions.push(UiAction::ImportDialog),
        Hit::Empty(_) => s.ui_actions.push(UiAction::Demo),
        Hit::Ruler => {
            let t = t_of(s, x);
            if n_press >= 2 {
                s.commit_ops("Boucle retirée", vec![Op::SetProject { field: studio_core::ops::ProjectField::LoopRegion(None) }]);
            } else if shift_held() {
                view.drag = Some(Drag::Loop { from: snap(s, t) });
            } else {
                s.seek(t);
                view.drag = Some(Drag::Seek);
            }
        }
        Hit::AddPill => return Action::AddMenu,
        Hit::Button(track, k) => {
            let Some(t) = s.doc.project.track(track).cloned() else { return Action::None };
            s.selected = Some(track);
            s.show_master_chain = false;
            match k {
                0 => s.commit_ops("Muet", vec![Op::SetTrack { id: track, field: TrackField::Mute(!t.mute) }]),
                1 => s.commit_ops("Solo", vec![Op::SetTrack { id: track, field: TrackField::Solo(!t.solo) }]),
                _ => {
                    if t.kind != TrackKind::Instrumental || s.recording == Some(track) {
                        s.toggle_record();
                    } else {
                        s.toasts.push("On n'enregistre pas sur l'instru : ajoute une piste de voix".into());
                    }
                }
            }
        }
        Hit::Volume(track) => {
            s.selected = Some(track);
            let db = vol_from((x - VOL_X) / (HEAD_W - VOL_X - 14.0));
            s.commit_ops("Volume", vec![Op::SetTrack { id: track, field: TrackField::VolumeDb((db * 2.0).round() / 2.0) }]);
            view.drag = Some(Drag::Volume { track });
        }
        Hit::Header(track) => {
            s.selected = Some(track);
            s.show_master_chain = false;
            s.ui_revision += 1;
            if n_press >= 2 || button == 3 {
                return Action::TrackMenu(track);
            }
        }
        Hit::Clip(track, clip) => {
            s.selected = Some(track);
            s.show_master_chain = false;
            s.selected_clip = Some((track, clip));
            if n_press >= 2 {
                let t = snap(s, t_of(s, x));
                split_at(s, track, clip, t);
            } else if let Some(c) = s.doc.project.track(track).and_then(|t| t.clip(clip)) {
                view.drag = Some(Drag::Clip { track, clip, from: c.start, press_x: x, moved: false });
            }
            s.ui_revision += 1;
        }
        Hit::Lane(track) => {
            s.selected = Some(track);
            s.show_master_chain = false;
            s.selected_clip = None;
            let t = t_of(s, x);
            s.seek(t);
            view.drag = Some(Drag::Seek);
            s.ui_revision += 1;
        }
        Hit::None => {}
    }
    if button == 3 && x < HEAD_W {
        if let Some((_, id)) = track_at(s, y) {
            return Action::TrackMenu(id);
        }
    }
    Action::None
}

fn motion(s: &mut Studio, view: &mut View, x: f64) {
    match view.drag {
        Some(Drag::Seek) => {
            let t = t_of(s, x.max(HEAD_W));
            s.seek(t);
        }
        Some(Drag::Loop { from }) => {
            let t = snap(s, t_of(s, x.max(HEAD_W)));
            view.preview_loop = Some((from.min(t), from.max(t)));
        }
        Some(Drag::Clip { track, clip, from, press_x, moved }) => {
            let dt = (x - press_x) / s.zoom;
            if moved || (x - press_x).abs() > 3.0 {
                let st = snap(s, (from + dt).max(0.0));
                view.preview = Some((clip, st));
                view.drag = Some(Drag::Clip { track, clip, from, press_x, moved: true });
            }
        }
        Some(Drag::Volume { track }) => {
            let db = vol_from((x - VOL_X) / (HEAD_W - VOL_X - 14.0));
            s.commit_ops("Volume", vec![Op::SetTrack { id: track, field: TrackField::VolumeDb((db * 2.0).round() / 2.0) }]);
        }
        Some(Drag::Scroll { press_x, from }) => {
            s.scroll_x = (from - (x - press_x) / s.zoom).max(0.0);
            view.scroll_target = None;
        }
        None => {}
    }
}

fn release(s: &mut Studio, view: &mut View) {
    if let Some((a, b)) = view.preview_loop.take() {
        if b - a > 0.2 {
            s.commit_ops("Boucle", vec![Op::SetProject { field: studio_core::ops::ProjectField::LoopRegion(Some((a, b))) }]);
            s.seek(a);
        }
    }
    if let Some(Drag::Clip { track, clip, moved: true, .. }) = view.drag {
        if let Some((_, st)) = view.preview {
            s.commit_ops("Déplacement du clip", vec![Op::SetClip { track, clip, field: ClipField::Start(st) }]);
        }
    }
    view.drag = None;
    view.preview = None;
}

fn split_at(s: &mut Studio, track: Id, clip: Id, at: f64) {
    let Some(c) = s.doc.project.track(track).and_then(|t| t.clip(clip)).cloned() else { return };
    if at <= c.start + 0.05 || at >= c.end() - 0.05 {
        return;
    }
    let left = at - c.start;
    let right = Clip { id: new_id(), start: at, offset: c.offset + left, length: c.length - left, fade_in: 0.005, ..c.clone() };
    s.commit_ops(
        "Couper le clip",
        vec![Op::SetClip { track, clip, field: ClipField::Length(left) }, Op::SetClip { track, clip, field: ClipField::FadeOut(0.005) }, Op::AddClip { track, clip: right }],
    );
}

thread_local! {
    static SHIFT: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

fn shift_held() -> bool {
    SHIFT.with(|s| s.get())
}

/// A small menu of entries.
fn menu(area: &DrawingArea, x: f64, y: f64, items: Vec<(String, Box<dyn Fn()>)>) {
    let pop = awra::Popover::new();
    let col = awra::Box::new(awra::Orientation::Vertical, 2);
    col.set_margins(6, 6, 6, 6);
    for (text, f) in items {
        if text.is_empty() {
            col.append(&awra::Separator::new(awra::Orientation::Horizontal));
            continue;
        }
        let b = awra::Button::with_label(&text);
        b.set_appearance(awra::ButtonAppearance::Ghost);
        let p = pop.clone();
        b.connect_clicked(move |_| {
            p.popdown();
            f();
        });
        col.append(&b);
    }
    pop.set_child(Some(&col));
    pop.popup_at(area, x, y);
}

fn add_menu(s: &S, area: &DrawingArea, x: f64, y: f64) {
    let mut items: Vec<(String, Box<dyn Fn()>)> = Vec::new();
    for (label, kind) in [("Voix", TrackKind::Vocal), ("Adlibs / backs", TrackKind::Adlib), ("Audio", TrackKind::Audio), ("Instru", TrackKind::Instrumental)] {
        let s = s.clone();
        items.push((label.to_string(), Box::new(move || {
            let id = s.borrow_mut().new_track(kind);
            s.borrow_mut().flash = Some((id, awra::now_ms()));
        })));
    }
    items.push((String::new(), Box::new(|| {})));
    let s2 = s.clone();
    items.push(("Importer un fichier audio…".into(), Box::new(move || s2.borrow_mut().ui_actions.push(UiAction::ImportDialog))));
    menu(area, x, y, items);
}

/// Rename, change the kind or colour of a track, or delete it.
fn track_menu(s: &S, area: &DrawingArea, track: Id, x: f64, y: f64) {
    let Some(t) = s.borrow().doc.project.track(track).cloned() else { return };
    let pop = awra::Popover::new();
    let col = awra::Box::new(awra::Orientation::Vertical, 8);
    col.set_margins(10, 10, 10, 10);
    let name = awra::Entry::new();
    name.set_text(&t.name);
    col.append(&name);
    let kinds = [TrackKind::Vocal, TrackKind::Adlib, TrackKind::Instrumental, TrackKind::Audio];
    let kd = awra::Dropdown::from_strings(&["Voix", "Adlibs", "Instru", "Audio"]);
    kd.set_selected(kinds.iter().position(|k| *k == t.kind).unwrap_or(0) as u32);
    col.append(&kd);
    let colors = awra::Box::new(awra::Orientation::Horizontal, 4);
    for c in TRACK_COLORS {
        let b = awra::Button::new();
        let sw = DrawingArea::with_canvas(move |_, cv, w, h| {
            cv.round_rect(0.0, 0.0, w as f64, h as f64, 4.0, Some(&color(c)), 0.0, None);
        });
        sw.set_content_size(16, 16);
        b.set_child(Some(&sw));
        b.set_appearance(awra::ButtonAppearance::Ghost);
        let (s2, p2) = (s.clone(), pop.clone());
        b.connect_clicked(move |_| {
            s2.borrow_mut().commit_ops("Couleur", vec![Op::SetTrack { id: track, field: TrackField::Color(c.to_string()) }]);
            p2.popdown();
        });
        colors.append(&b);
    }
    col.append(&colors);
    let del = awra::Button::with_label("Supprimer la piste");
    del.set_appearance(awra::ButtonAppearance::Destructive);
    col.append(&del);
    pop.set_child(Some(&col));
    let s2 = s.clone();
    name.connect_activate(move |e| {
        let n = e.text();
        if !n.trim().is_empty() {
            s2.borrow_mut().commit_ops("Renommer", vec![Op::SetTrack { id: track, field: TrackField::Name(n) }]);
        }
    });
    let s2 = s.clone();
    let n2 = name.clone();
    pop.connect_closed(move |_| {
        let n = n2.text();
        let cur = s2.borrow().doc.project.track(track).map(|t| t.name.clone());
        if !n.trim().is_empty() && cur.is_some_and(|c| c != n) {
            s2.borrow_mut().commit_ops("Renommer", vec![Op::SetTrack { id: track, field: TrackField::Name(n) }]);
        }
    });
    let s2 = s.clone();
    kd.connect_selected_notify(move |d| {
        let k = kinds[d.selected() as usize % kinds.len()];
        s2.borrow_mut().commit_ops("Type de piste", vec![Op::SetTrack { id: track, field: TrackField::Kind(k) }]);
    });
    let (s2, p2) = (s.clone(), pop.clone());
    del.connect_clicked(move |_| {
        p2.popdown();
        s2.borrow_mut().commit_ops("Piste supprimée", vec![Op::RemoveTrack { id: track }]);
    });
    pop.popup_at(area, x, y);
}

pub fn split_selected_clip(s: &mut Studio) {
    if let Some((t, c)) = s.selected_clip {
        let at = s.engine.position();
        split_at(s, t, c, at);
    }
}

pub fn delete_selected_clip(s: &mut Studio) {
    if let Some((t, c)) = s.selected_clip.take() {
        s.commit_ops("Supprimer le clip", vec![Op::RemoveClip { track: t, clip: c }]);
    }
}

fn cursor_for(h: Hit, drag: Option<Drag>) -> &'static str {
    match (drag, h) {
        (Some(Drag::Clip { .. } | Drag::Scroll { .. }), _) => "grabbing",
        (Some(Drag::Volume { .. }), _) => "ew-resize",
        (_, Hit::Clip(..)) => "grab",
        (_, Hit::Button(..) | Hit::AddPill | Hit::Empty(_) | Hit::Header(_)) => "pointer",
        (_, Hit::Volume(_)) => "ew-resize",
        (_, Hit::Ruler) => "text",
        _ => "default",
    }
}

impl TimelinePanel {
    pub fn new(s: &S) -> Rc<TimelinePanel> {
        let view = Rc::new(RefCell::new(View::default()));
        let (s2, v2) = (s.clone(), view.clone());
        let area = DrawingArea::with_canvas(move |_, c, w, h| {
            let Ok(st) = s2.try_borrow() else { return };
            let Ok(mut v) = v2.try_borrow_mut() else { return };
            v.width = w as f64;
            v.height = h as f64;
            draw(&st, &mut v, c, w as f64, h as f64);
        });
        area.set_hexpand(true);
        area.set_vexpand(true);
        area.set_content_size(200, 200);
        area.set_focusable(true);
        area.set_accepts_file_drops(true);
        let a: &Widget = area.as_ref();
        {
            let (s, v, ar) = (s.clone(), view.clone(), area.clone());
            a.connect_raw(sig::PRESSED, move |_, args| {
                ar.grab_focus();
                SHIFT.with(|x| x.set(args.mods().bits() & 1 != 0));
                let action = {
                    let Ok(mut st) = s.try_borrow_mut() else { return };
                    press(&mut st, &mut v.borrow_mut(), args.x(), args.y(), args.button() as i32, args.n_press())
                };
                match action {
                    Action::TrackMenu(id) => track_menu(&s, &ar, id, args.x(), args.y()),
                    Action::AddMenu => add_menu(&s, &ar, args.x(), args.y()),
                    Action::None => {}
                }
                ar.queue_draw();
            });
        }
        {
            let (s, v, ar) = (s.clone(), view.clone(), area.clone());
            a.connect_raw(sig::MOTION, move |_, args| {
                let Ok(mut st) = s.try_borrow_mut() else { return };
                let mut vw = v.borrow_mut();
                let dragging = vw.drag.is_some();
                if dragging {
                    motion(&mut st, &mut vw, args.x());
                }
                let h = hit(&st, &vw, args.x(), args.y());
                if vw.hover != Some(h) || dragging {
                    vw.hover = Some(h);
                    ar.set_cursor_from_name(Some(cursor_for(h, vw.drag)));
                    ar.queue_draw();
                }
            });
        }
        {
            let (v, ar) = (view.clone(), area.clone());
            a.connect_raw(sig::LEAVE, move |_, _| {
                if v.borrow_mut().hover.take().is_some() {
                    ar.queue_draw();
                }
            });
        }
        {
            let (s, v, ar) = (s.clone(), view.clone(), area.clone());
            a.connect_raw(sig::RELEASED, move |_, args| {
                let Ok(mut st) = s.try_borrow_mut() else { return };
                let mut vw = v.borrow_mut();
                release(&mut st, &mut vw);
                let h = hit(&st, &vw, args.x(), args.y());
                ar.set_cursor_from_name(Some(cursor_for(h, None)));
                ar.queue_draw();
            });
        }
        {
            let (s, v, ar) = (s.clone(), view.clone(), area.clone());
            a.connect_raw(sig::SCROLL, move |_, args| {
                let Ok(st) = s.try_borrow() else { return };
                let mut vw = v.borrow_mut();
                let ctrl = args.mods().bits() & 2 != 0;
                let shift = args.mods().bits() & 1 != 0;
                let (dx, dy) = (args.dx(), args.dy());
                let (px, _) = ar.pointer_position().unwrap_or((HEAD_W, 0.0));
                if ctrl {
                    // zoom around the pointer, eased
                    let cur = vw.zoom_target.map(|z| z.0).unwrap_or(st.zoom);
                    let z = (cur * if dy < 0.0 { 1.25 } else { 1.0 / 1.25 }).clamp(4.0, 800.0);
                    let anchor_x = px.max(HEAD_W);
                    vw.zoom_target = Some((z, t_of(&st, anchor_x), anchor_x));
                } else if shift || dx.abs() > dy.abs() {
                    let d = if dx.abs() > 0.0 { dx } else { dy };
                    let cur = vw.scroll_target.unwrap_or(st.scroll_x);
                    vw.scroll_target = Some((cur + d * 60.0 / st.zoom).max(0.0));
                } else {
                    let rows = st.doc.project.tracks.len() + pending_new(&st);
                    let max = ((rows as f64 + 1.0) * TRACK_H - vw.height + RULER_H + ADD_H).max(0.0);
                    let cur = vw.scroll_y_target.unwrap_or(st.scroll_y);
                    vw.scroll_y_target = Some((cur + dy * 42.0).clamp(0.0, max));
                }
                ar.queue_draw();
            });
        }
        {
            let (s, v, ar) = (s.clone(), view.clone(), area.clone());
            a.connect_raw(sig::DROP_HOVER, move |_, args| {
                v.borrow_mut().drop = if args.active() { Some((args.x(), args.y())) } else { None };
                if let Ok(mut st) = s.try_borrow_mut() {
                    st.drag_over = args.active();
                }
                ar.queue_draw();
            });
        }
        {
            let (s, v, ar) = (s.clone(), view.clone(), area.clone());
            a.connect_raw(sig::FILES_DROPPED, move |_, args| {
                let files: Vec<std::path::PathBuf> = args.text().unwrap_or_default().lines().filter(|l| !l.is_empty()).map(Into::into).collect();
                v.borrow_mut().drop = None;
                let Ok(mut st) = s.try_borrow_mut() else { return };
                st.drag_over = false;
                let (track, at) = drop_target(&st, args.x(), args.y());
                st.import_files(&files, track, at);
                ar.queue_draw();
            });
        }
        Rc::new(TimelinePanel { area, view })
    }

    /// Eased zoom and scrolling, following the playhead. True while
    /// something moves.
    pub fn animate(&self, s: &mut Studio) -> bool {
        let mut v = self.view.borrow_mut();
        let width = v.width;
        if s.engine.is_playing() && v.drag.is_none() && width > HEAD_W + 100.0 {
            let x = x_of(s, s.engine.position());
            if x > width - 60.0 || x < HEAD_W {
                v.scroll_target = Some((s.engine.position() - 1.0).max(0.0));
            }
        }
        let mut moving = false;
        if let Some((z, at, ax)) = v.zoom_target {
            s.zoom += (z - s.zoom) * 0.28;
            if (z - s.zoom).abs() < z * 0.002 {
                s.zoom = z;
                v.zoom_target = None;
            }
            s.scroll_x = (at - (ax - HEAD_W) / s.zoom).max(0.0);
            v.scroll_target = None;
            moving = true;
        }
        if let Some(t) = v.scroll_target {
            s.scroll_x += (t - s.scroll_x) * 0.25;
            if (t - s.scroll_x).abs() * s.zoom < 0.5 {
                s.scroll_x = t;
                v.scroll_target = None;
            }
            moving = true;
        }
        if let Some(t) = v.scroll_y_target {
            s.scroll_y += (t - s.scroll_y) * 0.3;
            if (t - s.scroll_y).abs() < 0.5 {
                s.scroll_y = t;
                v.scroll_y_target = None;
            }
            moving = true;
        }
        if let Some((_, since)) = s.flash {
            if now() - since > 1500.0 {
                s.flash = None;
            } else {
                moving = true;
            }
        }
        if moving {
            self.area.queue_draw();
        }
        moving
    }
}

impl Panel for TimelinePanel {
    fn widget(&self) -> Widget {
        self.area.clone().upcast::<Widget>()
    }

    fn refresh(&self, _s: &Studio) {
        self.area.queue_draw();
    }

    fn tick(&self, s: &Studio) {
        let t = now();
        let fading = s.assets.loaded_at.values().any(|t0| t - t0 < 400.0);
        let v = self.view.borrow();
        if s.engine.is_playing()
            || s.recording.is_some()
            || v.drag.is_some()
            || v.drop.is_some()
            || !s.pending_imports.is_empty()
            || s.doc.project.tracks.is_empty()
            || s.drag_over
            || fading
        {
            self.area.queue_draw();
        }
    }
}
