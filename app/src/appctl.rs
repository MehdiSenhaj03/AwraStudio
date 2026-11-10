//! The whole application, for the agent: its state as JSON and every
//! action a user could take by hand (on the UI thread).

use std::path::PathBuf;

use serde_json::{json, Value};
use studio_ai::tools::resolve_track;
use studio_core::model::*;
use studio_core::ops::{Op, ProjectField, Target};

use crate::studio::{Studio, UiAction};
use crate::ui::appearance::{Appearance, ACCENTS};

const MODES: [&str; 3] = ["direct", "low_latency", "full"];

fn sv(v: &Value, k: &str) -> Option<String> {
    v.get(k).and_then(|x| match x {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    })
}
fn fv(v: &Value, k: &str) -> Option<f64> {
    v.get(k).and_then(|x| x.as_f64().or_else(|| x.as_str().and_then(|s| parse_time(s))))
}
fn bv(v: &Value, k: &str) -> Option<bool> {
    v.get(k).and_then(|x| x.as_bool().or_else(|| x.as_str().map(|s| matches!(s, "true" | "on" | "oui" | "1"))))
}

/// "1:30", "90", "1m30" → seconds.
pub fn parse_time(s: &str) -> Option<f64> {
    let s = s.trim().replace(['m', 'h'], ":").replace('s', "");
    let mut t = 0.0;
    for part in s.split(':').filter(|p| !p.is_empty()) {
        t = t * 60.0 + part.trim().replace(',', ".").parse::<f64>().ok()?;
    }
    Some(t)
}

fn track_id(s: &Studio, v: &Value) -> Result<Id, String> {
    let q = sv(v, "track").ok_or("track : la piste (nom ou id) est attendue")?;
    match resolve_track(&s.doc.project, &q) {
        Some(Target::Track(id)) => Ok(id),
        _ => Err(format!("piste « {q} » introuvable")),
    }
}

fn mode_name(m: u8) -> &'static str {
    MODES[m.min(2) as usize]
}

/// Everything about the application besides the project.
pub fn state(s: &Studio) -> Value {
    let p = &s.doc.project;
    let e = &s.engine;
    let name_of = |id: Id| p.track(id).map(|t| t.name.clone());
    let set = &s.settings;
    let hist = &s.doc.history;
    json!({
        "transport": {
            "playing": e.is_playing(),
            "position_s": (e.position() * 100.0).round() / 100.0,
            "recording": s.recording.map(|t| json!({"track": name_of(t), "since_s": s.rec_start})),
            "loop": p.loop_region,
            "metronome": set.metronome,
            "count_in": set.count_in,
            "bpm": p.bpm,
            "duration_s": (p.duration() * 10.0).round() / 10.0,
        },
        "audio": {
            "input": e.input_name,
            "input_open": e.has_input(),
            "input_level": (e.input_level() * 1000.0).round() / 1000.0,
            "output": e.output_name,
            "output_open": e.has_device(),
            "device_rate": e.device_rate(),
            "project_rate": p.sample_rate,
            "chosen": {"input": set.audio_input, "output": set.audio_output, "no_input": set.no_input},
            "buffer_frames": set.buffer_frames,
            "monitor_mode": mode_name(set.monitor_mode),
            "monitor_armed": set.monitor_armed,
            "monitor_latency_ms": e.monitor_latency_ms(set.monitor_mode).map(|x| x.round()),
            "monitored_track": s.monitor_target().and_then(name_of),
            "recording_latency_compensation_ms": set.latency_ms,
            "problem": e.last_error,
        },
        "selection": {
            "track": s.selected.and_then(|id| p.track(id).map(|t| json!({"id": id.to_string(), "name": t.name}))),
            "clip": s.selected_clip.map(|(t, c)| json!({"track": t.to_string(), "clip": c.to_string()})),
            "master_chain_shown": s.show_master_chain,
        },
        "project_file": {
            "name": p.name,
            "folder": s.doc.dir.as_ref().filter(|_| !s.doc.is_temporary()).map(|d| d.display().to_string()),
            "saved_once": !s.doc.is_temporary(),
            "unsaved_changes": s.doc.dirty,
        },
        "settings": {
            "user_name": set.user_name,
            "ai": {"provider": set.ai.kind.label(), "model": set.ai.model(), "proactive": set.ai.proactive},
            "try_upnp": set.try_upnp,
            "sync_transport": set.sync_transport,
        },
        "appearance": set.appearance,
        "interface": s.ui_info,
        "view": {"zoom_px_per_s": s.zoom, "scroll_s": s.scroll_x},
        "session": s.session.as_ref().map(|x| json!({
            "role": format!("{:?}", x.role).to_lowercase(),
            "invite_code": x.invite,
            "port": x.port,
            "upnp": x.upnp,
            "participants": s.peers.iter().map(|p| json!({"name": p.name, "status": p.status})).collect::<Vec<_>>(),
            "chat": s.chat.iter().rev().take(15).rev().map(|(w, t)| json!({"from": w, "text": t})).collect::<Vec<_>>(),
        })),
        "history": {
            "undo": hist.undo.iter().rev().take(12).map(|t| json!({"label": t.forward.label, "by": format!("{:?}", t.forward.actor)})).collect::<Vec<_>>(),
            "redo": hist.redo.iter().rev().take(6).map(|t| t.forward.label.clone()).collect::<Vec<_>>(),
        },
        "export": {"busy": s.export_busy, "last": s.last_export.as_ref().map(|(p, l, tp)| json!({"path": p.display().to_string(), "lufs": l, "true_peak": tp}))},
        "imports_in_progress": s.pending_imports.iter().map(|p| p.name.clone()).collect::<Vec<_>>(),
        "plugins_known": set.plugins.len(),
        "recent_projects": set.recent,
        "default_folder": crate::settings::music_dir().display().to_string(),
        "platform": {"os": std::env::consts::OS, "app_version": env!("CARGO_PKG_VERSION")},
    })
}

