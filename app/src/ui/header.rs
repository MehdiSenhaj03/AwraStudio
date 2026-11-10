//! The header: file menu, transport, interface menu, undo.

use std::rc::Rc;

use awra::prelude::*;
use awra::{ButtonAppearance, Dock, Orientation, Typography};

use super::{button, fmt_time, label, preset, S, PRESETS};
use crate::studio::Studio;

pub struct HeaderUi {
    pub header: awra::Header,
    play: awra::Button,
    record: awra::Button,
    time: awra::Label,
    info: awra::Label,
    metronome: awra::ToggleButton,
    undo: awra::Button,
    redo: awra::Button,
    session: awra::Button,
    window: awra::Window,
}

/// A menu entry: "" is a separator, "#Title" a section title; `check`
/// shows a tick column (Some(true) ticked).
struct Entry {
    text: String,
    key: Option<&'static str>,
    check: Option<bool>,
    f: Box<dyn Fn()>,
}

fn entry(text: &str, f: Box<dyn Fn()>) -> Entry {
    Entry { text: text.to_string(), key: None, check: None, f }
}

fn popover_menu(anchor: &awra::Button, items: Vec<Entry>) {
    let pop = awra::Popover::new();
    let col = awra::Box::new(Orientation::Vertical, 2);
    col.set_margins(6, 6, 6, 6);
    let pop2 = pop.clone();
    for Entry { text, key, check, f } in items {
        if text.is_empty() {
            col.append(&awra::Separator::new(Orientation::Horizontal));
            continue;
        }
        if let Some(t) = text.strip_prefix('#') {
            let l = super::label(t, awra::Typography::Eyebrow);
            l.set_xalign(0.0);
            l.set_margins(4, 10, 2, 10);
            col.append(&l);
            continue;
        }
        let b = match check {
            Some(on) => super::menu_check_item(&text, on),
            None => super::menu_item(&text, key),
        };
        let p = pop2.clone();
        b.connect_clicked(move |_| {
            p.popdown();
            f();
        });
        col.append(&b);
    }
    pop.set_child(Some(&col));
    pop.popup_pointing(anchor);
}

