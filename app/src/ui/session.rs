//! Making music together: host or join a session, participants, chat.

use std::cell::RefCell;
use std::rc::Rc;

use awra::prelude::*;
use awra::{ButtonAppearance, Orientation, Typography, Widget};

use super::{button, label, section, updating, Panel, S};
use crate::studio::Studio;

pub struct SessionPanel {
    root: awra::Box,
    content: awra::Box,
    chat_box: RefCell<Option<awra::Box>>,
    signature: RefCell<String>,
    s: S,
}

impl SessionPanel {
    pub fn new(s: &S) -> Rc<SessionPanel> {
        let root = awra::Box::new(Orientation::Vertical, 10);
        root.set_margins(12, 14, 12, 14);
        root.append(&label("Session à plusieurs", Typography::Title3));
        let content = awra::Box::new(Orientation::Vertical, 12);
        root.append(&super::scrolled(&content));
        Rc::new(SessionPanel { root, content, chat_box: RefCell::new(None), signature: RefCell::new(String::new()), s: s.clone() })
    }

    fn build_idle(&self, s: &Studio) {
        let intro = label(
            "Faites de la musique à plusieurs, chacun chez soi, sans serveur : l'un héberge, les autres rejoignent avec le code. \
             Pistes, prises, paroles, lecture et réglages se synchronisent ; les fichiers restent sur vos machines.",
            Typography::Muted,
        );
        intro.set_wrap(true);
        self.content.append(&intro);
        let name_row = awra::Box::new(Orientation::Horizontal, 6);
        name_row.append(&label("Ton nom", Typography::Label));
        let name = awra::Entry::new();
        name.set_text(&s.settings.user_name);
        name.set_hexpand(true);
        name_row.append(&name);
        self.content.append(&name_row);
        let c = self.s.clone();
        name.connect_changed(move |e| {
            if !updating() {
                c.borrow_mut().settings.user_name = e.text();
            }
        });

        let h = section("Héberger");
        let host = button("Ouvrir une session avec ce projet", Some("awra-studio-people-symbolic"), ButtonAppearance::Primary);
        let c = self.s.clone();
        host.connect_clicked(move |_| c.borrow_mut().host_session());
        h.append(&host);
        self.content.append(&h);

        let j = section("Rejoindre");
        let code = awra::Entry::new();
        code.set_placeholder_text(Some("Colle le code d'invitation (AWRA-…)"));
        let join = button("Rejoindre", None, ButtonAppearance::Secondary);
        let c = self.s.clone();
        let code2 = code.clone();
        join.connect_clicked(move |_| {
            let t = code2.text();
            c.borrow_mut().join_session(&t);
        });
        let row = awra::Box::new(Orientation::Horizontal, 6);
        code.set_hexpand(true);
        row.append(&code);
        row.append(&join);
        j.append(&row);
        let lan = button("Chercher sur le réseau local", Some("network-wireless-symbolic"), ButtonAppearance::Ghost);
        let found = awra::Box::new(Orientation::Vertical, 4);
        let f2 = found.clone();
        lan.connect_clicked(move |b| {
            b.set_busy(true);
            let list = studio_collab::discovery::discover(std::time::Duration::from_millis(2500));
            b.set_busy(false);
            f2.remove_all();
            if list.is_empty() {
                f2.append(&label("Aucune session trouvée sur ce réseau.", Typography::Muted));
            }
            for f in list {
                f2.append(&label(&format!("{} — {}:{} (demande-lui le code)", f.host, f.addr, f.port), Typography::Label));
            }
        });
        j.append(&lan);
        j.append(&found);
        self.content.append(&j);
    }

    fn build_active(&self, s: &Studio) {
        let sess = s.session.as_ref().unwrap();
        if let Some(code) = &sess.invite {
            let inv = section("Code d'invitation");
            let l = label(code, Typography::Monospace);
            l.set_selectable(true);
            l.set_wrap(true);
            l.set_wrap_mode(awra::WrapMode::Char);
            inv.append(&l);
            let copy = button("Copier le code", Some("edit-copy-symbolic"), ButtonAppearance::Secondary);
            let code = code.clone();
            let c = self.s.clone();
            copy.connect_clicked(move |b| {
                b.set_clipboard_text(&code);
                c.borrow_mut().toasts.push("Code copié : envoie-le à tes amis".into());
            });
            inv.append(&copy);
            let hint = label(
                if sess.upnp {
                    "Ta box a ouvert le port : le code marche aussi par Internet."
                } else {
                    "Sur le même réseau, le code marche tel quel. Par Internet, ouvre le port indiqué sur ta box (ou active l'UPnP)."
                },
                Typography::Muted,
            );
            hint.set_wrap(true);
            inv.append(&hint);
            self.content.append(&inv);
        }
        let p = section("Participants");
        for peer in &s.peers {
            let row = awra::Box::new(Orientation::Horizontal, 8);
            let av = awra::Avatar::new(28, Some(&peer.name));
            row.append(&av);
            row.append(&label(&peer.name, Typography::Body));
            let status = match peer.status.as_str() {
                "recording" => "● enregistre",
                "listening" => "écoute",
                _ => "",
            };
            if !status.is_empty() {
                let b = awra::Badge::new(status);
                b.set_appearance(if peer.status == "recording" { awra::BadgeAppearance::Danger } else { awra::BadgeAppearance::Neutral });
                row.append(&b);
            }
            p.append(&row);
        }
        self.content.append(&p);
        let sync = awra::CheckButton::with_label("Lecture synchronisée");
        sync.set_active(s.settings.sync_transport);
        let c = self.s.clone();
        sync.connect_toggled(move |b| {
            if !updating() {
                c.borrow_mut().settings.sync_transport = b.is_active();
            }
        });
        self.content.append(&sync);

        let chat = section("Discussion");
        let msgs = awra::Box::new(Orientation::Vertical, 4);
        chat.append(&msgs);
        let entry = awra::Entry::new();
        entry.set_placeholder_text(Some("Écris un message…"));
        let c = self.s.clone();
        entry.connect_activate(move |e| {
            let t = e.text();
            if !t.trim().is_empty() {
                e.set_text("");
                c.borrow_mut().send_chat(&t);
            }
        });
        chat.append(&entry);
        self.content.append(&chat);
        *self.chat_box.borrow_mut() = Some(msgs);
        let leave = button("Quitter la session", Some("window-close-symbolic"), ButtonAppearance::Destructive);
        let c = self.s.clone();
        leave.connect_clicked(move |_| c.borrow_mut().leave_session());
        self.content.append(&leave);
    }

    fn fill_chat(&self, s: &Studio) {
        if let Some(b) = self.chat_box.borrow().as_ref() {
            b.remove_all();
            for (who, text) in s.chat.iter().rev().take(40).collect::<Vec<_>>().into_iter().rev() {
                let l = label(&format!("{who} : {text}"), Typography::Body);
                l.set_wrap(true);
                b.append(&l);
            }
        }
    }
}

impl Panel for SessionPanel {
    fn widget(&self) -> Widget {
        self.root.clone().upcast::<Widget>()
    }

    fn refresh(&self, s: &Studio) {
        let sig = match &s.session {
            None => "idle".to_string(),
            Some(x) => format!("{:?}{}{:?}", x.role, x.invite.clone().unwrap_or_default(), s.peers),
        };
        if *self.signature.borrow() != sig {
            *self.signature.borrow_mut() = sig;
            self.content.remove_all();
            *self.chat_box.borrow_mut() = None;
            if s.session.is_some() {
                self.build_active(s);
            } else {
                self.build_idle(s);
            }
        }
        self.fill_chat(s);
    }
}
