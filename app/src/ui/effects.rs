//! The selected track's effects (or the master's): simple controls first,
//! every setting one click away; quick AI chains.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use awra::prelude::*;
use awra::{ButtonAppearance, Orientation, Typography, Widget};
use studio_core::effects::{self, EffectKind, Unit};
use studio_core::model::*;
use studio_core::ops::{chain, Op, Target, TrackField};

use super::{button, label, scrolled, updating, Panel, S};
use crate::studio::Studio;

enum Control {
    Slider(awra::Slider, awra::Label, effects::ParamSpec),
    Choice(awra::Dropdown),
    Toggle(awra::Switch),
    Plugin(awra::Slider, awra::Label, f64, f64),
}

pub struct EffectsPanel {
    root: awra::Box,
    title: awra::Label,
    target_switch: awra::SegmentedControl,
    quick: awra::Box,
    track_row: awra::Box,
    volume: awra::Slider,
    volume_label: awra::Label,
    pan: awra::Slider,
    list: awra::Box,
    controls: RefCell<HashMap<(Id, String), Control>>,
    signature: RefCell<String>,
    advanced: Rc<RefCell<Vec<Id>>>,
    s: S,
    window: awra::Window,
}

fn target_of(s: &Studio) -> Option<Target> {
    if s.show_master_chain {
        return Some(Target::Master);
    }
    s.selected.filter(|id| s.doc.project.track(*id).is_some()).map(Target::Track)
}

fn signature(s: &Studio) -> String {
    let Some(t) = target_of(s) else { return "none".into() };
    let ch = chain(&s.doc.project, t).cloned().unwrap_or_default();
    let mut sig = format!("{t:?}");
    for slot in ch {
        sig.push_str(&format!("|{}:{}", slot.id, slot.enabled));
    }
    if let Target::Track(id) = t {
        if let Some(tr) = s.doc.project.track(id) {
            sig.push_str(&format!("|{}|{:?}", tr.name, tr.kind));
        }
    }
    sig
}

