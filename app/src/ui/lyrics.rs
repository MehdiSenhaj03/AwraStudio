//! The lyrics notebook: sections (verses, chorus…) written together,
//! linked to the timeline.

use std::cell::RefCell;
use std::rc::Rc;

use awra::prelude::*;
use awra::{ButtonAppearance, Orientation, Typography, Widget};
use studio_core::model::*;
use studio_core::ops::Op;

use super::{button, label, updating, Panel, S};
use crate::studio::Studio;

struct SectionUi {
    id: Id,
    title: awra::Entry,
    text: awra::TextView,
}

pub struct LyricsPanel {
    root: awra::Box,
    list: awra::Box,
    sections: RefCell<Vec<SectionUi>>,
    signature: RefCell<String>,
    s: S,
}

const TITLES: [&str; 6] = ["Intro", "Couplet", "Pré-refrain", "Refrain", "Pont", "Outro"];

fn set_section(s: &S, id: Id, title: Option<String>, text: Option<String>, start: Option<Option<f64>>) {
    let mut st = s.borrow_mut();
    let Some(sec) = st.doc.project.lyrics.iter().find(|x| x.id == id).cloned() else { return };
    let op = Op::SetSection {
        id,
        title: title.unwrap_or(sec.title),
        text: text.unwrap_or(sec.text),
        start: start.unwrap_or(sec.start),
    };
    st.commit_ops("Paroles", vec![op]);
}

impl LyricsPanel {
    pub fn new(s: &S) -> Rc<LyricsPanel> {
        let root = awra::Box::new(Orientation::Vertical, 8);
        root.set_margins(10, 12, 10, 12);
        let top = awra::Box::new(Orientation::Horizontal, 6);
        let t = label("Paroles", Typography::Title3);
        t.set_hexpand(true);
        top.append(&t);
        let dd = awra::Dropdown::from_strings(&TITLES);
        dd.set_selected(1);
        let add = button("Section", Some("list-add-symbolic"), ButtonAppearance::Secondary);
        top.append(&dd);
        top.append(&add);
        root.append(&top);
        let list = awra::Box::new(Orientation::Vertical, 12);
        root.append(&super::scrolled(&list));
        let c = s.clone();
        add.connect_clicked(move |_| {
            let mut st = c.borrow_mut();
            let base = TITLES[dd.selected() as usize % TITLES.len()];
            let n = st.doc.project.lyrics.iter().filter(|l| l.title.starts_with(base)).count();
            let title = if matches!(base, "Couplet") { format!("{base} {}", n + 1) } else { base.to_string() };
            let index = st.doc.project.lyrics.len();
            let pos = st.engine.position();
            st.commit_ops(
                "Nouvelle section",
                vec![Op::AddSection { section: LyricSection { id: new_id(), title, text: String::new(), start: Some(pos) }, index }],
            );
        });
        Rc::new(LyricsPanel { root, list, sections: RefCell::new(Vec::new()), signature: RefCell::new(String::new()), s: s.clone() })
    }

    fn rebuild(&self, s: &Studio) {
        super::clear(&self.list);
        self.sections.borrow_mut().clear();
        if s.doc.project.lyrics.is_empty() {
            let l = label("Écris ici tes couplets et refrains. Ils sont partagés avec les participants de la session et liés à la timeline.", Typography::Muted);
            l.set_wrap(true);
            self.list.append(&l);
        }
        for sec in &s.doc.project.lyrics {
            let card = awra::Card::new();
            let col = awra::Box::new(Orientation::Vertical, 6);
            let head = awra::Box::new(Orientation::Horizontal, 6);
            let title = awra::Entry::new();
            title.set_text(&sec.title);
            title.set_hexpand(true);
            title.set_has_frame(false);
            let when = label(&sec.start.map(super::fmt_time).unwrap_or_else(|| "—".into()), Typography::Label);
            let place = button("", Some("media-playlist-repeat-song-symbolic"), ButtonAppearance::Ghost);
            place.set_tooltip_text(Some("Placer à la tête de lecture"));
            let jump = button("", Some("media-playback-start-symbolic"), ButtonAppearance::Ghost);
            jump.set_tooltip_text(Some("Écouter depuis cette section"));
            let del = button("", Some("user-trash-symbolic"), ButtonAppearance::Ghost);
            for w in [super::wd(&title), super::wd(&when), super::wd(&place), super::wd(&jump), super::wd(&del)] {
                head.append(w);
            }
            let text = awra::TextView::new();
            text.set_text(&sec.text);
            text.set_wrap_mode(awra::WrapMode::Word);
            text.set_placeholder(Some("Tes paroles…"));
            text.set_size_request(-1, 90);
            col.append(&head);
            col.append(&text);
            card.set_child(Some(&col));
            self.list.append(&card);
            let id = sec.id;
            let c = self.s.clone();
            title.connect_changed(move |e| {
                if !updating() {
                    set_section(&c, id, Some(e.text()), None, None);
                }
            });
            let c = self.s.clone();
            text.connect_changed(move |t| {
                if !updating() {
                    set_section(&c, id, None, Some(t.text()), None);
                }
            });
            let c = self.s.clone();
            place.connect_clicked(move |_| {
                let pos = c.borrow().engine.position();
                set_section(&c, id, None, None, Some(Some(pos)));
            });
            let (c, start) = (self.s.clone(), sec.start);
            jump.connect_clicked(move |_| {
                let mut st = c.borrow_mut();
                st.seek(start.unwrap_or(0.0));
                if !st.engine.is_playing() {
                    st.play_pause();
                }
            });
            let c = self.s.clone();
            del.connect_clicked(move |_| c.borrow_mut().commit_ops("Section supprimée", vec![Op::RemoveSection { id }]));
            self.sections.borrow_mut().push(SectionUi { id, title, text });
        }
    }
}

impl Panel for LyricsPanel {
    fn widget(&self) -> Widget {
        self.root.clone().upcast::<Widget>()
    }

    fn refresh(&self, s: &Studio) {
        let sig: String = s.doc.project.lyrics.iter().map(|l| format!("{}:{:?};", l.id, l.start.map(|x| (x * 10.0) as i64))).collect();
        if *self.signature.borrow() != sig {
            *self.signature.borrow_mut() = sig;
            self.rebuild(s);
            return;
        }
        // others' edits (collaboration) without disturbing the one typing
        for ui in self.sections.borrow().iter() {
            if let Some(sec) = s.doc.project.lyrics.iter().find(|l| l.id == ui.id) {
                if !ui.title.has_focus() && ui.title.text() != sec.title {
                    ui.title.set_text(&sec.title);
                }
                if !ui.text.has_focus() && ui.text.text() != sec.text {
                    ui.text.set_text(&sec.text);
                }
            }
        }
    }
}