impl HeaderUi {
    pub fn new(s: &S, window: &awra::Window, dock: &Dock) -> Rc<HeaderUi> {
        let header = awra::Header::new();
        header.set_title(Some("Awra Studio"));

        let file = button("", Some("open-menu-symbolic"), ButtonAppearance::Ghost);
        file.set_tooltip_text(Some("Projet"));
        header.pack_start(&file);

        let transport = awra::Box::new(Orientation::Horizontal, 6);
        let to_start = button("", Some("media-skip-backward-symbolic"), ButtonAppearance::Ghost);
        to_start.set_tooltip_text(Some("Revenir au début (Début)"));
        let play = button("", Some("media-playback-start-symbolic"), ButtonAppearance::Primary);
        play.set_tooltip_text(Some("Lecture / pause (Espace)"));
        let record = button("", Some("awra-studio-record-symbolic"), ButtonAppearance::Secondary);
        record.set_tooltip_text(Some("Enregistrer sur la piste sélectionnée (R)"));
        let time = label("00:00.0", Typography::Title3);
        time.set_tabular_digits(true);
        time.set_width_chars(8);
        let info = label("", Typography::Muted);
        let metronome = awra::ToggleButton::new();
        metronome.set_icon_name("awra-studio-metronome-symbolic");
        metronome.set_tooltip_text(Some("Métronome"));
        metronome.set_appearance(ButtonAppearance::Ghost);
        for w in [super::wd(&to_start), super::wd(&play), super::wd(&record), super::wd(&time), super::wd(&info), super::wd(&metronome)] {
            transport.append(w);
        }
        header.set_center_widget(Some(&transport));

        let session = button("", Some("awra-studio-people-symbolic"), ButtonAppearance::Ghost);
        session.set_tooltip_text(Some("Session à plusieurs"));
        let layout = button("", Some("awra-studio-layout-symbolic"), ButtonAppearance::Ghost);
        layout.set_tooltip_text(Some("Interface : espaces de travail, panneaux, mode édition (Ctrl+E)"));
        let undo = button("", Some("edit-undo-symbolic"), ButtonAppearance::Ghost);
        undo.set_tooltip_text(Some("Annuler (Ctrl+Z)"));
        let redo = button("", Some("edit-redo-symbolic"), ButtonAppearance::Ghost);
        redo.set_tooltip_text(Some("Rétablir (Ctrl+Maj+Z)"));
        header.pack_end(&layout);
        header.pack_end(&session);
        header.pack_end(&redo);
        header.pack_end(&undo);

        let ui = Rc::new(HeaderUi {
            header,
            play: play.clone(),
            record: record.clone(),
            time,
            info,
            metronome: metronome.clone(),
            undo: undo.clone(),
            redo: redo.clone(),
            session: session.clone(),
            window: window.clone(),
        });

        let c = s.clone();
        to_start.connect_clicked(move |_| c.borrow_mut().seek(0.0));
        let c = s.clone();
        play.connect_clicked(move |_| c.borrow_mut().play_pause());
        let c = s.clone();
        record.connect_clicked(move |_| c.borrow_mut().toggle_record());
        let c = s.clone();
        metronome.connect_toggled(move |b| {
            if super::updating() {
                return;
            }
            let mut st = c.borrow_mut();
            st.settings.metronome = b.is_active();
            let on = b.is_active();
            st.engine.send(studio_engine::Cmd::Metronome(on));
        });
        let c = s.clone();
        undo.connect_clicked(move |_| c.borrow_mut().undo());
        let c = s.clone();
        redo.connect_clicked(move |_| c.borrow_mut().redo());
        let d = dock.clone();
        session.connect_clicked(move |_| {
            if d.is_shown("session") {
                d.hide("session");
            } else {
                d.move_panel("session", awra::DockArea::Right);
            }
        });
        let d = dock.clone();
        let c = s.clone();
        let l2 = layout.clone();
        let win = window.clone();
        layout.connect_clicked(move |_| {
            let mut items: Vec<Entry> = vec![entry("#Disposition", Box::new(|| {}))];
            for (key, name, _) in PRESETS.iter() {
                let d = d.clone();
                items.push(entry(name, Box::new(move || {
                    if let Some(p) = preset(key) {
                        d.set_layout(p);
                    }
                })));
            }
            items.push(entry("", Box::new(|| {})));
            let (c3, w3) = (c.clone(), win.clone());
            items.push(entry("Apparence et transparence…", Box::new(move || super::settings_dialog::open_appearance(&c3, &w3))));
            let d2 = d.clone();
            let c2 = c.clone();
            items.push(Entry {
                text: (if d.is_editing() { "Terminer la personnalisation" } else { "Personnaliser l'interface…" }).into(),
                key: Some("Ctrl+E"),
                check: None,
                f: Box::new(move || {
                    let on = !d2.is_editing();
                    d2.set_editing(on);
                    if on {
                        c2.borrow_mut().toasts.push("Chaque panneau a maintenant une barre : déplace-le, agrandis-le ou masque-le".into());
                    }
                }),
            });
            items.push(entry("", Box::new(|| {})));
            items.push(entry("#Panneaux", Box::new(|| {})));
            for (id, title) in d.panel_ids() {
                let shown = d.is_shown(&id);
                let d3 = d.clone();
                items.push(Entry {
                    text: title,
                    key: None,
                    check: Some(shown),
                    f: Box::new(move || {
                        if d3.is_shown(&id) {
                            d3.hide(&id)
                        } else {
                            d3.show(&id)
                        }
                    }),
                });
            }
            popover_menu(&l2, items);
        });
        let ui2 = ui.clone();
        let c = s.clone();
        let f2 = file.clone();
        file.connect_clicked(move |_| {
            let mk = |f: Box<dyn Fn(&HeaderUi, &S)>| -> Box<dyn Fn()> {
                let ui = ui2.clone();
                let c = c.clone();
                Box::new(move || f(&ui, &c))
            };
            let k = |text: &str, key: Option<&'static str>, f: Box<dyn Fn()>| Entry { text: text.into(), key, check: None, f };
            let items = vec![
                k("Nouveau projet", Some("Ctrl+N"), mk(Box::new(|_, s| super::new_project(s)))),
                k("Ouvrir un projet…", Some("Ctrl+O"), mk(Box::new(|ui, s| ui.open(s)))),
                k("Projet de démonstration", None, mk(Box::new(|_, s| super::load_demo(s)))),
                k("", None, Box::new(|| {})),
                k("Enregistrer", Some("Ctrl+S"), mk(Box::new(|ui, s| ui.save(s, false)))),
                k("Enregistrer sous…", None, mk(Box::new(|ui, s| ui.save(s, true)))),
                k("", None, Box::new(|| {})),
                k("Importer de l'audio…", Some("Ctrl+I"), mk(Box::new(|ui, s| ui.import(s)))),
                k("Réglages…", Some("Ctrl+,"), mk(Box::new(|ui, s| super::settings_dialog::open(s, &ui.window)))),
            ];
            popover_menu(&f2, items);
        });
        ui
    }

