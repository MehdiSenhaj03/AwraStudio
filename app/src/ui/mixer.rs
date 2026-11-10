//! The mixer: one strip per track and the master, with meters.

use std::cell::RefCell;
use std::rc::Rc;

use awra::prelude::*;
use awra::{ButtonAppearance, LevelMeter, Orientation, Typography, Widget};
use studio_core::model::*;
use studio_core::ops::{Op, ProjectField, TrackField};

use super::effects::{db_to_slider, slider_to_db};
use super::{label, updating, Panel, S};
use crate::studio::Studio;

struct Strip {
    id: Option<Id>,
    fader: awra::Slider,
    value: awra::Label,
    pan: Option<awra::Slider>,
    meter: LevelMeter,
    mute: Option<awra::ToggleButton>,
    solo: Option<awra::ToggleButton>,
}

pub struct MixerPanel {
    root: awra::Box,
    strips_box: awra::Box,
    strips: RefCell<Vec<Strip>>,
    signature: RefCell<String>,
    s: S,
}

impl MixerPanel {
    pub fn new(s: &S) -> Rc<MixerPanel> {
        let root = awra::Box::new(Orientation::Vertical, 6);
        root.set_margins(10, 12, 10, 12);
        let top = awra::Box::new(Orientation::Horizontal, 6);
        let t = label("Mixage", Typography::Title3);
        t.set_hexpand(true);
        top.append(&t);
        let auto = super::button("Équilibrer", Some("awra-studio-ai-symbolic"), ButtonAppearance::Secondary);
        auto.set_tooltip_text(Some("L'IA règle les volumes et le panoramique"));
        let c = s.clone();
        auto.connect_clicked(move |_| c.borrow_mut().ask_ai_local("Mixe et équilibre les voix"));
        top.append(&auto);
        root.append(&top);
        let strips_box = awra::Box::new(Orientation::Horizontal, 10);
        let sw = awra::ScrolledWindow::new();
        sw.set_policy(awra::PolicyType::Automatic, awra::PolicyType::Never);
        sw.set_child(Some(&strips_box));
        sw.set_vexpand(true);
        root.append(&sw);
        Rc::new(MixerPanel { root, strips_box, strips: RefCell::new(Vec::new()), signature: RefCell::new(String::new()), s: s.clone() })
    }

