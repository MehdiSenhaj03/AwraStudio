//! VST3 and CLAP plugins installed on the machine.

use std::cell::RefCell;
use std::rc::Rc;

use awra::prelude::*;
use awra::{ButtonAppearance, Orientation, Typography, Widget};
use studio_core::effects::{EffectKind, PluginFormat};
use studio_core::model::*;
use studio_core::ops::{chain, Op, Target};

use super::{button, label, Panel, S};
use crate::studio::Studio;

pub struct PluginsPanel {
    root: awra::Box,
    list: awra::Box,
    scan: awra::Button,
    search: awra::SearchEntry,
    signature: RefCell<String>,
    s: S,
}

impl PluginsPanel {
    pub fn new(s: &S) -> Rc<PluginsPanel> {
        let root = awra::Box::new(Orientation::Vertical, 8);
        root.set_margins(12, 14, 12, 14);
        let top = awra::Box::new(Orientation::Horizontal, 6);
        let t = label("Plugins VST3 / CLAP", Typography::Title3);
        t.set_hexpand(true);
        let scan = button("Rechercher", Some("view-refresh-symbolic"), ButtonAppearance::Secondary);
        top.append(&t);
        top.append(&scan);
        root.append(&top);
        let search = awra::SearchEntry::new();
        search.set_placeholder_text(Some("Filtrer les plugins…"));
        root.append(&search);
        let list = awra::Box::new(Orientation::Vertical, 4);
        root.append(&super::scrolled(&list));
        let c = s.clone();
        scan.connect_clicked(move |_| c.borrow_mut().scan_plugins());
        let p = Rc::new(PluginsPanel { root, list, scan, search: search.clone(), signature: RefCell::new(String::new()), s: s.clone() });
        let c = s.clone();
        search.connect_search_changed(move |_| c.borrow_mut().ui_revision += 1);
        p
    }
}

impl Panel for PluginsPanel {
    fn widget(&self) -> Widget {
        self.root.clone().upcast::<Widget>()
    }

    fn refresh(&self, s: &Studio) {
        self.scan.set_busy(s.scan_busy);
        let q = self.search.text().to_lowercase();
        let sig = format!("{}|{q}", s.settings.plugins.len());
        if *self.signature.borrow() == sig {
            return;
        }
        *self.signature.borrow_mut() = sig;
        self.list.remove_all();
        if s.settings.plugins.is_empty() {
            let l = label(
                "Aucun plugin connu. « Rechercher » parcourt les dossiers standard (~/.vst3, ~/.clap, /usr/lib/vst3, Program Files\\Common Files\\VST3…).",
                Typography::Muted,
            );
            l.set_wrap(true);
            self.list.append(&l);
        }
        for p in s.settings.plugins.iter().filter(|p| q.is_empty() || p.name.to_lowercase().contains(&q) || p.vendor.to_lowercase().contains(&q)) {
            let row = awra::Box::new(Orientation::Horizontal, 6);
            let b = awra::Badge::new(if p.format == PluginFormat::Clap { "CLAP" } else { "VST3" });
            row.append(&b);
            let l = label(&format!("{} — {}", p.name, p.vendor), Typography::Label);
            l.set_hexpand(true);
            l.set_ellipsize(awra::pango::EllipsizeMode::End);
            row.append(&l);
            let add = button("", Some("list-add-symbolic"), ButtonAppearance::Ghost);
            add.set_tooltip_text(Some("Ajouter à la piste sélectionnée"));
            let (c, desc) = (self.s.clone(), p.clone());
            add.connect_clicked(move |_| {
                let mut st = c.borrow_mut();
                let target = if st.show_master_chain { Some(Target::Master) } else { st.selected.map(Target::Track) };
                let Some(t) = target else {
                    st.toasts.push("Sélectionne d'abord une piste".into());
                    return;
                };
                let kind = EffectKind::Plugin { format: desc.format, path: desc.path.clone(), plugin_id: desc.id.clone(), name: desc.name.clone() };
                let slot = EffectSlot { id: new_id(), kind, enabled: true, params: Default::default(), plugin_state: None, added_by: Actor::User };
                let index = chain(&st.doc.project, t).map(|c| c.len()).unwrap_or(0);
                st.commit_ops(&format!("Plugin {}", desc.name), vec![Op::AddEffect { target: t, slot, index }]);
            });
            row.append(&add);
            self.list.append(&row);
        }
    }
}
