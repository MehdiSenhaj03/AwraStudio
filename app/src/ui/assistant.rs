//! The AI sound engineer: talk to it, it acts; every action can be undone.

use std::cell::RefCell;
use std::rc::Rc;

use awra::prelude::*;
use awra::{ButtonAppearance, Orientation, Typography, Widget};

use super::{button, label, updating, Panel, S};
use crate::studio::Studio;

pub struct AssistantPanel {
    root: awra::Box,
    provider: awra::Label,
    list: awra::Box,
    scroller: awra::ScrolledWindow,
    busy_row: awra::Box,
    busy_label: awra::Label,
    entry: awra::Entry,
    signature: RefCell<String>,
    s: S,
}

const SUGGESTIONS: [(&str, &str); 6] = [
    ("Voix plus propre", "Rends ma voix plus propre"),
    ("Autotune calé", "Mets-moi un autotune bien calibré"),
    ("Voix étouffée ?", "Ma voix est trop étouffée, corrige ça"),
    ("Mixer les voix", "Mixe nos voix pour qu'elles se complètent bien"),
    ("Variantes A/B", "Propose plusieurs variantes de mix"),
    ("Finaliser le son", "Finalise le son"),
];

impl AssistantPanel {
    pub fn new(s: &S, window: &awra::Window) -> Rc<AssistantPanel> {
        let root = awra::Box::new(Orientation::Vertical, 8);
        root.set_margins(10, 12, 10, 12);
        let top = awra::Box::new(Orientation::Horizontal, 6);
        let icon = awra::Image::from_icon_name("awra-studio-ai-symbolic");
        let provider = label("", Typography::Label);
        provider.set_hexpand(true);
        provider.set_ellipsize(awra::pango::EllipsizeMode::End);
        let settings = button("", Some("preferences-system-symbolic"), ButtonAppearance::Ghost);
        settings.set_tooltip_text(Some("Choisir le modèle : ChatGPT, Claude, local…"));
        top.append(&icon);
        top.append(&provider);
        top.append(&settings);
        root.append(&top);

        let list = awra::Box::new(Orientation::Vertical, 10);
        list.set_margin_end(6);
        let scroller = super::scrolled(&list);
        root.append(&scroller);

        let busy_row = awra::Box::new(Orientation::Horizontal, 8);
        let spinner = awra::Spinner::new();
        spinner.start();
        let busy_label = label("", Typography::Muted);
        busy_row.append(&spinner);
        busy_row.append(&busy_label);
        busy_row.set_visible(false);
        root.append(&busy_row);

        let chips = awra::FlowBox::new();
        chips.set_selection_mode(awra::SelectionMode::None);
        chips.set_min_children_per_line(1);
        chips.set_max_children_per_line(3);
        for (short, sgt) in SUGGESTIONS {
            let b = awra::Button::with_label(short);
            b.set_appearance(ButtonAppearance::Secondary);
            b.set_tooltip_text(Some(sgt));
            let c = s.clone();
            b.connect_clicked(move |_| c.borrow_mut().ask_ai(sgt));
            chips.append(&b);
        }
        root.append(&chips);

        let input = awra::Box::new(Orientation::Horizontal, 6);
        let entry = awra::Entry::new();
        entry.set_placeholder_text(Some("Demande ce que tu veux à ton ingénieur…"));
        entry.set_hexpand(true);
        let send = button("", Some("mail-send-symbolic"), ButtonAppearance::Primary);
        send.set_tooltip_text(Some("Envoyer"));
        input.append(&entry);
        input.append(&send);
        root.append(&input);

        let panel = Rc::new(AssistantPanel {
            root,
            provider,
            list,
            scroller,
            busy_row,
            busy_label,
            entry: entry.clone(),
            signature: RefCell::new(String::new()),
            s: s.clone(),
        });
        let go = {
            let c = s.clone();
            let e = entry.clone();
            move || {
                let t = e.text();
                if t.trim().is_empty() {
                    return;
                }
                e.set_text("");
                c.borrow_mut().ask_ai(&t);
            }
        };
        let g2 = go.clone();
        entry.connect_activate(move |_| g2());
        send.connect_clicked(move |_| go());
        let c = s.clone();
        let w = window.clone();
        settings.connect_clicked(move |_| super::settings_dialog::open(&c, &w));
        panel
    }