fn ok(say: impl Into<String>) -> Result<Value, String> {
    Ok(json!({"ok": true, "say": say.into()}))
}

fn wav_format(v: &Value) -> studio_dsp::io::WavFormat {
    match sv(v, "format").unwrap_or_default().to_lowercase().as_str() {
        f if f.contains("16") => studio_dsp::io::WavFormat::Pcm16,
        f if f.contains("32") || f.contains("float") => studio_dsp::io::WavFormat::Float32,
        _ => studio_dsp::io::WavFormat::Pcm24,
    }
}

fn file_name(s: &Studio) -> String {
    crate::ui::header::sanitize(&s.doc.project.name)
}

/// Does one action (see studio_ai::tools::APP_ACTIONS).
pub fn action(s: &mut Studio, v: &Value) -> Result<Value, String> {
    let a = sv(v, "action").ok_or("action manquante")?;
    match a.as_str() {
        // transport
        "play" => {
            if !s.engine.is_playing() {
                s.play_pause();
            }
            ok("Lecture.")
        }
        "pause" => {
            if s.engine.is_playing() {
                s.play_pause();
            }
            ok("Pause.")
        }
        "stop" => {
            s.stop();
            ok("Arrêt, retour au début.")
        }
        "seek" => {
            let t = fv(v, "time").ok_or("time (s) attendu")?;
            s.seek(t);
            ok(format!("Tête de lecture à {}.", crate::ui::fmt_time(t)))
        }
        "set_loop" => {
            let (a, b) = (fv(v, "start").ok_or("start attendu")?, fv(v, "end").ok_or("end attendu")?);
            if b <= a {
                return Err("la fin doit suivre le début".into());
            }
            s.commit_ops("Boucle", vec![Op::SetProject { field: ProjectField::LoopRegion(Some((a, b))) }]);
            ok(format!("Boucle de {} à {}.", crate::ui::fmt_time(a), crate::ui::fmt_time(b)))
        }
        "clear_loop" => {
            s.commit_ops("Boucle retirée", vec![Op::SetProject { field: ProjectField::LoopRegion(None) }]);
            ok("Boucle retirée.")
        }
        "metronome" => {
            let on = bv(v, "on").unwrap_or(!s.settings.metronome);
            s.settings.metronome = on;
            s.engine.send(studio_engine::Cmd::Metronome(on));
            s.ui_revision += 1;
            ok(if on { "Métronome activé." } else { "Métronome coupé." })
        }
        "record" => {
            let on = bv(v, "on").unwrap_or(s.recording.is_none());
            if v.get("track").is_some() {
                s.selected = Some(track_id(s, v)?);
            }
            if on != s.recording.is_some() {
                s.toggle_record();
            }
            if on && s.recording.is_none() {
                return Err(s.toasts.last().cloned().unwrap_or_else(|| "enregistrement impossible".into()));
            }
            ok(if on { "Enregistrement lancé." } else { "Prise terminée." })
        }
        // selection and view
        "select_track" => {
            let id = track_id(s, v)?;
            s.selected = Some(id);
            s.show_master_chain = false;
            s.ui_revision += 1;
            ok("Piste sélectionnée.")
        }
        "select_clip" => {
            let t = track_id(s, v)?;
            let c = sv(v, "clip").and_then(|c| c.parse::<u64>().ok()).ok_or("clip (id) attendu")?;
            s.selected = Some(t);
            s.selected_clip = Some((t, c));
            s.ui_revision += 1;
            ok("Clip sélectionné.")
        }
        "show_master_chain" => {
            s.show_master_chain = bv(v, "on").unwrap_or(true);
            s.ui_revision += 1;
            ok("Effets du master affichés.")
        }
        "zoom" => {
            let secs = fv(v, "seconds").ok_or("seconds (durée visible) attendu")?.max(1.0);
            let w = s.ui_info.get("timeline_width").and_then(Value::as_f64).unwrap_or(900.0);
            s.zoom = (w / secs).clamp(2.0, 800.0);
            s.ui_revision += 1;
            ok(format!("{secs:.0} s visibles."))
        }
        "scroll_to" => {
            s.scroll_x = fv(v, "time").ok_or("time attendu")?.max(0.0);
            s.ui_revision += 1;
            ok("Vue déplacée.")
        }
        "editing" => {
            s.ui_actions.push(UiAction::Editing(bv(v, "on").unwrap_or(true)));
            ok("Mode personnalisation changé.")
        }
        "open_settings" => {
            s.ui_actions.push(UiAction::OpenSettings(v.get("tab").and_then(Value::as_u64).unwrap_or(0) as u32));
            ok("Réglages ouverts.")
        }
        "window" => {
            s.ui_actions.push(UiAction::Window(sv(v, "state").unwrap_or_else(|| "maximize".into())));
            ok("Fenêtre modifiée.")
        }
        // quick edits
        "new_track" => {
            let kind = match sv(v, "kind").unwrap_or_default().to_lowercase().as_str() {
                "adlib" | "adlibs" => TrackKind::Adlib,
                "instrumental" | "instru" | "beat" => TrackKind::Instrumental,
                "audio" => TrackKind::Audio,
                _ => TrackKind::Vocal,
            };
            let id = s.new_track(kind);
            if let Some(n) = sv(v, "name") {
                s.commit_ops("Renommer", vec![Op::SetTrack { id, field: studio_core::ops::TrackField::Name(n) }]);
            }
            s.flash = Some((id, awra::now_ms()));
            Ok(json!({"ok": true, "track": id.to_string(), "say": "Piste créée."}))
        }
        "split_clip" => {
            let t = track_id(s, v)?;
            let at = fv(v, "at").ok_or("at (s) attendu")?;
            let clip = match sv(v, "clip").and_then(|c| c.parse::<u64>().ok()) {
                Some(c) => c,
                None => s.doc.project.track(t).and_then(|tr| tr.clips.iter().find(|c| c.start < at && c.end() > at)).map(|c| c.id).ok_or("aucun clip à cet endroit")?,
            };
            crate::ui::timeline::split_at(s, t, clip, at);
            ok(format!("Clip coupé à {}.", crate::ui::fmt_time(at)))
        }
        "duplicate_clip" | "normalize_clip" | "mute_clip" | "set_fades" | "trim_clip" | "move_clip" | "copy_clip" => {
            let t = track_id(s, v)?;
            let clip = match sv(v, "clip").and_then(|c| c.parse::<u64>().ok()) {
                Some(c) => c,
                // the clip under a time, else the track's first
                None => {
                    let at = fv(v, "at").unwrap_or(-1.0);
                    let tr = s.doc.project.track(t).ok_or("piste introuvable")?;
                    tr.clips.iter().find(|c| c.start <= at && c.end() >= at).or(tr.clips.first()).map(|c| c.id).ok_or("cette piste n'a aucun clip")?
                }
            };
            match a.as_str() {
                "duplicate_clip" => {
                    let id = crate::edit::duplicate(s, t, clip).ok_or("clip introuvable")?;
                    Ok(json!({"ok": true, "clip": id.to_string(), "say": "Clip dupliqué juste après."}))
                }
                "normalize_clip" => {
                    let g = crate::edit::normalize(s, t, clip).ok_or("normalisation impossible")?;
                    ok(format!("Clip normalisé ({g:+.1} dB)."))
                }
                "mute_clip" => {
                    crate::edit::toggle_mute(s, t, clip);
                    ok("Clip muet / réactivé.")
                }
                "set_fades" => {
                    crate::edit::set_fades(s, t, clip, fv(v, "fade_in").unwrap_or(0.0), fv(v, "fade_out").unwrap_or(0.0));
                    ok("Fondus réglés.")
                }
                "trim_clip" => {
                    let c = s.doc.project.track(t).and_then(|x| x.clip(clip)).cloned().ok_or("clip introuvable")?;
                    let start = fv(v, "start").unwrap_or(c.start).max(c.start - c.offset).max(0.0);
                    let end = fv(v, "end").unwrap_or(c.end()).min(c.start - c.offset + crate::edit::asset_len(s, &c));
                    if end - start < crate::edit::MIN_CLIP {
                        return Err("le clip serait vide".into());
                    }
                    crate::edit::trim(s, t, clip, start, end - start);
                    ok("Clip rogné.")
                }
                "move_clip" => {
                    if let Some(to) = sv(v, "to_track") {
                        let to = match resolve_track(&s.doc.project, &to) {
                            Some(Target::Track(id)) => id,
                            _ => return Err(format!("piste « {to} » introuvable")),
                        };
                        crate::edit::move_to_track(s, t, clip, to);
                        if let Some(st) = fv(v, "start") {
                            crate::edit::move_clip(s, to, clip, st.max(0.0));
                        }
                    } else {
                        crate::edit::move_clip(s, t, clip, fv(v, "start").ok_or("start attendu")?.max(0.0));
                    }
                    ok("Clip déplacé.")
                }
                _ => {
                    crate::edit::copy(s, t, clip);
                    ok("Clip copié (paste pour le coller).")
                }
            }
        }
        "paste" => {
            let track = if v.get("track").is_some() { Some(track_id(s, v)?) } else { None };
            if crate::edit::paste(s, track, fv(v, "at")) {
                ok("Collé.")
            } else {
                Err("rien à coller".into())
            }
        }
        "duplicate_track" => {
            let t = track_id(s, v)?;
            let id = crate::edit::duplicate_track(s, t).ok_or("piste introuvable")?;
            Ok(json!({"ok": true, "track": id.to_string(), "say": "Piste dupliquée."}))
        }
        "move_track" => {
            let t = track_id(s, v)?;
            crate::edit::move_track(s, t, v.get("delta").and_then(Value::as_i64).unwrap_or(1) as i32);
            ok("Piste déplacée.")
        }
        "add_marker" => {
            let t = fv(v, "time").unwrap_or_else(|| s.engine.position());
            crate::edit::add_marker(s, t, sv(v, "name"));
            ok(format!("Repère ajouté à {}.", crate::ui::fmt_time(t)))
        }
        "zoom_to_fit" => {
            crate::edit::zoom_to_fit(s);
            ok("Tout le morceau est visible.")
        }
        "delete_clip" => {
            let t = track_id(s, v)?;
            let c = sv(v, "clip").and_then(|c| c.parse::<u64>().ok()).ok_or("clip (id) attendu")?;
            s.commit_ops("Supprimer le clip", vec![Op::RemoveClip { track: t, clip: c }]);
            ok("Clip supprimé.")
        }
        // audio
        "set_audio" => {
            let mut reopen = false;
            if let Some(i) = sv(v, "input") {
                match i.to_lowercase().as_str() {
                    "none" | "aucun" | "off" => s.settings.no_input = true,
                    "system" | "default" | "défaut" | "systeme" | "système" => {
                        s.settings.no_input = false;
                        s.settings.audio_input = None;
                    }
                    _ => {
                        s.settings.no_input = false;
                        s.settings.audio_input = Some(match_device(&i, &studio_engine::list_devices().inputs)?);
                    }
                }
                reopen = true;
            }
            if let Some(o) = sv(v, "output") {
                s.settings.audio_output = match o.to_lowercase().as_str() {
                    "system" | "default" | "défaut" | "systeme" | "système" => None,
                    _ => Some(match_device(&o, &studio_engine::list_devices().outputs)?),
                };
                reopen = true;
            }
            if let Some(b) = v.get("buffer_frames").and_then(Value::as_u64) {
                s.settings.buffer_frames = b as u32;
                reopen = true;
            }
            if let Some(m) = sv(v, "monitor_mode") {
                let m = match m.as_str() {
                    "direct" | "0" => 0,
                    "full" | "2" => 2,
                    _ => 1,
                };
                s.settings.monitor_mode = m;
                s.engine.send(studio_engine::Cmd::MonitorMode(m));
            }
            if let Some(b) = bv(v, "monitor_armed") {
                s.settings.monitor_armed = b;
            }
            if let Some(l) = fv(v, "latency_ms") {
                s.settings.latency_ms = l;
                s.engine.latency_ms = l;
            }
            s.settings.save();
            let msg = if reopen { s.reopen_audio() } else { "Réglage audio appliqué.".into() };
            s.ui_revision += 1;
            Ok(json!({"ok": true, "say": msg, "latency_ms": s.engine.monitor_latency_ms(s.settings.monitor_mode).map(|x| x.round())}))
        }
        // settings
        "set_settings" => {
            let st = &mut s.settings;
            if let Some(n) = sv(v, "user_name") {
                st.user_name = n;
            }
            if let Some(b) = bv(v, "count_in") {
                st.count_in = b;
            }
            if let Some(b) = bv(v, "try_upnp") {
                st.try_upnp = b;
            }
            if let Some(b) = bv(v, "proactive") {
                st.ai.proactive = b;
            }
            if let Some(b) = bv(v, "sync_transport") {
                st.sync_transport = b;
            }
            st.save();
            s.ui_revision += 1;
            ok("Réglages enregistrés.")
        }
        "set_appearance" => {
            let mut ap: Appearance = match sv(v, "preset").as_deref() {
                Some("opaque") => Appearance::opaque(),
                Some("clear") => Appearance::clear_glass(),
                Some("glass") => Appearance::default(),
                _ => s.settings.appearance.clone(),
            };
            if let Some(t) = sv(v, "theme") {
                ap.theme = match t.to_lowercase().as_str() {
                    "light" | "clair" => 1,
                    "dark" | "sombre" => 2,
                    _ => 0,
                };
            }
            if let Some(acc) = sv(v, "accent") {
                ap.accent = if acc.starts_with('#') {
                    Some(acc)
                } else {
                    ACCENTS.iter().find(|x| x.0.to_lowercase() == acc.to_lowercase()).map(|x| x.1.to_string()).filter(|x| !x.is_empty())
                };
            }
            if let Some(b) = bv(v, "blur") {
                ap.blur = b;
            }
            for part in ["window", "header", "panels", "cards", "menus", "lanes"] {
                if let Some(x) = v.get(part) {
                    ap.set(part, x.as_f64().map(|f| if f > 1.0 { f / 100.0 } else { f }));
                }
            }
            s.settings.appearance = ap;
            s.settings.save();
            s.ui_actions.push(UiAction::ApplyAppearance);
            ok("Apparence appliquée.")
        }
        // files
        "save" => {
            let dir = match sv(v, "dir") {
                Some(d) => Some(PathBuf::from(d)),
                None if s.doc.is_temporary() => Some(crate::settings::music_dir().join(format!("{}.awra", file_name(s)))),
                None => None,
            };
            let d = s.save(dir)?;
            Ok(json!({"ok": true, "folder": d.display().to_string(), "say": format!("Projet enregistré dans {}.", d.display())}))
        }
        "export" => {
            let path = sv(v, "path").map(PathBuf::from).unwrap_or_else(|| crate::settings::music_dir().join(format!("{}.wav", file_name(s))));
            if let Some(d) = path.parent() {
                let _ = std::fs::create_dir_all(d);
            }
            s.export(path.clone(), wav_format(v));
            Ok(json!({"ok": true, "path": path.display().to_string(), "say": format!("Export lancé vers {} (il se termine en arrière-plan).", path.display())}))
        }
        "export_stems" => {
            let dir = sv(v, "dir").map(PathBuf::from).unwrap_or_else(|| crate::settings::music_dir().join(format!("{} - pistes", file_name(s))));
            let _ = std::fs::create_dir_all(&dir);
            s.export_stems(dir.clone());
            ok(format!("Export des pistes séparées vers {}.", dir.display()))
        }
        "import_audio" => {
            let paths: Vec<PathBuf> = match v.get("paths").or_else(|| v.get("path")) {
                Some(Value::Array(a)) => a.iter().filter_map(Value::as_str).map(PathBuf::from).collect(),
                Some(Value::String(p)) => vec![PathBuf::from(p)],
                _ => return Err("paths : les fichiers à importer".into()),
            };
            if let Some(p) = paths.iter().find(|p| !p.exists()) {
                return Err(format!("{} n'existe pas", p.display()));
            }
            let track = if v.get("track").is_some() { Some(track_id(s, v)?) } else { None };
            s.import_files(&paths, track, fv(v, "at").unwrap_or(0.0));
            ok(format!("{} fichier(s) en cours d'import.", paths.len()))
        }
        "add_reference" => {
            let p = PathBuf::from(sv(v, "path").ok_or("path attendu")?);
            s.add_reference(&p);
            ok("Référence ajoutée.")
        }
        "new_project" => {
            crate::ui::new_project_studio(s);
            ok("Nouveau projet.")
        }
        "open_project" => {
            let p = PathBuf::from(sv(v, "path").ok_or("path attendu (dossier .awra)")?);
            let doc = studio_core::Document::open(&p).map_err(|e| e.to_string())?;
            s.set_document(doc);
            s.settings.add_recent(&p.to_string_lossy());
            ok(format!("Projet « {} » ouvert.", s.doc.project.name))
        }
        "load_demo" => {
            s.ui_actions.push(UiAction::Demo);
            ok("Projet de démonstration chargé.")
        }
        // history
        "undo" => {
            s.undo();
            ok("Annulé.")
        }
        "redo" => {
            s.redo();
            ok("Rétabli.")
        }
        // session
        "host_session" => {
            s.host_session();
            let code = s.session.as_ref().and_then(|x| x.invite.clone());
            Ok(json!({"ok": code.is_some(), "invite_code": code, "say": "Session ouverte."}))
        }
        "join_session" => {
            s.join_session(&sv(v, "code").ok_or("code attendu")?);
            ok("Connexion en cours.")
        }
        "leave_session" => {
            s.leave_session();
            ok("Session quittée.")
        }
        "send_chat" => {
            if s.session.is_none() {
                return Err("aucune session ouverte".into());
            }
            s.send_chat(&sv(v, "text").ok_or("text attendu")?);
            ok("Message envoyé.")
        }
        "scan_plugins" => {
            s.scan_plugins();
            ok("Recherche des plugins lancée.")
        }
        "toast" => {
            s.toasts.push(sv(v, "text").unwrap_or_default());
            ok("Message affiché.")
        }
        other => Err(format!("action inconnue : {other}")),
    }
}