    pub fn open(&self, s: &S) {
        let dlg = awra::FileDialog::builder().title("Ouvrir un projet Awra Studio (dossier .awra)").build();
        let s = s.clone();
        dlg.select_folder(Some(&self.window), None, move |r| {
            if let Ok(f) = r {
                if let Some(p) = f.path() {
                    match studio_core::Document::open(&p) {
                        Ok(doc) => {
                            let mut st = s.borrow_mut();
                            st.set_document(doc);
                            st.settings.add_recent(&p.to_string_lossy());
                        }
                        Err(e) => s.borrow_mut().toasts.push(format!("Ouverture impossible : {e}")),
                    }
                }
            }
        });
    }

    pub fn save(&self, s: &S, ask: bool) {
        let temp = s.borrow().doc.is_temporary();
        if !ask && !temp {
            let r = s.borrow_mut().save(None);
            let msg = match r {
                Ok(p) => format!("Enregistré dans {}", p.display()),
                Err(e) => format!("Enregistrement impossible : {e}"),
            };
            s.borrow_mut().toasts.push(msg);
            return;
        }
        let name = s.borrow().doc.project.name.clone();
        let dlg = awra::FileDialog::builder().title("Dossier où enregistrer le projet").build();
        let s = s.clone();
        dlg.select_folder(Some(&self.window), None, move |r| {
            if let Ok(f) = r {
                if let Some(p) = f.path() {
                    let dir = p.join(format!("{}.awra", sanitize(&name)));
                    let r = s.borrow_mut().save(Some(dir));
                    let msg = match r {
                        Ok(p) => format!("Enregistré dans {}", p.display()),
                        Err(e) => format!("Enregistrement impossible : {e}"),
                    };
                    s.borrow_mut().toasts.push(msg);
                }
            }
        });
    }

    pub fn import(&self, s: &S) {
        let filter = awra::FileFilter::new();
        filter.set_name(Some("Audio"));
        for e in studio_dsp::io::AUDIO_EXTENSIONS {
            filter.add_suffix(e);
        }
        let dlg = awra::FileDialog::builder().title("Importer de l'audio").filter(&filter).build();
        let s = s.clone();
        dlg.open_multiple(Some(&self.window), None, move |r| {
            if let Ok(files) = r {
                let paths: Vec<_> = files.iter().filter_map(|f| f.path()).collect();
                let sel = {
                    let st = s.borrow();
                    st.selected.filter(|_| !st.doc.project.tracks.is_empty() && false)
                };
                s.borrow_mut().import_files(&paths, sel, 0.0);
            }
        });
    }

    pub fn refresh(&self, s: &Studio) {
        let p = &s.doc.project;
        self.header.set_title(Some(&p.name));
        let state = if s.session.is_some() {
            format!("Session · {} participant(s)", s.peers.len().max(1))
        } else if s.doc.is_temporary() {
            "Non enregistré".to_string()
        } else if s.doc.dirty {
            "Modifié".to_string()
        } else {
            "Enregistré".to_string()
        };
        self.header.set_subtitle(Some(&state));
        let key = p.key.map(|k| format!(" · {} {}", studio_core::NOTE_NAMES_FR[k as usize % 12], if p.minor { "mineur" } else { "majeur" })).unwrap_or_default();
        self.info.set_text(&format!("{:.0} BPM{key}", p.bpm));
        self.undo.set_sensitive(s.doc.history.can_undo());
        self.redo.set_sensitive(s.doc.history.can_redo());
        self.metronome.set_active(s.settings.metronome);
        if s.session.is_some() {
            self.session.set_appearance(ButtonAppearance::Secondary);
        } else {
            self.session.set_appearance(ButtonAppearance::Ghost);
        }
        let _ = &self.window;
    }

    pub fn tick(&self, s: &Studio) {
        self.time.set_text(&fmt_time(s.engine.position()));
        let playing = s.engine.is_playing();
        self.play.set_icon_name(if playing { "media-playback-pause-symbolic" } else { "media-playback-start-symbolic" });
        self.record.set_appearance(if s.recording.is_some() { ButtonAppearance::Destructive } else { ButtonAppearance::Secondary });
        // a slow breath while recording
        let o = if s.recording.is_some() { 0.78 + 0.22 * (awra::now_ms() / 1000.0 * std::f64::consts::PI * 1.4).cos() } else { 1.0 };
        if (self.record.opacity() - o).abs() > 0.01 {
            self.record.set_opacity(o);
        }
    }
}

pub fn sanitize(name: &str) -> String {
    name.chars().map(|c| if c.is_alphanumeric() || c == ' ' || c == '-' || c == '_' { c } else { '_' }).collect::<String>().trim().to_string()
}
