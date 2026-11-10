//! Files of the project and reference tracks.

use std::cell::RefCell;
use std::rc::Rc;

use awra::prelude::*;
use awra::{ButtonAppearance, Orientation, Typography, Widget};
use studio_core::model::*;
use studio_core::ops::Op;

use super::{button, label, section, Panel, S};
use crate::studio::Studio;

pub struct BrowserPanel {
    root: awra::Box,
    files: awra::Box,
    refs: awra::Box,
    signature: RefCell<String>,
    s: S,
}

impl BrowserPanel {
    pub fn new(s: &S, window: &awra::Window) -> Rc<BrowserPanel> {
        let root = awra::Box::new(Orientation::Vertical, 10);
        root.set_margins(12, 14, 12, 14);
        let content = awra::Box::new(Orientation::Vertical, 14);
        let f = section("Fichiers du projet");
        let files = awra::Box::new(Orientation::Vertical, 4);
        f.append(&files);
        let import = button("Importer de l'audio…", Some("document-open-symbolic"), ButtonAppearance::Secondary);
        f.append(&import);
        content.append(&f);
        let r = section("Morceaux de référence");
        let expl = label(
            "Importe un son dont tu aimes le rendu, puis demande « inspire-toi de la référence » : l'IA en reprend l'autotune, la réverbe, l'ambiance et le niveau.",
            Typography::Muted,
        );
        expl.set_wrap(true);
        r.append(&expl);
        let refs = awra::Box::new(Orientation::Vertical, 6);
        r.append(&refs);
        let add_ref = button("Ajouter une référence…", Some("list-add-symbolic"), ButtonAppearance::Secondary);
        r.append(&add_ref);
        content.append(&r);
        root.append(&super::scrolled(&content));

        let (c, w) = (s.clone(), window.clone());
        import.connect_clicked(move |_| {
            let dlg = awra::FileDialog::builder().title("Importer de l'audio").filter(&audio_filter()).build();
            let c2 = c.clone();
            dlg.open_multiple(Some(&w), None, move |r| {
                if let Ok(files) = r {
                    let paths: Vec<_> = files.iter().filter_map(|f| f.path()).collect();
                    c2.borrow_mut().import_files(&paths, None, 0.0);
                }
            });
        });
        let (c, w) = (s.clone(), window.clone());
        add_ref.connect_clicked(move |_| {
            let dlg = awra::FileDialog::builder().title("Morceau de référence").filter(&audio_filter()).build();
            let c2 = c.clone();
            dlg.open(Some(&w), None, move |r| {
                if let Ok(file) = r {
                    if let Some(p) = file.path() {
                        c2.borrow_mut().add_reference(&p);
                    }
                }
            });
        });
        Rc::new(BrowserPanel { root, files, refs, signature: RefCell::new(String::new()), s: s.clone() })
    }
}

pub fn audio_filter() -> awra::FileFilter {
    let filter = awra::FileFilter::new();
    filter.set_name(Some("Audio"));
    for e in studio_dsp::io::AUDIO_EXTENSIONS {
        filter.add_suffix(e);
    }
    filter
}

impl Panel for BrowserPanel {
    fn widget(&self) -> Widget {
        self.root.clone().upcast::<Widget>()
    }

    fn refresh(&self, s: &Studio) {
        let p = &s.doc.project;
        let sig = format!("{}|{}|{}", p.assets.len(), p.references.len(), s.selected.unwrap_or(0));
        if *self.signature.borrow() == sig {
            return;
        }
        *self.signature.borrow_mut() = sig;
        self.files.remove_all();
        let mut assets: Vec<&AssetInfo> = p.assets.values().filter(|a| a.kind != AssetKind::Reference).collect();
        assets.sort_by(|a, b| a.name.cmp(&b.name));
        if assets.is_empty() {
            self.files.append(&label("Aucun fichier. Glisse une instru dans la fenêtre.", Typography::Muted));
        }
        for a in assets {
            let row = awra::Box::new(Orientation::Horizontal, 6);
            let icon = awra::Image::from_icon_name(if a.kind == AssetKind::Take { "awra-studio-mic-symbolic" } else { "audio-x-generic-symbolic" });
            row.append(&icon);
            let l = label(&format!("{} · {}", a.name, super::fmt_time(a.duration)), Typography::Label);
            l.set_hexpand(true);
            l.set_ellipsize(awra::pango::EllipsizeMode::Middle);
            row.append(&l);
            let add = button("", Some("list-add-symbolic"), ButtonAppearance::Ghost);
            add.set_tooltip_text(Some("Ajouter à la piste sélectionnée, à la tête de lecture"));
            let (c, id, dur, name) = (self.s.clone(), a.id.clone(), a.duration, a.name.clone());
            add.connect_clicked(move |_| {
                let mut st = c.borrow_mut();
                let Some(track) = st.selected else {
                    st.toasts.push("Sélectionne d'abord une piste".into());
                    return;
                };
                let clip = Clip { id: new_id(), asset: id.clone(), name: name.clone(), start: st.engine.position(), offset: 0.0, length: dur, gain_db: 0.0, fade_in: 0.0, fade_out: 0.0, muted: false };
                st.commit_ops("Clip ajouté", vec![Op::AddClip { track, clip }]);
            });
            row.append(&add);
            self.files.append(&row);
        }
        self.refs.remove_all();
        for r in &p.references {
            let card = awra::Card::new();
            let col = awra::Box::new(Orientation::Vertical, 6);
            col.append(&label(&r.name, Typography::Body));
            let row = awra::Box::new(Orientation::Horizontal, 6);
            let inspire = button("S'en inspirer", Some("awra-studio-ai-symbolic"), ButtonAppearance::Primary);
            let name = r.name.clone();
            let c = self.s.clone();
            inspire.connect_clicked(move |_| c.borrow_mut().ask_ai_local(&format!("Inspire-toi de la référence {name} pour ma voix")));
            let analyse = button("Écouter l'analyse", None, ButtonAppearance::Ghost);
            let (c, name) = (self.s.clone(), r.name.clone());
            analyse.connect_clicked(move |_| c.borrow_mut().ask_ai(&format!("Analyse la référence « {name} » et dis-moi ce qui caractérise son son")));
            let del = button("", Some("user-trash-symbolic"), ButtonAppearance::Ghost);
            let (c, id) = (self.s.clone(), r.id);
            del.connect_clicked(move |_| c.borrow_mut().commit_ops("Référence retirée", vec![Op::RemoveReference { id }]));
            row.append(&inspire);
            row.append(&analyse);
            row.append(&del);
            col.append(&row);
            card.set_child(Some(&col));
            self.refs.append(&card);
        }
    }
}
