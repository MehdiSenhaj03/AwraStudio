//! The multitrack timeline, drawn on one canvas: ruler, track headers
//! (mute, solo, record, level), clips with their waveforms, the playhead.
//! Click to place the playhead, drag clips, Ctrl+wheel to zoom, drop
//! files on a track.

use std::cell::RefCell;
use std::rc::Rc;

use awra::gdk::RGBA;
use awra::prelude::*;
use awra::{sig, DrawingArea, StyleManager, Typography, Widget};
use studio_core::model::*;
use studio_core::ops::{ClipField, Op, TrackField};

use super::{Panel, S};
use crate::studio::Studio;

pub const HEAD_W: f64 = 196.0;
pub const RULER_H: f64 = 42.0;
pub const TRACK_H: f64 = 76.0;
const ADD_H: f64 = 48.0;

#[derive(Clone, Copy, Debug)]
enum Drag {
    Seek,
    Loop { from: f64 },
    Clip { track: Id, clip: Id, from: f64, press_x: f64, moved: bool },
    Volume { track: Id },
    Scroll { press_x: f64, from: f64 },
}

#[derive(Default)]
struct View {
    drag: Option<Drag>,
    preview: Option<(Id, f64)>,
    preview_loop: Option<(f64, f64)>,
    width: f64,
    height: f64,
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

fn vol_pos(db: f64) -> f64 {
    ((db + 40.0) / 46.0).clamp(0.0, 1.0)
}

fn vol_from(p: f64) -> f64 {
    (p.clamp(0.0, 1.0) * 46.0 - 40.0 * 1.0).clamp(-40.0, 6.0)
}

/// Header buttons: (label, x offset) at y + 44.
const BTN: [(&str, f64); 3] = [("M", 12.0), ("S", 40.0), ("R", 68.0)];

fn draw(s: &Studio, view: &View, c: &awra::Canvas, w: f64, h: f64) {
    let dark = StyleManager::default().is_dark();
    let fg = style("foreground", RGBA::new(0.1, 0.1, 0.1, 1.0));
    let muted = style("muted", fg.with_alpha(0.6));
    let accent = style("accent", RGBA::new(0.35, 0.5, 0.95, 1.0));
    let sep = style("separator", fg.with_alpha(0.12));
    let danger = style("danger", RGBA::new(0.9, 0.3, 0.3, 1.0));
    let well = if dark { RGBA::new(1.0, 1.0, 1.0, 0.035) } else { RGBA::new(0.0, 0.0, 0.0, 0.03) };
    let p = &s.doc.project;

    // lanes background
    c.fill_rect(HEAD_W, 0.0, w - HEAD_W, h, &well);

    // grid and ruler
    let beat = p.beat();
    let bar = beat * 4.0;
    let px_bar = bar * s.zoom;
    let step_bars = if px_bar < 28.0 { 4.0 } else if px_bar < 56.0 { 2.0 } else { 1.0 };
    let first = (s.scroll_x / bar).floor().max(0.0);
    let mut b = first;
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
            // beats
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
    c.fill_rect(HEAD_W, RULER_H - 1.0, w - HEAD_W, 1.0, &sep);
    // lyric sections in the ruler
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
            c.fill_rect(xa, 0.0, xb - xa, RULER_H - 2.0, &accent.with_alpha(0.12));
        }
    }

    if p.tracks.is_empty() {
        let msg = "Glisse ton instru ici pour commencer";
        let tw = c.text_width(msg, Typography::Title2);
        c.text((w - tw) / 2.0, h / 2.0 - 30.0, msg, Typography::Title2, &fg);
        let sub = "ou Projet ▸ Importer de l'audio… — ou essaie le projet de démonstration";
        let sw = c.text_width(sub, Typography::Body);
        c.text((w - sw) / 2.0, h / 2.0 + 6.0, sub, Typography::Body, &muted);
        c.icon("awra-studio-wave-symbolic", w / 2.0 - 16.0, h / 2.0 - 80.0, 32.0, &muted);
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
        }
        let armed = s.recording == Some(t.id);
        // header
        c.round_rect(8.0, y + 8.0, 4.0, TRACK_H - 16.0, 2.0, Some(&tc), 0.0, None);
        c.text(20.0, y + 6.0, &t.name, Typography::Body, &fg);
        let kind = match &t.owner {
            Some(o) => format!("{} · {o}", t.kind.label()),
            None => t.kind.label().to_string(),
        };
        c.text(20.0, y + 24.0, &kind, Typography::Label, &muted);
        for (lbl, bx) in BTN {
            let on = match lbl {
                "M" => t.mute,
                "S" => t.solo,
                _ => armed || t.armed,
            };
            let bg = if on {
                match lbl {
                    "M" => style("warning", RGBA::new(0.95, 0.7, 0.2, 1.0)),
                    "S" => accent,
                    _ => danger,
                }
            } else {
                fg.with_alpha(0.08)
            };
            c.round_rect(bx + 8.0, y + 48.0, 24.0, 18.0, 5.0, Some(&bg), 0.0, None);
            let tw = c.text_width(lbl, Typography::Label);
            let white = RGBA::new(1.0, 1.0, 1.0, 1.0);
            c.text(bx + 8.0 + (24.0 - tw) / 2.0, y + 49.0, lbl, Typography::Label, if on { &white } else { &muted });
        }
        // volume bar
        let vx = 108.0;
        let vw = HEAD_W - vx - 14.0;
        c.round_rect(vx, y + 54.0, vw, 6.0, 3.0, Some(&fg.with_alpha(0.1)), 0.0, None);
        c.round_rect(vx, y + 54.0, vw * vol_pos(t.volume_db), 6.0, 3.0, Some(&tc.with_alpha(0.9)), 0.0, None);
        let vt = format!("{:+.1} dB", t.volume_db);
        c.text(vx + vw - c.text_width(&vt, Typography::Label), y + 36.0, &vt, Typography::Label, &muted);
        // level meter at the header's edge
        if let Some(m) = s.engine.shared.meters.try_lock() {
            if let Some((_, l, r)) = m.tracks.iter().find(|x| x.0 == t.id) {
                let lv = l.max(*r) as f64;
                let p = if lv > 1e-5 { ((20.0 * lv.log10() + 60.0) / 63.0).clamp(0.0, 1.0) } else { 0.0 };
                let mh = (TRACK_H - 16.0) * p;
                c.fill_rect(HEAD_W - 6.0, y + TRACK_H - 8.0 - mh, 3.0, mh, &if lv > 0.98 { danger } else { style("success", RGBA::new(0.3, 0.8, 0.4, 1.0)) });
            }
        }
        // lane separator
        c.fill_rect(0.0, y + TRACK_H - 1.0, w, 1.0, &sep);

        // clips
        for clip in &t.clips {
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
            let fill = tc.with_alpha(if clip.muted { 0.08 } else if sel { 0.34 } else { 0.22 });
            c.round_rect(cx0, y + 5.0, cx1 - cx0, TRACK_H - 10.0, 7.0, Some(&fill), if sel { 2.0 } else { 1.0 }, Some(&tc.with_alpha(if sel { 1.0 } else { 0.6 })));
            // waveform
            if let Some(pk) = s.assets.peaks.get(&clip.asset) {
                let mid = y + TRACK_H / 2.0 + 6.0;
                let amp = (TRACK_H - 30.0) / 2.0;
                let gain = db_to_gain(clip.gain_db) as f32;
                let wave = tc.with_alpha(0.85);
                let mut x = cx0.floor();
                while x < cx1 {
                    let ta = clip.offset + (x - x0) / s.zoom;
                    let tb = ta + 1.0 / s.zoom;
                    let (mn, mx) = pk.range(ta, tb);
                    let top = mid - (mx * gain).clamp(-1.0, 1.0) as f64 * amp;
                    let bot = mid - (mn * gain).clamp(-1.0, 1.0) as f64 * amp;
                    c.fill_rect(x, top, 1.0, (bot - top).max(1.0), &wave);
                    x += 1.0;
                }
            } else {
                c.text(cx0 + 8.0, y + TRACK_H / 2.0, "chargement…", Typography::Label, &muted);
            }
            if x0 >= HEAD_W - 2.0 && cx1 - cx0 > 60.0 {
                c.text(cx0 + 8.0, y + 8.0, &clip.name, Typography::Label, &fg);
            }
        }
        // the take being recorded
        if armed {
            let x0 = x_of(s, s.rec_start).max(HEAD_W);
            let x1 = x_of(s, s.engine.position()).min(w);
            if x1 > x0 {
                c.round_rect(x0, y + 5.0, x1 - x0, TRACK_H - 10.0, 7.0, Some(&danger.with_alpha(0.25)), 1.0, Some(&danger));
                c.text(x0 + 8.0, y + 8.0, "● Enregistrement", Typography::Label, &danger);
            }
        }
    }

    // add-track row
    let ay = track_y(s, p.tracks.len());
    if ay < h {
        let mut x = 14.0;
        for (lbl, _) in ADD_BUTTONS {
            let tw = c.text_width(lbl, Typography::Label) + 22.0;
            c.round_rect(x, ay + 12.0, tw, 24.0, 8.0, Some(&fg.with_alpha(0.06)), 1.0, Some(&sep));
            c.text(x + 11.0, ay + 16.0, lbl, Typography::Label, &muted);
            x += tw + 8.0;
        }
    }

    // header column edge
    c.fill_rect(HEAD_W - 1.0, 0.0, 1.0, h, &sep);

    // playhead
    let px = x_of(s, s.engine.position());
    if px >= HEAD_W && px <= w {
        let col = if s.recording.is_some() { danger } else { accent };
        c.fill_rect(px - 0.75, 0.0, 1.5, h, &col);
        c.round_rect(px - 6.0, RULER_H - 12.0, 12.0, 12.0, 3.0, Some(&col), 0.0, None);
    }
}

