//! Awra Studio's core: the project model, operations (undoable,
//! shareable), history and persistence.
//!
//! SPDX-License-Identifier: GPL-3.0-or-later

pub mod effects;
pub mod history;
pub mod model;
pub mod ops;

pub use effects::{EffectKind, PluginFormat};
pub use history::Document;
pub use model::*;
pub use ops::{ClipField, Op, ProjectField, Target, TrackField, Transaction};

#[cfg(test)]
mod tests {
    use super::*;

    fn demo() -> Document {
        let mut doc = Document::new(Project::new("Test"));
        let t = Track::new("Voix", TrackKind::Vocal);
        let id = t.id;
        doc.commit(Transaction::new("piste", Actor::User, vec![Op::AddTrack { track: t, index: 0 }])).unwrap();
        let clip = Clip {
            id: new_id(),
            asset: "abc".into(),
            name: "Prise 1".into(),
            start: 1.0,
            offset: 0.0,
            length: 4.0,
            gain_db: 0.0,
            fade_in: 0.0,
            fade_out: 0.0,
            muted: false,
        };
        doc.commit(Transaction::new("clip", Actor::User, vec![Op::AddClip { track: id, clip }])).unwrap();
        doc
    }

    #[test]
    fn undo_redo_roundtrip() {
        let mut doc = demo();
        let before = doc.project.clone();
        let tid = doc.project.tracks[0].id;
        let slot = EffectSlot::new(EffectKind::Compressor);
        let sid = slot.id;
        doc.commit(Transaction::new(
            "comp",
            Actor::Ai,
            vec![
                Op::AddEffect { target: Target::Track(tid), slot, index: 0 },
                Op::SetParam { target: Target::Track(tid), slot: sid, param: "ratio".into(), value: 4.0 },
                Op::SetTrack { id: tid, field: TrackField::VolumeDb(-3.0) },
            ],
        ))
        .unwrap();
        assert_eq!(doc.project.tracks[0].effects[0].param("ratio"), 4.0);
        doc.undo().unwrap();
        assert_eq!(doc.project, before);
        doc.redo().unwrap();
        assert_eq!(doc.project.tracks[0].volume_db, -3.0);
        assert_eq!(doc.project.tracks[0].effects.len(), 1);
    }

    #[test]
    fn failed_transaction_rolls_back() {
        let mut doc = demo();
        let before = doc.project.clone();
        let tid = doc.project.tracks[0].id;
        let r = doc.commit(Transaction::new(
            "x",
            Actor::User,
            vec![Op::SetTrack { id: tid, field: TrackField::Mute(true) }, Op::RemoveTrack { id: 42 }],
        ));
        assert!(r.is_err());
        assert_eq!(doc.project, before);
    }

    #[test]
    fn undo_ai_only() {
        let mut doc = demo();
        let tid = doc.project.tracks[0].id;
        doc.commit(Transaction::new("ai", Actor::Ai, vec![Op::SetTrack { id: tid, field: TrackField::Pan(0.5) }]))
            .unwrap();
        doc.commit(Transaction::new("me", Actor::User, vec![Op::SetTrack { id: tid, field: TrackField::Solo(true) }]))
            .unwrap();
        doc.undo_last_by(&Actor::Ai).unwrap();
        assert_eq!(doc.project.tracks[0].pan, 0.0);
        assert!(doc.project.tracks[0].solo);
    }

    #[test]
    fn slider_edits_coalesce() {
        let mut doc = demo();
        let tid = doc.project.tracks[0].id;
        let n = doc.history.undo.len();
        for v in [-1.0, -2.0, -3.0] {
            doc.commit(Transaction::new("vol", Actor::User, vec![Op::SetTrack { id: tid, field: TrackField::VolumeDb(v) }]))
                .unwrap();
        }
        assert_eq!(doc.history.undo.len(), n + 1);
        doc.undo();
        assert_eq!(doc.project.tracks[0].volume_db, 0.0);
    }

    #[test]
    fn serde_roundtrip_and_save() {
        let mut doc = demo();
        let tid = doc.project.tracks[0].id;
        doc.commit(Transaction::new(
            "fx",
            Actor::User,
            vec![Op::AddEffect { target: Target::Master, slot: EffectSlot::new(EffectKind::Limiter), index: 0 }],
        ))
        .unwrap();
        let tx = Transaction::new("p", Actor::Peer("Sam".into()), vec![Op::SetTrack { id: tid, field: TrackField::Pan(-0.2) }]);
        let json = serde_json::to_string(&tx).unwrap();
        let back: Transaction = serde_json::from_str(&json).unwrap();
        assert_eq!(back, tx);
        let dir = std::env::temp_dir().join(format!("studio-core-test-{}", new_id()));
        doc.save_to(&dir).unwrap();
        let re = Document::open(&dir).unwrap();
        assert_eq!(re.project, doc.project);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn param_specs_are_sane() {
        for k in effects::BUILTIN.iter() {
            let s = effects::spec(k);
            for p in s.params {
                assert!(p.min <= p.default && p.default <= p.max, "{} {}", s.name, p.id);
                let n = p.to_norm(p.default);
                assert!((-1e-9..=1.0 + 1e-9).contains(&n), "{} {} {n}", s.name, p.id);
                let back = p.from_norm(n);
                assert!((back - p.default).abs() < 1e-6 * (1.0 + p.max.abs()), "{} {} {back}", s.name, p.id);
            }
            assert_eq!(EffectKind::from_key(k.key()).as_ref(), Some(k));
        }
    }

    #[test]
    fn find_track_by_name() {
        let doc = demo();
        assert!(doc.project.find_track("voi").is_some());
        assert!(doc.project.find_track("VOIX").is_some());
        assert!(doc.project.find_track("beat").is_none());
    }
}