impl EffectsPanel {
    pub fn new(s: &S, window: &awra::Window) -> Rc<EffectsPanel> {
        let root = awra::Box::new(Orientation::Vertical, 8);
        root.set_margins(10, 12, 10, 12);
        let top = awra::Box::new(Orientation::Horizontal, 8);
        let title = label("", Typography::Title3);
        title.set_hexpand(true);
        let target_switch = awra::SegmentedControl::new();
        target_switch.append("Piste");
        target_switch.append("Master");
        top.append(&title);
        top.append(&target_switch);
        root.append(&top);

        let quick = awra::Box::new(Orientation::Horizontal, 6);
        root.append(&quick);

        let track_row = awra::Box::new(Orientation::Horizontal, 8);
        let vl = label("Volume", Typography::Label);
        let volume = awra::Slider::with_range(Orientation::Horizontal, 0.0, 1.0, 0.001);
        volume.set_draw_value(false);
        volume.set_hexpand(true);
        let volume_label = label("0.0 dB", Typography::Label);
        volume_label.set_width_chars(8);
        let pl = label("Pan", Typography::Label);
        let pan = awra::Slider::with_range(Orientation::Horizontal, -1.0, 1.0, 0.01);
        pan.set_draw_value(false);
        pan.set_size_request(110, -1);
        for w in [super::wd(&vl), super::wd(&volume), super::wd(&volume_label), super::wd(&pl), super::wd(&pan)] {
            track_row.append(w);
        }
        root.append(&track_row);

        let list = awra::Box::new(Orientation::Vertical, 8);
        root.append(&scrolled(&list));

        let add_row = awra::Box::new(Orientation::Horizontal, 6);
        let names: Vec<&str> = effects::BUILTIN.iter().map(|k| effects::spec(k).name).collect();
        let dd = awra::Dropdown::from_strings(&names);
        dd.set_hexpand(true);
        let add = button("Ajouter l'effet", Some("list-add-symbolic"), ButtonAppearance::Secondary);
        add_row.append(&dd);
        add_row.append(&add);
        root.append(&add_row);

        let panel = Rc::new(EffectsPanel {
            root,
            title,
            target_switch: target_switch.clone(),
            quick,
            track_row,
            volume: volume.clone(),
            volume_label,
            pan: pan.clone(),
            list,
            controls: RefCell::new(HashMap::new()),
            signature: RefCell::new(String::new()),
            advanced: Rc::new(RefCell::new(Vec::new())),
            s: s.clone(),
            window: window.clone(),
        });
        let c = s.clone();
        target_switch.connect_changed(move |_, i| {
            if updating() {
                return;
            }
            let mut st = c.borrow_mut();
            st.show_master_chain = i == 1;
            st.ui_revision += 1;
        });
        let c = s.clone();
        add.connect_clicked(move |_| {
            let kind = effects::BUILTIN[dd.selected() as usize % effects::BUILTIN.len()].clone();
            let mut st = c.borrow_mut();
            if let Some(t) = target_of(&st) {
                let len = chain(&st.doc.project, t).map(|c| c.len()).unwrap_or(0);
                // dynamics and EQ before space effects
                let index = match kind {
                    EffectKind::Reverb | EffectKind::Delay | EffectKind::Limiter => len,
                    _ => chain(&st.doc.project, t)
                        .and_then(|c| c.iter().position(|s| matches!(s.kind, EffectKind::Reverb | EffectKind::Delay | EffectKind::Limiter)))
                        .unwrap_or(len),
                };
                let name = effects::spec(&kind).name;
                st.commit_ops(&format!("Ajout : {name}"), vec![Op::AddEffect { target: t, slot: EffectSlot::new(kind), index }]);
            }
        });
        let c = s.clone();
        volume.connect_value_changed(move |sl| {
            if updating() {
                return;
            }
            let mut st = c.borrow_mut();
            let db = slider_to_db(sl.value());
            match target_of(&st) {
                Some(Target::Track(id)) => st.commit_ops("Volume", vec![Op::SetTrack { id, field: TrackField::VolumeDb(db) }]),
                Some(Target::Master) => st.commit_ops("Volume master", vec![Op::SetProject { field: studio_core::ops::ProjectField::MasterVolumeDb(db) }]),
                None => {}
            }
        });
        let c = s.clone();
        pan.connect_value_changed(move |sl| {
            if updating() {
                return;
            }
            let mut st = c.borrow_mut();
            if let Some(Target::Track(id)) = target_of(&st) {
                st.commit_ops("Panoramique", vec![Op::SetTrack { id, field: TrackField::Pan((sl.value() * 20.0).round() / 20.0) }]);
            }
        });
        panel
    }

    fn rebuild(self: &EffectsPanel, s: &Studio) {
        super::clear(&self.list);
        super::clear(&self.quick);
        self.controls.borrow_mut().clear();
        let target = target_of(s);
        self.target_switch.set_selected(if s.show_master_chain { 1 } else { 0 });
        let Some(target) = target else {
            self.title.set_text("Effets");
            self.track_row.set_visible(false);
            let l = label("Sélectionne une piste dans la timeline pour voir et régler ses effets.", Typography::Muted);
            l.set_wrap(true);
            self.list.append(&l);
            return;
        };
        self.track_row.set_visible(true);
        let p = &s.doc.project;
        let (name, kind) = match target {
            Target::Master => ("Master".to_string(), None),
            Target::Track(id) => p.track(id).map(|t| (t.name.clone(), Some(t.kind))).unwrap_or_default(),
        };
        self.title.set_text(&name);
        self.pan.set_visible(kind.is_some());

        // quick AI actions
        let quick: Vec<(&str, String)> = match kind {
            Some(TrackKind::Instrumental) => vec![("Faire de la place aux voix", format!("rends l'instru {name} moins envahissant : baisse-le de 3 dB"))],
            Some(_) => vec![
                ("Propre", format!("rends la voix {name} plus propre")),
                ("Mélodique", format!("rendu mélodique sur {name}")),
                ("Autotune", format!("autotune bien calibré sur {name}")),
                ("Agressive", format!("rendu agressif sur {name}")),
            ],
            None => vec![("Mastering auto", "fais-moi un mastering propre".into()), ("Fort (club)", "mastering fort".into())],
        };
        let ai = awra::Image::from_icon_name("awra-studio-ai-symbolic");
        ai.set_tooltip_text(Some("Chaînes préparées par l'ingénieur IA (annulables)"));
        self.quick.append(&ai);
        for (lbl, req) in quick {
            let b = button(lbl, None, ButtonAppearance::Secondary);
            let c = self.s.clone();
            b.connect_clicked(move |_| c.borrow_mut().ask_ai_local(&req));
            self.quick.append(&b);
        }

        let ch = chain(p, target).cloned().unwrap_or_default();
        if ch.is_empty() {
            let l = label(
                if kind.is_some() {
                    "Aucun effet. Choisis une chaîne ci-dessus, demande à l'IA, ou ajoute un effet."
                } else {
                    "Aucun traitement sur le master. « Mastering auto » prépare un son prêt à sortir."
                },
                Typography::Muted,
            );
            l.set_wrap(true);
            self.list.append(&l);
        }
        for (i, slot) in ch.iter().enumerate() {
            self.list.append(&self.card(s, target, slot, i, ch.len()));
        }
    }

