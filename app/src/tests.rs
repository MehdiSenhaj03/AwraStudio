//! The studio without its interface: changes, undo, export, and two
//! studios making music together over a real (loopback) session.

use std::time::{Duration, Instant};

use studio_core::effects::EffectKind;
use studio_core::model::*;
use studio_core::ops::{Op, Target, TrackField};

use crate::settings::Settings;
use crate::studio::Studio;

fn studio(name: &str) -> Studio {
    std::env::set_var("AWRA_STUDIO_NO_AUDIO", "1");
    let settings = Settings { user_name: name.into(), try_upnp: false, ..Default::default() };
    let (doc, bufs) = crate::demo::demo_document();
    let mut s = Studio::new(doc, settings);
    for (id, b) in bufs {
        s.assets.insert(&id, b);
    }
    s.rebuild_graph();
    s
}

fn pump_until(studios: &mut [&mut Studio], mut done: impl FnMut(&[&mut Studio]) -> bool) {
    let start = Instant::now();
    while !done(studios) {
        for s in studios.iter_mut() {
            s.tick();
        }
        assert!(start.elapsed() < Duration::from_secs(20), "timed out");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn edits_undo_and_engine_follow() {
    let mut s = studio("Sam");
    let voice = s.doc.project.tracks[1].id;
    s.commit_ops("eq", vec![Op::AddEffect { target: Target::Track(voice), slot: EffectSlot::new(EffectKind::Eq), index: 0 }]);
    assert_eq!(s.doc.project.tracks[1].effects.len(), 1);
    s.commit_ops("vol", vec![Op::SetTrack { id: voice, field: TrackField::VolumeDb(-6.0) }]);
    s.engine.play();
    std::thread::sleep(Duration::from_millis(150));
    s.tick();
    assert!(s.engine.position() > 0.05);
    let m = s.engine.shared.meters.lock().clone();
    assert!(m.master.0 > 0.01, "the mix plays: {:?}", m.master);
    s.undo();
    s.undo();
    assert!(s.doc.project.tracks[1].effects.is_empty());
    assert_eq!(s.doc.project.tracks[1].volume_db, 0.0);
    s.redo();
    assert_eq!(s.doc.project.tracks[1].effects.len(), 1);
}

#[test]
fn ai_turn_through_the_worker() {
    let mut s = studio("Sam");
    s.ask_ai("Rends ma voix plus propre");
    pump_until(&mut [&mut s], |s| s[0].ai_busy.is_none() && s[0].ai_entries.len() >= 3);
    let last = s.ai_entries.last().unwrap();
    assert!(!last.actions.is_empty(), "{}", last.text);
    assert!(!s.doc.project.tracks[1].effects.is_empty());
    // its own undo button
    let id = last.actions[0].0;
    assert!(s.undo_transaction(id));
    assert!(s.doc.project.tracks[1].effects.is_empty());
}

#[test]
fn export_renders_the_song() {
    let mut s = studio("Sam");
    let path = std::env::temp_dir().join(format!("awra-export-{}.wav", std::process::id()));
    s.export(path.clone(), studio_dsp::io::WavFormat::Pcm16);
    pump_until(&mut [&mut s], |s| !s[0].export_busy);
    let (p, lufs, _) = s.last_export.clone().expect("exported");
    assert_eq!(p, path);
    assert!(lufs > -40.0 && lufs < -5.0, "{lufs}");
    let b = studio_dsp::io::decode_file(&path).unwrap();
    assert!(b.duration() > 15.0);
    std::fs::remove_file(path).ok();
}

#[test]
fn stems_line_up() {
    let mut s = studio("Sam");
    let dir = std::env::temp_dir().join(format!("awra-stems-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    s.export_stems(dir.clone());
    pump_until(&mut [&mut s], |s| !s[0].export_busy);
    let files: Vec<_> = std::fs::read_dir(&dir).unwrap().flatten().collect();
    assert_eq!(files.len(), 4);
    let lens: Vec<usize> = files.iter().map(|f| studio_dsp::io::decode_file(&f.path()).unwrap().frames()).collect();
    assert!(lens.windows(2).all(|w| w[0] == w[1]), "{lens:?}");
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn two_studios_in_a_session() {
    let mut a = studio("Sam");
    a.host_session();
    let code = a.session.as_ref().unwrap().invite.clone().unwrap();
    let mut b = Studio::new(studio_core::Document::new(Project::new("vide")), Settings { user_name: "Léa".into(), ..Default::default() });
    // join blocks until the host answers: the host must keep ticking
    let joiner = std::thread::spawn(move || studio_collab::Session::join(&code, "Léa"));
    pump_until(&mut [&mut a], |_| joiner.is_finished());
    b.session = Some(joiner.join().unwrap().unwrap());
    pump_until(&mut [&mut a, &mut b], |s| s[1].doc.project.tracks.len() == 4);
    assert_eq!(b.doc.project.name, a.doc.project.name);
    // the files come over
    pump_until(&mut [&mut a, &mut b], |s| s[1].doc.project.assets.keys().all(|k| s[1].assets.store.contains_key(k)));

    // Léa changes her track, Sam sees it
    let lea = b.doc.project.tracks[2].id;
    b.commit_ops("vol", vec![Op::SetTrack { id: lea, field: TrackField::VolumeDb(-4.5) }]);
    pump_until(&mut [&mut a, &mut b], |s| s[0].doc.project.track(lea).unwrap().volume_db == -4.5);

    // Sam adds a section of lyrics, Léa sees it
    let sec = LyricSection { id: new_id(), title: "Pont".into(), text: "la la".into(), start: None };
    a.commit_ops("pont", vec![Op::AddSection { section: sec, index: 2 }]);
    pump_until(&mut [&mut a, &mut b], |s| s[1].doc.project.lyrics.len() == 3);

    // concurrent edits converge
    let sam = a.doc.project.tracks[1].id;
    a.commit_ops("pan", vec![Op::SetTrack { id: sam, field: TrackField::Pan(-0.3) }]);
    b.commit_ops("pan", vec![Op::SetTrack { id: sam, field: TrackField::Pan(0.4) }]);
    pump_until(&mut [&mut a, &mut b], |s| {
        s[0].replica.pending() == 0 && s[1].replica.pending() == 0 && s[0].doc.project == s[1].doc.project
    });

    // chat
    b.send_chat("on enregistre le refrain ?");
    pump_until(&mut [&mut a, &mut b], |s| s[0].chat.iter().any(|c| c.1.contains("refrain")));
    a.leave_session();
}
