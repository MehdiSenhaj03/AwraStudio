//! Mastering and export: finish the song in one click, export it.

use std::rc::Rc;

use awra::prelude::*;
use awra::{ButtonAppearance, LevelMeter, Orientation, Typography, Widget};
use studio_dsp::io::WavFormat;

use super::{button, label, section, Panel, S};
use crate::studio::Studio;

pub struct MasterPanel {
    root: awra::Box,
    chain: awra::Label,
    meter: LevelMeter,
    last: awra::Label,
    export: awra::Button,
}

const STYLES: [(&str, &str); 4] = [
    ("Streaming (équilibré)", "streaming"),
    ("Fort (club, rap)", "loud"),
    ("Dynamique (acoustique)", "dynamic"),
    ("Chaud", "warm"),
];

impl MasterPanel {
    pub fn new(s: &S, window: &awra::Window) -> Rc<MasterPanel> {
        let root = awra::Box::new(Orientation::Vertical, 12);
        root.set_margins(12, 14, 12, 14);
        root.append(&label("Finir le son", Typography::Title3));

        let finish = section("En un clic");
        let fin = button("Finaliser le morceau", Some("awra-studio-ai-symbolic"), ButtonAppearance::Primary);
        fin.set_tooltip_text(Some("Voix traitées, place dans l'instru, équilibre, mastering : tout est annulable"));
        let c = s.clone();
        fin.connect_clicked(move |_| c.borrow_mut().ask_ai_local("Finalise le son"));
        finish.append(&fin);
        let expl = label("L'IA traite les voix, fait de la place dans l'instru, équilibre les volumes puis calibre le mastering. Tu peux tout annuler ou ajuster ensuite.", Typography::Muted);
        expl.set_wrap(true);
        finish.append(&expl);
        root.append(&finish);

        let m = section("Mastering");
        let row = awra::Box::new(Orientation::Horizontal, 6);
        let names: Vec<&str> = STYLES.iter().map(|x| x.0).collect();
        let style = awra::Dropdown::from_strings(&names);
        style.set_hexpand(true);
        let go = button("Mastering auto", None, ButtonAppearance::Secondary);
        row.append(&style);
        row.append(&go);
        m.append(&row);
        let c = s.clone();
        go.connect_clicked(move |_| {
            let st = STYLES[style.selected() as usize % STYLES.len()].1;
            let req = match st {
                "loud" => "mastering fort",
                "dynamic" => "mastering dynamique",
                "warm" => "mastering chaud",
                _ => "mastering propre",
            };
            c.borrow_mut().ask_ai_local(req);
        });
        let chain = label("", Typography::Muted);
        chain.set_wrap(true);
        m.append(&chain);
        let edit = button("Régler la chaîne du master", Some("awra-studio-mixer-symbolic"), ButtonAppearance::Ghost);
        let c = s.clone();
        edit.connect_clicked(move |_| {
            let mut st = c.borrow_mut();
            st.show_master_chain = true;
            st.ui_revision += 1;
        });
        m.append(&edit);
        let meter = LevelMeter::new(Orientation::Horizontal);
        meter.as_ref().set_size_request(-1, 12);
        m.append(&meter);
        root.append(&m);

        let e = section("Export");
        let fmt = awra::Dropdown::from_strings(&["WAV 24 bits (studio)", "WAV 16 bits (CD, plateformes)", "WAV 32 bits flottant"]);
        e.append(&fmt);
        let export = button("Exporter le morceau…", Some("awra-studio-export-symbolic"), ButtonAppearance::Primary);
        e.append(&export);
        let last = label("", Typography::Muted);
        last.set_wrap(true);
        e.append(&last);
        root.append(&e);
        let c = s.clone();
        let w = window.clone();
        export.connect_clicked(move |_| {
            let name = super::header::sanitize(&c.borrow().doc.project.name);
            let f = match fmt.selected() {
                1 => WavFormat::Pcm16,
                2 => WavFormat::Float32,
                _ => WavFormat::Pcm24,
            };
            let filter = awra::FileFilter::new();
            filter.set_name(Some("WAV"));
            filter.add_suffix("wav");
            let dlg = awra::FileDialog::builder().title("Exporter le morceau").filter(&filter).initial_name(&format!("{name}.wav")).build();
            let c2 = c.clone();
            dlg.save(Some(&w), None, move |r| {
                if let Ok(file) = r {
                    if let Some(mut p) = file.path() {
                        if p.extension().is_none() {
                            p.set_extension("wav");
                        }
                        c2.borrow_mut().export(p, f);
                    }
                }
            });
        });
        Rc::new(MasterPanel { root, chain, meter, last, export })
    }
}

impl Panel for MasterPanel {
    fn widget(&self) -> Widget {
        self.root.clone().upcast::<Widget>()
    }

    fn refresh(&self, s: &Studio) {
        let ch = &s.doc.project.master.effects;
        self.chain.set_text(&if ch.is_empty() {
            "Aucun traitement de mastering pour l'instant.".to_string()
        } else {
            format!("Chaîne : {}", ch.iter().map(|e| studio_core::effects::display_name(&e.kind)).collect::<Vec<_>>().join(" → "))
        });
        self.last.set_text(&match &s.last_export {
            Some((p, lufs, tp)) => format!("Dernier export : {} — {lufs:.1} LUFS, crête {tp:.1} dBTP", p.display()),
            None => "Le morceau est rendu tel que tu l'entends (effets et plugins compris).".into(),
        });
        self.export.set_busy(s.export_busy);
    }

    fn tick(&self, s: &Studio) {
        if let Some(m) = s.engine.shared.meters.try_lock() {
            self.meter.set_levels(m.master.0, m.master.1);
            let gr = m.reductions.iter().filter(|r| s.doc.project.master.effects.iter().any(|e| e.id == r.0)).map(|r| r.1).fold(0.0f32, f32::max);
            self.meter.set_reduction(gr);
        }
    }
}