    fn card(&self, s: &Studio, target: Target, slot: &EffectSlot, index: usize, len: usize) -> awra::Card {
        let card = awra::Card::new();
        let col = awra::Box::new(Orientation::Vertical, 6);
        let head = awra::Box::new(Orientation::Horizontal, 6);
        let sw = awra::Switch::new();
        sw.set_active(slot.enabled);
        sw.set_valign(awra::Align::Center);
        let name = label(&effects::display_name(&slot.kind), Typography::Body);
        name.set_weight(600);
        head.append(&sw);
        head.append(&name);
        if slot.added_by == Actor::Ai {
            let b = awra::Badge::new("IA");
            b.set_appearance(awra::BadgeAppearance::Accent);
            b.set_tooltip_text(Some("Ajouté par l'ingénieur IA"));
            head.append(&b);
        }
        let sp = awra::Spacer::new();
        sp.set_hexpand(true);
        head.append(&sp);
        if let Some(m) = s.engine.shared.meters.try_lock() {
            if let Some((_, gr)) = m.reductions.iter().find(|r| r.0 == slot.id) {
                head.append(&label(&format!("−{gr:.1} dB"), Typography::Label));
            }
        }
        let up = button("", Some("go-up-symbolic"), ButtonAppearance::Ghost);
        up.set_sensitive(index > 0);
        let down = button("", Some("go-down-symbolic"), ButtonAppearance::Ghost);
        down.set_sensitive(index + 1 < len);
        let more = awra::ToggleButton::new();
        more.set_icon_name("view-more-symbolic");
        more.set_tooltip_text(Some("Tous les réglages"));
        more.set_appearance(ButtonAppearance::Ghost);
        let del = button("", Some("user-trash-symbolic"), ButtonAppearance::Ghost);
        del.set_tooltip_text(Some("Retirer"));
        for w in [super::wd(&up), super::wd(&down), super::wd(&more), super::wd(&del)] {
            head.append(w);
        }
        col.append(&head);
        let sid = slot.id;
        let c = self.s.clone();
        sw.connect_active_notify(move |w| {
            if updating() {
                return;
            }
            c.borrow_mut().commit_ops("Effet activé/désactivé", vec![Op::SetEffectEnabled { target, slot: sid, enabled: w.is_active() }]);
        });
        let c = self.s.clone();
        up.connect_clicked(move |_| c.borrow_mut().commit_ops("Ordre des effets", vec![Op::MoveEffect { target, slot: sid, index: index.saturating_sub(1) }]));
        let c = self.s.clone();
        down.connect_clicked(move |_| c.borrow_mut().commit_ops("Ordre des effets", vec![Op::MoveEffect { target, slot: sid, index: index + 1 }]));
        let c = self.s.clone();
        del.connect_clicked(move |_| c.borrow_mut().commit_ops("Effet retiré", vec![Op::RemoveEffect { target, slot: sid }]));

        let grid = awra::Grid::new();
        grid.set_column_spacing(10);
        grid.set_row_spacing(4);
        let show_all = self.advanced.borrow().contains(&slot.id);
        more.set_active(show_all);
        let adv = self.advanced.clone();
        let c = self.s.clone();
        more.connect_toggled(move |b| {
            if updating() {
                return;
            }
            let mut a = adv.borrow_mut();
            if b.is_active() {
                a.push(sid);
            } else {
                a.retain(|x| *x != sid);
            }
            c.borrow_mut().ui_revision += 1;
        });
        let mut row = 0;
        if let EffectKind::Plugin { .. } = &slot.kind {
            match s.plugins.instance(slot.id) {
                Some(inst) => {
                    let params = inst.params();
                    let shown = if show_all { params.len() } else { params.len().min(8) };
                    for p in params.iter().take(shown) {
                        let v = slot.params.get(&p.id).copied().unwrap_or(p.value);
                        let sl = awra::Slider::with_range(Orientation::Horizontal, 0.0, 1.0, 0.001);
                        sl.set_draw_value(false);
                        sl.set_hexpand(true);
                        let span = (p.max - p.min).max(1e-9);
                        sl.set_value((v - p.min) / span);
                        let val = label(&inst.value_text(&p.id, v).unwrap_or(format!("{v:.2}")), Typography::Label);
                        val.set_width_chars(9);
                        grid.attach(&label(&p.name, Typography::Label), 0, row, 1, 1);
                        grid.attach(&sl, 1, row, 1, 1);
                        grid.attach(&val, 2, row, 1, 1);
                        let (c, pid, min, inst2, val2) = (self.s.clone(), p.id.clone(), p.min, inst.clone(), val.clone());
                        sl.connect_value_changed(move |w| {
                            if updating() {
                                return;
                            }
                            let v = min + w.value() * span;
                            val2.set_text(&inst2.value_text(&pid, v).unwrap_or(format!("{v:.2}")));
                            c.borrow_mut().commit_ops("Réglage du plugin", vec![Op::SetParam { target, slot: sid, param: pid.clone(), value: v }]);
                        });
                        self.controls.borrow_mut().insert((sid, p.id.clone()), Control::Plugin(sl, val, p.min, span));
                        row += 1;
                    }
                    if inst.has_gui() {
                        let open = button("Ouvrir l'interface du plugin", Some("view-reveal-symbolic"), ButtonAppearance::Secondary);
                        let (i2, c) = (inst.clone(), self.s.clone());
                        open.connect_clicked(move |_| {
                            if let Err(e) = i2.open_gui() {
                                c.borrow_mut().toasts.push(e.to_string());
                            }
                        });
                        grid.attach(&open, 0, row, 3, 1);
                    }
                }
                None => {
                    grid.attach(&label("Plugin introuvable sur cette machine (il est ignoré à la lecture).", Typography::Muted), 0, 0, 3, 1);
                }
            }
        } else {
            for p in effects::spec(&slot.kind).params.iter().filter(|p| show_all || p.essential) {
                let v = slot.param(p.id);
                let name = label(p.label, Typography::Label);
                grid.attach(&name, 0, row, 1, 1);
                let pid = p.id.to_string();
                let key = (sid, pid.clone());
                match p.unit {
                    Unit::Choice => {
                        let dd = awra::Dropdown::from_strings(p.choices);
                        dd.set_selected(v.round() as u32);
                        dd.set_hexpand(true);
                        let c = self.s.clone();
                        dd.connect_selected_notify(move |d| {
                            if updating() {
                                return;
                            }
                            c.borrow_mut().commit_ops("Réglage", vec![Op::SetParam { target, slot: sid, param: pid.clone(), value: d.selected() as f64 }]);
                        });
                        grid.attach(&dd, 1, row, 2, 1);
                        self.controls.borrow_mut().insert(key, Control::Choice(dd));
                    }
                    Unit::Toggle => {
                        let sw = awra::Switch::new();
                        sw.set_active(v >= 0.5);
                        sw.set_halign(awra::Align::Start);
                        let c = self.s.clone();
                        sw.connect_active_notify(move |w| {
                            if updating() {
                                return;
                            }
                            c.borrow_mut().commit_ops("Réglage", vec![Op::SetParam { target, slot: sid, param: pid.clone(), value: if w.is_active() { 1.0 } else { 0.0 } }]);
                        });
                        grid.attach(&sw, 1, row, 2, 1);
                        self.controls.borrow_mut().insert(key, Control::Toggle(sw));
                    }
                    _ => {
                        let sl = awra::Slider::with_range(Orientation::Horizontal, 0.0, 1.0, 0.001);
                        sl.set_draw_value(false);
                        sl.set_hexpand(true);
                        sl.set_value(p.to_norm(v));
                        let val = label(&p.format(v), Typography::Label);
                        val.set_width_chars(9);
                        val.set_tabular_digits(true);
                        let (c, spec, val2) = (self.s.clone(), *p, val.clone());
                        sl.connect_value_changed(move |w| {
                            if updating() {
                                return;
                            }
                            let v = spec.from_norm(w.value());
                            val2.set_text(&spec.format(v));
                            c.borrow_mut().commit_ops("Réglage", vec![Op::SetParam { target, slot: sid, param: pid.clone(), value: v }]);
                        });
                        grid.attach(&sl, 1, row, 1, 1);
                        grid.attach(&val, 2, row, 1, 1);
                        self.controls.borrow_mut().insert(key, Control::Slider(sl, val, *p));
                    }
                }
                row += 1;
            }
        }
        col.append(&grid);
        let desc = label(effects::spec(&slot.kind).description, Typography::Muted);
        desc.set_wrap(true);
        if show_all {
            col.append(&desc);
        }
        card.set_child(Some(&col));
        let _ = &self.window;
        card
    }

