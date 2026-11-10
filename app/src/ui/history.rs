//! What was done, by whom (you, a collaborator, the AI), undoable.

use std::cell::RefCell;
use std::rc::Rc;

use awra::prelude::*;
use awra::{ButtonAppearance, Orientation, Typography, Widget};
use studio_core::model::Actor;

use super::{button, label, Panel, S};
use crate::studio::Studio;

pub struct HistoryPanel {
    root: awra::Box,
    list: awra::Box,
    signature: RefCell<(usize, usize)>,
    s: S,
}

impl HistoryPanel {
    pub fn new(s: &S) -> Rc<HistoryPanel> {
        let root = awra::Box::new(Orientation::Vertical, 8);
        root.set_margins(12, 14, 12, 14);
        let top = awra::Box::new(Orientation::Horizontal, 6);
        let t = label("Historique", Typography::Title3);
        t.set_hexpand(true);
        top.append(&t);
        let undo_ai = button("Annuler la dernière action de l'IA", Some("awra-studio-ai-symbolic"), ButtonAppearance::Ghost);
        let c = s.clone();
        undo_ai.connect_clicked(move |_| {
            let mut st = c.borrow_mut();
            match st.doc.undo_last_by(&Actor::Ai) {
                Some(t) => {
                    st.toasts.push(format!("Annulé : {}", t.label));
                    st.rebuild_graph();
                }
                None => st.toasts.push("Aucune action de l'IA à annuler".into()),
            }
        });
        top.append(&undo_ai);
        root.append(&top);
        let list = awra::Box::new(Orientation::Vertical, 2);
        root.append(&super::scrolled(&list));
        Rc::new(HistoryPanel { root, list, signature: RefCell::new((usize::MAX, 0)), s: s.clone() })
    }
}

impl Panel for HistoryPanel {
    fn widget(&self) -> Widget {
        self.root.clone().upcast::<Widget>()
    }

    fn refresh(&self, s: &Studio) {
        let sig = (s.doc.history.undo.len(), s.doc.history.redo.len());
        if *self.signature.borrow() == sig {
            return;
        }
        *self.signature.borrow_mut() = sig;
        self.list.remove_all();
        for e in s.doc.history.redo.iter() {
            let l = label(&format!("↷ {}", e.forward.label), Typography::Muted);
            self.list.append(&l);
        }
        for e in s.doc.history.undo.iter().rev().take(80) {
            let row = awra::Box::new(Orientation::Horizontal, 6);
            let who = match &e.forward.actor {
                Actor::User => "Toi".to_string(),
                Actor::Ai => "IA".to_string(),
                Actor::Peer(n) => n.clone(),
            };
            let b = awra::Badge::new(&who);
            if e.forward.actor == Actor::Ai {
                b.set_appearance(awra::BadgeAppearance::Accent);
            }
            row.append(&b);
            let l = label(&e.forward.label, Typography::Label);
            l.set_hexpand(true);
            l.set_ellipsize(awra::pango::EllipsizeMode::End);
            row.append(&l);
            let undo = button("", Some("edit-undo-symbolic"), ButtonAppearance::Ghost);
            undo.set_tooltip_text(Some("Annuler cette action seulement"));
            let (c, id) = (self.s.clone(), e.forward.id);
            undo.connect_clicked(move |_| {
                if !c.borrow_mut().undo_transaction(id) {
                    c.borrow_mut().toasts.push("Impossible : des changements plus récents en dépendent".into());
                }
            });
            row.append(&undo);
            self.list.append(&row);
        }
    }
}