    fn strip(&self, track: Option<&Track>) -> (awra::Box, Strip) {
        let col = awra::Box::new(Orientation::Vertical, 6);
        col.set_size_request(92, -1);
        let name = label(track.map(|t| t.name.as_str()).unwrap_or("Master"), Typography::Label);
        name.set_ellipsize(awra::pango::EllipsizeMode::End);
        name.set_xalign(0.5);
        col.append(&name);
        let pan = track.map(|t| {
            let p = awra::Slider::with_range(Orientation::Horizontal, -1.0, 1.0, 0.01);
            p.set_draw_value(false);
            p.set_value(t.pan);
            p.set_tooltip_text(Some("Panoramique"));
            col.append(&p);
            p
        });
        let row = awra::Box::new(Orientation::Horizontal, 4);
        row.set_vexpand(true);
        row.set_halign(awra::Align::Center);
        let fader = awra::Slider::with_range(Orientation::Vertical, 0.0, 1.0, 0.001);
        fader.set_draw_value(false);
        fader.set_vexpand(true);
        fader.set_size_request(-1, 140);
        let meter = LevelMeter::new(Orientation::Vertical);
        meter.as_ref().set_vexpand(true);
        row.append(&fader);
        row.append(&meter);
        col.append(&row);
        let db = track.map(|t| t.volume_db).unwrap_or(0.0);
        fader.set_value(db_to_slider(db));
        let value = label(&format!("{db:+.1} dB"), Typography::Label);
        value.set_xalign(0.5);
        value.set_tabular_digits(true);
        col.append(&value);
        let (mut mute, mut solo) = (None, None);
        if let Some(t) = track {
            let ms = awra::Box::new(Orientation::Horizontal, 4);
            ms.set_halign(awra::Align::Center);
            let m = awra::ToggleButton::with_label("M");
            m.set_active(t.mute);
            let s = awra::ToggleButton::with_label("S");
            s.set_active(t.solo);
            ms.append(&m);
            ms.append(&s);
            col.append(&ms);
            let (c, id) = (self.s.clone(), t.id);
            m.connect_toggled(move |b| {
                if !updating() {
                    c.borrow_mut().commit_ops("Muet", vec![Op::SetTrack { id, field: TrackField::Mute(b.is_active()) }]);
                }
            });
            let (c, id) = (self.s.clone(), t.id);
            s.connect_toggled(move |b| {
                if !updating() {
                    c.borrow_mut().commit_ops("Solo", vec![Op::SetTrack { id, field: TrackField::Solo(b.is_active()) }]);
                }
            });
            mute = Some(m);
            solo = Some(s);
        }
        let id = track.map(|t| t.id);
        let (c, v2) = (self.s.clone(), value.clone());
        fader.connect_value_changed(move |f| {
            if updating() {
                return;
            }
            let db = slider_to_db(f.value());
            v2.set_text(&format!("{db:+.1} dB"));
            let op = match id {
                Some(id) => Op::SetTrack { id, field: TrackField::VolumeDb(db) },
                None => Op::SetProject { field: ProjectField::MasterVolumeDb(db) },
            };
            c.borrow_mut().commit_ops("Volume", vec![op]);
        });
        if let (Some(p), Some(id)) = (&pan, id) {
            let c = self.s.clone();
            p.connect_value_changed(move |w| {
                if !updating() {
                    c.borrow_mut().commit_ops("Panoramique", vec![Op::SetTrack { id, field: TrackField::Pan((w.value() * 20.0).round() / 20.0) }]);
                }
            });
        }
        (col, Strip { id, fader, value, pan, meter, mute, solo })
    }
}

impl Panel for MixerPanel {
    fn widget(&self) -> Widget {
        self.root.clone().upcast::<Widget>()
    }

    fn refresh(&self, s: &Studio) {
        let sig: String = s.doc.project.tracks.iter().map(|t| format!("{}{};", t.id, t.name)).collect();
        if *self.signature.borrow() != sig {
            *self.signature.borrow_mut() = sig;
            super::clear(&self.strips_box);
            let mut strips = Vec::new();
            for t in &s.doc.project.tracks {
                let (w, st) = self.strip(Some(t));
                self.strips_box.append(&w);
                strips.push(st);
            }
            self.strips_box.append(&awra::Separator::new(Orientation::Vertical));
            let (w, st) = self.strip(None);
            self.strips_box.append(&w);
            strips.push(st);
            *self.strips.borrow_mut() = strips;
        }
        for st in self.strips.borrow().iter() {
            let (db, pan, mute, solo) = match st.id {
                Some(id) => match s.doc.project.track(id) {
                    Some(t) => (t.volume_db, t.pan, t.mute, t.solo),
                    None => continue,
                },
                None => (s.doc.project.master.volume_db, 0.0, false, false),
            };
            if (slider_to_db(st.fader.value()) - db).abs() > 0.05 {
                st.fader.set_value(db_to_slider(db));
            }
            st.value.set_text(&format!("{db:+.1} dB"));
            if let Some(p) = &st.pan {
                if (p.value() - pan).abs() > 0.01 {
                    p.set_value(pan);
                }
            }
            if let Some(m) = &st.mute {
                m.set_active(mute);
            }
            if let Some(x) = &st.solo {
                x.set_active(solo);
            }
        }
    }

    fn tick(&self, s: &Studio) {
        let Some(m) = s.engine.shared.meters.try_lock().map(|m| m.clone()) else { return };
        for st in self.strips.borrow().iter() {
            match st.id {
                Some(id) => {
                    if let Some((_, l, r)) = m.tracks.iter().find(|x| x.0 == id) {
                        st.meter.set_levels(*l, *r);
                    }
                }
                None => st.meter.set_levels(m.master.0, m.master.1),
            }
        }
    }
}
