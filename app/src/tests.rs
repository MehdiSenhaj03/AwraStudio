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
    // no sound card here: pretend two microphones and two outputs
    std::env::set_var("AWRA_STUDIO_FAKE_DEVICES", "Micro USB Shure MV7;Micro intégré|Casque;Haut-parleurs");
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

#[test]
fn dropped_files_land_on_a_track_or_a_new_one() {
    let mut s = studio("Sam");
    let dir = std::env::temp_dir().join(format!("awra-drop-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let rate = s.doc.project.sample_rate;
    let mut files = Vec::new();
    for (name, secs) in [("a.wav", 1.0), ("b.wav", 2.0)] {
        let p = dir.join(name);
        let tone: Vec<f32> = (0..(rate as f64 * secs) as usize).map(|i| (i as f32 * 0.05).sin() * 0.3).collect();
        studio_dsp::io::write_wav(&p, &studio_dsp::AudioBuffer::from_mono(rate, tone), studio_dsp::io::WavFormat::Pcm16).unwrap();
        files.push(p);
    }
    // onto an existing track: one after the other from the drop point
    let voice = s.doc.project.tracks[1].id;
    let before = s.doc.project.track(voice).unwrap().clips.len();
    s.import_files(&files, Some(voice), 3.0);
    assert_eq!(s.pending_imports.len(), 2);
    pump_until(&mut [&mut s], |s| s[0].pending_imports.is_empty());
    let t = s.doc.project.track(voice).unwrap();
    assert_eq!(t.clips.len(), before + 2);
    let mut starts: Vec<f64> = t.clips.iter().filter(|c| c.name == "a" || c.name == "b").map(|c| c.start).collect();
    starts.sort_by(f64::total_cmp);
    assert!((starts[0] - 3.0).abs() < 1e-6, "{starts:?}");
    assert!((starts[1] - starts[0]).abs() > 0.9, "{starts:?}");
    // onto nothing: a new track, undone in one step
    let n = s.doc.project.tracks.len();
    s.import_files(&files[..1], None, 0.5);
    pump_until(&mut [&mut s], |s| s[0].pending_imports.is_empty());
    assert_eq!(s.doc.project.tracks.len(), n + 1);
    let last = s.doc.project.tracks.last().unwrap();
    assert_eq!(last.kind, TrackKind::Audio);
    assert!((last.clips[0].start - 0.5).abs() < 1e-6);
    s.undo();
    assert_eq!(s.doc.project.tracks.len(), n);
    std::fs::remove_dir_all(dir).ok();
}

fn ask(s: &mut Studio, q: &str) -> String {
    let n = s.ai_entries.len();
    s.ask_ai(q);
    pump_until(&mut [s], |s| s[0].ai_busy.is_none() && s[0].ai_entries.len() >= n + 2);
    s.ai_entries.last().unwrap().text.clone()
}

#[test]
fn the_engineer_sees_and_drives_the_whole_app() {
    let mut s = studio("Sam");
    // what microphone do I have?
    let r = ask(&mut s, "Quel micro j'ai ?");
    assert!(r.contains("Micro USB Shure MV7") && r.contains("Micro intégré"), "{r}");
    // switch it
    let r = ask(&mut s, "Utilise le micro intégré");
    assert_eq!(s.settings.audio_input.as_deref(), Some("Micro intégré"), "{r}");
    assert_eq!(s.engine.input_name.as_deref(), Some("Micro intégré"));
    let r = ask(&mut s, "passe la sortie sur les haut-parleurs");
    assert_eq!(s.settings.audio_output.as_deref(), Some("Haut-parleurs"), "{r}");
    // the shortest return
    let r = ask(&mut s, "Réduis la latence du retour micro au minimum");
    assert_eq!(s.settings.buffer_frames, 64, "{r}");
    assert_eq!(s.settings.monitor_mode, 0);
    // transport, appearance, files
    ask(&mut s, "va à 1:05");
    assert!((s.engine.position() - 65.0).abs() < 0.01);
    ask(&mut s, "active le métronome");
    assert!(s.settings.metronome);
    ask(&mut s, "passe en mode sombre");
    assert_eq!(s.settings.appearance.theme, 2);
    // edit by voice: split the voice at 4 s
    let voice = s.doc.project.tracks[1].clone();
    ask(&mut s, &format!("coupe {} à 0:04", voice.name));
    assert_eq!(s.doc.project.tracks[1].clips.len(), voice.clips.len() + 1);
    // the full state, as the model reads it
    let st = crate::appctl::state(&s);
    assert_eq!(st["audio"]["input"], "Micro intégré");
    assert_eq!(st["transport"]["metronome"], true);
    assert_eq!(st["audio"]["monitor_mode"], "direct");
    // and any action by name
    let r = crate::appctl::action(&mut s, &serde_json::json!({"action": "set_loop", "start": "0:10", "end": 20}));
    assert!(r.is_ok(), "{r:?}");
    assert_eq!(s.doc.project.loop_region, Some((10.0, 20.0)));
    let r = crate::appctl::action(&mut s, &serde_json::json!({"action": "new_track", "kind": "adlib", "name": "Choeurs"}));
    assert!(r.is_ok());
    assert!(s.doc.project.tracks.iter().any(|t| t.name == "Choeurs"));
    assert!(crate::appctl::action(&mut s, &serde_json::json!({"action": "fly"})).is_err());
}

#[test]
fn everyday_editing() {
    use crate::edit;
    let mut s = studio("Sam");
    let voice = s.doc.project.tracks[1].id;
    let first = s.doc.project.tracks[1].clips[0].clone();
    let n0 = s.doc.project.tracks[1].clips.len();
    // split where the playhead is (the right-click "Couper ici" does the same at the click)
    let at = first.start + first.length / 2.0;
    assert!(edit::split_at(&mut s, voice, first.id, at));
    let t = s.doc.project.track(voice).unwrap();
    assert_eq!(t.clips.len(), n0 + 1);
    let left = t.clip(first.id).unwrap().clone();
    let right = t.clips.iter().find(|c| (c.start - at).abs() < 1e-9).unwrap().clone();
    assert!((left.end() - at).abs() < 1e-9);
    assert!((right.offset - (first.offset + first.length / 2.0)).abs() < 1e-9, "the sound continues where it was cut");
    // undo puts it back in one step
    s.undo();
    assert_eq!(s.doc.project.track(voice).unwrap().clips.len(), n0);
    assert!(edit::split_at(&mut s, voice, first.id, at));

    // duplicating the left half replaces what was under it (the right half): nothing doubles
    let dup = edit::duplicate(&mut s, voice, first.id).unwrap();
    let t = s.doc.project.track(voice).unwrap();
    let d = t.clip(dup).unwrap();
    assert!((d.start - at).abs() < 1e-9);
    for c in t.clips.iter().filter(|c| c.id != dup) {
        assert!(c.end() <= d.start + 1e-9 || c.start >= d.end() - 1e-9, "{c:?} under the copy");
    }

    // copy / paste at a time on another track
    let adlibs = s.doc.project.tracks[3].id;
    edit::copy(&mut s, voice, first.id);
    assert!(edit::paste(&mut s, Some(adlibs), Some(30.0)));
    assert!(s.doc.project.track(adlibs).unwrap().clips.iter().any(|c| (c.start - 30.0).abs() < 1e-9));

    // trim the left edge: the sound stays where it was on the timeline
    let c = s.doc.project.track(voice).unwrap().clip(first.id).unwrap().clone();
    edit::trim(&mut s, voice, first.id, c.start + 1.0, c.length - 1.0);
    let c2 = s.doc.project.track(voice).unwrap().clip(first.id).unwrap().clone();
    assert!((c2.offset - (c.offset + 1.0)).abs() < 1e-9 && (c2.end() - c.end()).abs() < 1e-9);

    // fades, gain, mute, normalize
    edit::set_fades(&mut s, voice, first.id, 0.2, 0.5);
    let c3 = s.doc.project.track(voice).unwrap().clip(first.id).unwrap().clone();
    assert_eq!((c3.fade_in, c3.fade_out), (0.2, 0.5));
    edit::toggle_mute(&mut s, voice, first.id);
    assert!(s.doc.project.track(voice).unwrap().clip(first.id).unwrap().muted);
    let g = edit::normalize(&mut s, voice, first.id).expect("normalized");
    assert!((s.doc.project.track(voice).unwrap().clip(first.id).unwrap().gain_db - g).abs() < 1e-9);

    // tracks and markers
    let copy = edit::duplicate_track(&mut s, voice).unwrap();
    let p = &s.doc.project;
    let i = p.tracks.iter().position(|t| t.id == copy).unwrap();
    assert_eq!(p.tracks[i - 1].id, voice);
    assert_eq!(p.tracks[i].clips.len(), p.tracks[i - 1].clips.len());
    edit::move_track(&mut s, copy, -1);
    assert_eq!(s.doc.project.tracks[i - 1].id, copy);
    edit::add_marker(&mut s, 12.0, None);
    assert_eq!(s.doc.project.markers.len(), 1);
}