const ADD_BUTTONS: [(&str, TrackKind); 3] = [("+ Voix", TrackKind::Vocal), ("+ Adlibs", TrackKind::Adlib), ("+ Audio", TrackKind::Audio)];

fn add_button_at(c_width: impl Fn(&str) -> f64, x: f64) -> Option<TrackKind> {
    let mut bx = 14.0;
    for (lbl, k) in ADD_BUTTONS {
        let tw = c_width(lbl) + 22.0;
        if x >= bx && x <= bx + tw {
            return Some(k);
        }
        bx += tw + 8.0;
    }
    None
}

thread_local! {
    static SHIFT: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

fn shift_held() -> bool {
    SHIFT.with(|s| s.get())
}

fn approx_width(text: &str) -> f64 {
    text.chars().count() as f64 * 7.2
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

fn press(s: &mut Studio, view: &mut View, x: f64, y: f64, button: i32, n_press: i32) {
    let p = &s.doc.project;
    if y < RULER_H {
        if x > HEAD_W {
            let t = t_of(s, x);
            if n_press >= 2 {
                // a double click on the ruler removes the loop
                s.commit_ops("Boucle retirée", vec![Op::SetProject { field: studio_core::ops::ProjectField::LoopRegion(None) }]);
                return;
            }
            if shift_held() {
                view.drag = Some(Drag::Loop { from: snap(s, t) });
                return;
            }
            s.seek(t);
            view.drag = Some(Drag::Seek);
        }
        return;
    }
    if button == 2 {
        view.drag = Some(Drag::Scroll { press_x: x, from: s.scroll_x });
        return;
    }
    let Some((i, track)) = track_at(s, y) else {
        // the add-track row
        let ay = track_y(s, p.tracks.len());
        if y >= ay && y <= ay + ADD_H {
            if let Some(k) = add_button_at(approx_width, x) {
                s.new_track(k);
            }
        }
        return;
    };
    let ty = track_y(s, i);
    if x < HEAD_W {
        s.selected = Some(track);
        s.show_master_chain = false;
        s.ui_revision += 1;
        let t = p.track(track).cloned().unwrap();
        // M / S / R
        if y >= ty + 46.0 && y <= ty + 68.0 {
            for (lbl, bx) in BTN {
                if x >= bx + 8.0 && x <= bx + 32.0 {
                    match lbl {
                        "M" => s.commit_ops("Muet", vec![Op::SetTrack { id: track, field: TrackField::Mute(!t.mute) }]),
                        "S" => s.commit_ops("Solo", vec![Op::SetTrack { id: track, field: TrackField::Solo(!t.solo) }]),
                        _ => {
                            if t.kind != TrackKind::Instrumental || s.recording == Some(track) {
                                s.toggle_record();
                            }
                        }
                    }
                    return;
                }
            }
        }
        let vx = 108.0;
        if x >= vx && y >= ty + 48.0 {
            let vw = HEAD_W - vx - 14.0;
            let db = vol_from((x - vx) / vw);
            s.commit_ops("Volume", vec![Op::SetTrack { id: track, field: TrackField::VolumeDb((db * 2.0).round() / 2.0) }]);
            view.drag = Some(Drag::Volume { track });
        }
        let _ = n_press;
        return;
    }
    let t = t_of(s, x);
    s.selected = Some(track);
    s.show_master_chain = false;
    match clip_at(s, track, t) {
        Some(c) => {
            if n_press >= 2 {
                s.selected_clip = Some((track, c.id));
                split_at(s, track, c.id, snap(s, t));
                return;
            }
            s.selected_clip = Some((track, c.id));
            view.drag = Some(Drag::Clip { track, clip: c.id, from: c.start, press_x: x, moved: false });
        }
        None => {
            s.selected_clip = None;
            s.seek(t);
            view.drag = Some(Drag::Seek);
        }
    }
    s.ui_revision += 1;
}

fn motion(s: &mut Studio, view: &mut View, x: f64, _y: f64) {
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
            let vx = 108.0;
            let vw = HEAD_W - vx - 14.0;
            let db = vol_from((x - vx) / vw);
            s.commit_ops("Volume", vec![Op::SetTrack { id: track, field: TrackField::VolumeDb((db * 2.0).round() / 2.0) }]);
        }
        Some(Drag::Scroll { press_x, from }) => {
            s.scroll_x = (from - (x - press_x) / s.zoom).max(0.0);
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

impl TimelinePanel {
    pub fn new(s: &S) -> Rc<TimelinePanel> {
        let view = Rc::new(RefCell::new(View::default()));
        let (s2, v2) = (s.clone(), view.clone());
        let area = DrawingArea::with_canvas(move |_, c, w, h| {
            let Ok(st) = s2.try_borrow() else { return };
            let Ok(mut v) = v2.try_borrow_mut() else { return };
            v.width = w as f64;
            v.height = h as f64;
            draw(&st, &v, c, w as f64, h as f64);
        });
        area.set_hexpand(true);
        area.set_vexpand(true);
        area.set_content_size(600, 260);
        area.set_focusable(true);
        area.set_accepts_file_drops(true);
        let a: &Widget = area.as_ref();
        {
            let (s, v, ar) = (s.clone(), view.clone(), area.clone());
            a.connect_raw(sig::PRESSED, move |_, args| {
                ar.grab_focus();
                SHIFT.with(|x| x.set(args.mods().bits() & 1 != 0));
                let header_menu = {
                    let Ok(mut st) = s.try_borrow_mut() else { return };
                    press(&mut st, &mut v.borrow_mut(), args.x(), args.y(), args.button() as i32, args.n_press());
                    let on_name = args.x() < HEAD_W
                        && track_at(&st, args.y()).is_some_and(|(i, _)| args.y() < track_y(&st, i) + 44.0);
                    if (args.n_press() >= 2 && on_name) || (args.button() == 3 && args.x() < HEAD_W) {
                        track_at(&st, args.y()).map(|t| t.1)
                    } else {
                        None
                    }
                };
                if let Some(track) = header_menu {
                    track_menu(&s, &ar, track, args.x(), args.y());
                }
                ar.queue_draw();
            });
        }
        {
            let (s, v, ar) = (s.clone(), view.clone(), area.clone());
            a.connect_raw(sig::MOTION, move |_, args| {
                if v.borrow().drag.is_none() {
                    return;
                }
                let Ok(mut st) = s.try_borrow_mut() else { return };
                motion(&mut st, &mut v.borrow_mut(), args.x(), args.y());
                ar.queue_draw();
            });
        }
        {
            let (s, v, ar) = (s.clone(), view.clone(), area.clone());
            a.connect_raw(sig::RELEASED, move |_, _| {
                let Ok(mut st) = s.try_borrow_mut() else { return };
                release(&mut st, &mut v.borrow_mut());
                ar.queue_draw();
            });
        }
        {
            let (s, v, ar) = (s.clone(), view.clone(), area.clone());
            a.connect_raw(sig::SCROLL, move |_, args| {
                let Ok(mut st) = s.try_borrow_mut() else { return };
                let ctrl = args.mods().bits() & 2 != 0;
                let shift = args.mods().bits() & 1 != 0;
                let (dx, dy) = (args.dx(), args.dy());
                let (px, _) = ar.pointer_position().unwrap_or((HEAD_W, 0.0));
                if ctrl {
                    // zoom around the pointer
                    let t = t_of(&st, px.max(HEAD_W));
                    st.zoom = (st.zoom * if dy < 0.0 { 1.2 } else { 1.0 / 1.2 }).clamp(4.0, 800.0);
                    st.scroll_x = (t - (px.max(HEAD_W) - HEAD_W) / st.zoom).max(0.0);
                } else if shift || dx.abs() > dy.abs() {
                    let d = if dx.abs() > 0.0 { dx } else { dy };
                    st.scroll_x = (st.scroll_x + d * 40.0 / st.zoom).max(0.0);
                } else {
                    let max = ((st.doc.project.tracks.len() as f64 + 1.0) * TRACK_H - v.borrow().height + RULER_H + ADD_H).max(0.0);
                    st.scroll_y = (st.scroll_y + dy * 30.0).clamp(0.0, max);
                }
                ar.queue_draw();
            });
        }
        {
            let (s, ar) = (s.clone(), area.clone());
            a.connect_raw(sig::FILES_DROPPED, move |_, args| {
                let files: Vec<std::path::PathBuf> = args.text().unwrap_or_default().lines().filter(|l| !l.is_empty()).map(Into::into).collect();
                let Ok(mut st) = s.try_borrow_mut() else { return };
                let at = if args.x() > HEAD_W { snap(&st, t_of(&st, args.x())) } else { 0.0 };
                let track = track_at(&st, args.y()).map(|t| t.1);
                st.import_files(&files, track, if track.is_some() { at } else { 0.0 });
                ar.queue_draw();
            });
        }
        Rc::new(TimelinePanel { area, view })
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
        if s.engine.is_playing() || s.recording.is_some() || self.view.borrow().drag.is_some() {
            self.area.queue_draw();
        }
    }
}

/// Keeps the playhead in view while playing (called from the frame).
pub fn follow(s: &mut Studio, width: f64) {
    if !s.engine.is_playing() {
        return;
    }
    let x = x_of(s, s.engine.position());
    if x > width - 40.0 || x < HEAD_W {
        s.scroll_x = (s.engine.position() - 2.0).max(0.0);
    }
}

impl TimelinePanel {
    pub fn width(&self) -> f64 {
        self.view.borrow().width
    }
}