    fn update_values(&self, s: &Studio) {
        let Some(t) = target_of(s) else { return };
        let ch = chain(&s.doc.project, t).cloned().unwrap_or_default();
        let controls = self.controls.borrow();
        for slot in &ch {
            for ((sid, pid), ctl) in controls.iter() {
                if *sid != slot.id {
                    continue;
                }
                match ctl {
                    Control::Slider(sl, val, spec) => {
                        let v = slot.param(pid);
                        if (spec.from_norm(sl.value()) - v).abs() > 1e-6 * (1.0 + v.abs()) {
                            sl.set_value(spec.to_norm(v));
                            val.set_text(&spec.format(v));
                        }
                    }
                    Control::Choice(dd) => {
                        let v = slot.param(pid).round() as u32;
                        if dd.selected() != v {
                            dd.set_selected(v);
                        }
                    }
                    Control::Toggle(sw) => sw.set_active(slot.param(pid) >= 0.5),
                    Control::Plugin(sl, _, min, span) => {
                        if let Some(v) = slot.params.get(pid) {
                            let n = (v - min) / span;
                            if (sl.value() - n).abs() > 1e-6 {
                                sl.set_value(n);
                            }
                        }
                    }
                }
            }
        }
        let (db, pan) = match t {
            Target::Master => (s.doc.project.master.volume_db, 0.0),
            Target::Track(id) => s.doc.project.track(id).map(|t| (t.volume_db, t.pan)).unwrap_or_default(),
        };
        if (slider_to_db(self.volume.value()) - db).abs() > 0.05 {
            self.volume.set_value(db_to_slider(db));
        }
        self.volume_label.set_text(&format!("{db:+.1} dB"));
        if (self.pan.value() - pan).abs() > 0.01 {
            self.pan.set_value(pan);
        }
    }
}

/// Fader law: 0..1 → -60..+6 dB (more resolution near 0 dB).
pub fn slider_to_db(v: f64) -> f64 {
    let v = v.clamp(0.0, 1.0);
    if v <= 0.0 {
        -96.0
    } else {
        ((v.powf(0.5) * 66.0 - 60.0) * 10.0).round() / 10.0
    }
}

pub fn db_to_slider(db: f64) -> f64 {
    if db <= -60.0 {
        0.0
    } else {
        ((db + 60.0) / 66.0).clamp(0.0, 1.0).powi(2)
    }
}

impl Panel for EffectsPanel {
    fn widget(&self) -> Widget {
        self.root.clone().upcast::<Widget>()
    }

    fn refresh(&self, s: &Studio) {
        let sig = format!("{}|{:?}", signature(s), self.advanced.borrow());
        if *self.signature.borrow() != sig {
            *self.signature.borrow_mut() = sig;
            self.rebuild(s);
        }
        self.update_values(s);
    }
}