    fn rebuild(&self, s: &Studio) {
        super::clear(&self.list);
        for (i, e) in s.ai_entries.iter().enumerate() {
            let card = awra::Card::new();
            if e.user {
                card.set_appearance(awra::CardAppearance::Tinted);
                card.set_halign(awra::Align::End);
                card.set_margin_start(40);
            } else {
                card.set_margin_end(20);
            }
            let col = awra::Box::new(Orientation::Vertical, 6);
            let text = label(&e.text, Typography::Body);
            text.set_wrap(true);
            text.set_selectable(true);
            col.append(&text);
            if let Some(n) = &e.note {
                let l = label(n, Typography::Muted);
                l.set_wrap(true);
                col.append(&l);
            }
            for (id, lbl) in &e.actions {
                let row = awra::Box::new(Orientation::Horizontal, 6);
                let done = e.undone.contains(id);
                let l = label(&format!("{} {}", if done { "↶" } else { "✓" }, lbl), Typography::Label);
                l.set_hexpand(true);
                l.set_wrap(true);
                row.append(&l);
                let b = button(if done { "Annulé" } else { "Annuler" }, None, ButtonAppearance::Ghost);
                b.set_sensitive(!done);
                let (c, id) = (self.s.clone(), *id);
                b.connect_clicked(move |_| {
                    let ok = c.borrow_mut().undo_transaction(id);
                    if !ok {
                        c.borrow_mut().toasts.push("Impossible d'annuler : la piste a changé depuis".into());
                    }
                    c.borrow_mut().ui_revision += 1;
                });
                row.append(&b);
                col.append(&row);
            }
            if !e.user && e.actions.len() > 1 && e.undone.len() < e.actions.len() {
                let all = button("Tout annuler", Some("edit-undo-symbolic"), ButtonAppearance::Secondary);
                all.set_halign(awra::Align::Start);
                let (c, ids, idx) = (self.s.clone(), e.actions.iter().map(|a| a.0).collect::<Vec<_>>(), i);
                all.connect_clicked(move |_| {
                    let mut st = c.borrow_mut();
                    for id in ids.iter().rev() {
                        st.undo_transaction(*id);
                    }
                    let _ = idx;
                    st.ui_revision += 1;
                });
                col.append(&all);
            }
            card.set_child(Some(&col));
            self.list.append(&card);
        }
        // the newest message in view
        let sc = self.scroller.clone();
        awra::glib::timeout_add_local_once(std::time::Duration::from_millis(60), move || {
            sc.set_vvalue(1e9);
        });
    }
}

impl Panel for AssistantPanel {
    fn widget(&self) -> Widget {
        self.root.clone().upcast::<Widget>()
    }

    fn refresh(&self, s: &Studio) {
        let cfg = &s.settings.ai;
        let model = cfg.model();
        self.provider.set_text(&if model.is_empty() { cfg.kind.label().to_string() } else { format!("{} · {model}", cfg.kind.label()) });
        let sig = format!(
            "{}|{}|{}",
            s.ai_entries.len(),
            s.ai_entries.iter().map(|e| e.undone.len()).sum::<usize>(),
            s.ai_entries.last().map(|e| e.text.len()).unwrap_or(0)
        );
        if *self.signature.borrow() != sig {
            *self.signature.borrow_mut() = sig;
            self.rebuild(s);
        }
        let _ = updating();
        let _ = &self.entry;
    }

    fn tick(&self, s: &Studio) {
        match &s.ai_busy {
            Some(p) => {
                self.busy_row.set_visible(true);
                self.busy_label.set_text(p);
            }
            None => self.busy_row.set_visible(false),
        }
    }
}