/// A device by (part of) its name.
fn match_device(q: &str, list: &[String]) -> Result<String, String> {
    let l = q.to_lowercase();
    list.iter()
        .find(|d| d.to_lowercase() == l)
        .or_else(|| list.iter().find(|d| d.to_lowercase().contains(&l)))
        .cloned()
        .ok_or_else(|| format!("« {q} » introuvable ; disponibles : {}", if list.is_empty() { "aucun".into() } else { list.join(", ") }))
}

/// Actions done on the agent's thread (no studio state needed).
pub fn worker_action(v: &Value) -> Option<Result<Value, String>> {
    match sv(v, "action")?.as_str() {
        "list_audio_devices" => {
            let d = studio_engine::list_devices();
            Some(Ok(json!({"inputs": d.inputs, "outputs": d.outputs, "system_input": d.default_input, "system_output": d.default_output})))
        }
        "list_files" => Some(list_files(v)),
        _ => None,
    }
}

fn list_files(v: &Value) -> Result<Value, String> {
    let dir = sv(v, "dir").map(PathBuf::from).or_else(dirs::home_dir).ok_or("dossier inconnu")?;
    let dir = if dir.starts_with("~") { dirs::home_dir().unwrap_or_default().join(dir.strip_prefix("~").unwrap_or(&dir)) } else { dir };
    let audio_only = bv(v, "audio_only").unwrap_or(false);
    let mut entries = Vec::new();
    for e in std::fs::read_dir(&dir).map_err(|e| format!("{} : {e}", dir.display()))?.flatten().take(400) {
        let p = e.path();
        let is_dir = p.is_dir();
        let name = e.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue;
        }
        let project = is_dir && p.join("project.json").exists();
        if audio_only && !is_dir && !studio_dsp::io::is_audio_file(&p) {
            continue;
        }
        entries.push(json!({"name": name, "path": p.display().to_string(), "dir": is_dir && !project, "awra_project": project,
                            "size_kb": if is_dir { None } else { e.metadata().ok().map(|m| m.len() / 1024) }}));
    }
    Ok(json!({"dir": dir.display().to_string(), "entries": entries}))
}
