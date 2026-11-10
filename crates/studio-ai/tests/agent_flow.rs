//! The local engineer driving a real document: render, analyze, act, undo.

use std::sync::Arc;

use studio_ai::host::{LayoutCommand, PluginDesc, StudioHost};
use studio_analysis::Analysis;
use studio_core::effects::EffectKind;
use studio_core::model::*;
use studio_core::ops::Transaction;
use studio_core::Document;
use studio_dsp::graph::{render_project, render_track, AssetStore};
use studio_dsp::AudioBuffer;

struct TestHost {
    doc: Document,
    assets: AssetStore,
    layouts: Vec<LayoutCommand>,
}

impl StudioHost for TestHost {
    fn project(&self) -> Project {
        self.doc.project.clone()
    }
    fn analyze_track(&mut self, track: Id, processed: bool) -> Result<Analysis, String> {
        let t = self.doc.project.track(track).ok_or("no track")?;
        let buf = render_track(&self.doc.project, &self.assets, track, processed, None).ok_or("empty")?;
        Ok(studio_analysis::analyze(&buf, t.kind != TrackKind::Instrumental))
    }
    fn analyze_mix(&mut self) -> Result<Analysis, String> {
        Ok(studio_analysis::analyze(&render_project(&self.doc.project, &self.assets, None), false))
    }
    fn analyze_reference(&mut self, _r: Id) -> Result<Analysis, String> {
        Err("none".into())
    }
    fn measure_lufs(&mut self, p: &Project) -> Result<f64, String> {
        let b = render_project(p, &self.assets, None);
        let (l, r) = b.lr();
        Ok(studio_dsp::loudness::integrated(&[l, r], b.sample_rate))
    }
    fn commit(&mut self, tx: Transaction) -> Result<Id, String> {
        let id = tx.id;
        self.doc.commit(tx).map_err(|e| e.to_string())?;
        Ok(id)
    }
    fn layout(&mut self, cmd: LayoutCommand) -> Result<String, String> {
        self.layouts.push(cmd);
        Ok("ok".into())
    }
    fn plugins(&self) -> Vec<PluginDesc> {
        Vec::new()
    }
    fn undo_ai(&mut self) -> Result<String, String> {
        self.doc.undo_last_by(&Actor::Ai).map(|t| t.label).ok_or("rien".into())
    }
}

fn host() -> TestHost {
    let sr = 48000u32;
    let n = sr as usize * 6;
    let mut seed = 3u32;
    let mut noise = move || {
        seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        (seed >> 8) as f32 / (1u32 << 24) as f32 - 0.5
    };
    // a "voice": phrases of a slightly flat A3 with harmonics, room noise
    let voice: Vec<f32> = (0..n)
        .map(|i| {
            let t = i as f32 / sr as f32;
            let on = (t % 1.5) < 1.0;
            let f = 216.0 + 2.0 * (t * 5.0).sin();
            let s = if on {
                let ph = 2.0 * std::f32::consts::PI * f * t;
                (ph.sin() * 0.3 + (2.0 * ph).sin() * 0.15 + (3.0 * ph).sin() * 0.08) * 0.8
            } else {
                0.0
            };
            s + noise() * 0.01
        })
        .collect();
    let beat: Vec<f32> = (0..n)
        .map(|i| {
            let t = i as f32 / sr as f32;
            let kick = if (t % 0.625) < 0.08 { (2.0 * std::f32::consts::PI * 55.0 * t).sin() * (1.0 - (t % 0.625) / 0.08) } else { 0.0 };
            kick * 0.8 + (2.0 * std::f32::consts::PI * 220.0 * t).sin() * 0.1 + (2.0 * std::f32::consts::PI * 261.6 * t).sin() * 0.08
        })
        .collect();
    let mut assets = AssetStore::new();
    assets.insert("voice".into(), Arc::new(AudioBuffer::from_mono(sr, voice)));
    assets.insert("beat".into(), Arc::new(AudioBuffer::from_mono(sr, beat)));
    let mut p = Project::new("Test");
    let mk = |name: &str, kind, asset: &str| {
        let mut t = Track::new(name, kind);
        t.clips.push(Clip { id: new_id(), asset: asset.into(), name: name.into(), start: 0.0, offset: 0.0, length: 6.0, gain_db: 0.0, fade_in: 0.0, fade_out: 0.0, muted: false });
        t
    };
    p.tracks.push(mk("Instru", TrackKind::Instrumental, "beat"));
    p.tracks.push(mk("Voix", TrackKind::Vocal, "voice"));
    p.tracks.push(mk("Adlibs", TrackKind::Adlib, "voice"));
    TestHost { doc: Document::new(p), assets, layouts: Vec::new() }
}

#[test]
fn clean_then_autotune_then_master_then_undo() {
    let mut h = host();
    let (reply, committed) = studio_ai::local::run(&mut h, "Rends ma voix plus propre", None);
    assert!(!committed.is_empty(), "{reply}");
    let voix = h.doc.project.tracks[1].clone();
    assert!(voix.effects.iter().any(|s| s.kind == EffectKind::Eq), "{reply}");
    assert!(voix.effects.iter().any(|s| s.kind == EffectKind::Compressor));

    let (reply, _) = studio_ai::local::run(&mut h, "Trouve la tonalité de l'instru", None);
    assert!(h.doc.project.key.is_some(), "{reply}");

    let (reply, _) = studio_ai::local::run(&mut h, "Mets-moi un autotune bien calibré", None);
    let at = h.doc.project.tracks[1].effects.iter().find(|s| s.kind == EffectKind::Autotune).cloned();
    assert!(at.is_some(), "{reply}");
    assert_eq!(at.unwrap().param("key"), h.doc.project.key.unwrap() as f64);

    let (reply, _) = studio_ai::local::run(&mut h, "Fais-moi un mastering propre", None);
    assert_eq!(h.doc.project.master.effects.len(), 4, "{reply}");
    let lufs = h.measure_lufs(&h.doc.project.clone()).unwrap();
    assert!((lufs + 12.0).abs() < 1.5, "mastered at {lufs} LUFS: {reply}");

    let (_, _) = studio_ai::local::run(&mut h, "annule", None);
    assert!(h.doc.project.master.effects.is_empty());
}

#[test]
fn finish_song_and_layout() {
    let mut h = host();
    let (reply, committed) = studio_ai::local::run(&mut h, "Finalise le son, style mélodique", None);
    assert!(committed.len() >= 4, "{reply}");
    let p = &h.doc.project;
    assert!(!p.tracks[0].effects.is_empty(), "instru gets room for vocals");
    assert!(p.tracks[2].pan.abs() > 0.2, "adlibs panned");
    assert!(p.tracks[1].effects.iter().any(|s| s.kind == EffectKind::Delay));
    studio_ai::local::run(&mut h, "Mets les paroles à droite et agrandis la timeline", None);
    assert_eq!(
        h.layouts,
        vec![
            LayoutCommand::Move { panel: "lyrics".into(), area: "right".into() },
            LayoutCommand::Resize { panel: "timeline".into(), factor: 1.4 }
        ]
    );
}

#[test]
fn variants_are_comparable() {
    let mut h = host();
    studio_ai::local::run(&mut h, "Propose plusieurs variantes de mix", None);
    assert_eq!(h.doc.project.variants.len(), 4);
    assert!(h.doc.project.tracks[1].effects.is_empty(), "the current mix is untouched");
    let o = studio_ai::tools::execute("apply_variant", &serde_json::json!({"name": "mélodique"}), &mut h);
    assert!(!o.error, "{}", o.output);
    assert!(!h.doc.project.tracks[1].effects.is_empty());
}
