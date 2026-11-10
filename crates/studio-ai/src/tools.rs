//! The agent's tools: JSON schemas (for LLM function calling) and their
//! execution against a [`StudioHost`].

use serde_json::{json, Value};
use studio_core::effects::{self, EffectKind, PluginFormat};
use studio_core::model::*;
use studio_core::ops::{chain, Op, ProjectField, Target, TrackField, Transaction};

use crate::engineer::{self, VocalStyle};
use crate::host::{LayoutCommand, StudioHost};

pub struct ToolDef {
    pub name: &'static str,
    pub description: &'static str,
    pub parameters: Value,
}

fn obj(props: Value, required: &[&str]) -> Value {
    json!({"type": "object", "properties": props, "required": required})
}

pub fn definitions() -> Vec<ToolDef> {
    let effect_keys: Vec<&str> = effects::BUILTIN.iter().map(|k| k.key()).collect();
    let track = json!({"type": "string", "description": "Nom ou id de la piste ; « master » pour le bus master."});
    vec![
        ToolDef {
            name: "get_project",
            description: "Lit l'état du projet : pistes (type, volume, pan, effets et paramètres), master, tempo, tonalité, paroles, références, variantes.",
            parameters: obj(json!({}), &[]),
        },
        ToolDef {
            name: "analyze_track",
            description: "Écoute une piste et renvoie ses mesures (loudness, bruit de fond, bandes de fréquences, sifflantes, réverbération, justesse, tonalité) et un diagnostic en mots. processed=false écoute la prise brute, true avec ses effets.",
            parameters: obj(json!({"track": track, "processed": {"type": "boolean"}}), &["track"]),
        },
        ToolDef {
            name: "analyze_mix",
            description: "Écoute le mix complet (loudness, crête, équilibre tonal, largeur) et la loudness de chaque piste traitée.",
            parameters: obj(json!({}), &[]),
        },
        ToolDef {
            name: "analyze_reference",
            description: "Écoute un morceau de référence importé par l'utilisateur.",
            parameters: obj(json!({"reference": {"type": "string", "description": "Nom ou id ; vide = la dernière."}}), &[]),
        },
        ToolDef {
            name: "set_track",
            description: "Change le volume (dB), le panoramique (-1 gauche .. 1 droite), mute, solo, nom ou type d'une piste.",
            parameters: obj(
                json!({"track": track, "volume_db": {"type": "number"}, "pan": {"type": "number"}, "mute": {"type": "boolean"},
                       "solo": {"type": "boolean"}, "name": {"type": "string"},
                       "kind": {"type": "string", "enum": ["instrumental", "vocal", "adlib", "audio"]}}),
                &["track"],
            ),
        },
        ToolDef {
            name: "add_effect",
            description: "Ajoute un effet intégré à une piste ou au master, avec des paramètres optionnels (voir get_effect_params).",
            parameters: obj(
                json!({"track": track, "effect": {"type": "string", "enum": effect_keys}, "params": {"type": "object", "additionalProperties": {"type": "number"}},
                       "position": {"type": "integer", "description": "Index dans la chaîne ; absent = à la fin."}}),
                &["track", "effect"],
            ),
        },
        ToolDef {
            name: "set_effect",
            description: "Règle les paramètres d'un effet existant (désigné par son id ou son type, ex. « eq », « autotune ») et/ou l'active/désactive.",
            parameters: obj(
                json!({"track": track, "effect": {"type": "string"}, "params": {"type": "object", "additionalProperties": {"type": "number"}},
                       "enabled": {"type": "boolean"}}),
                &["track", "effect"],
            ),
        },
        ToolDef {
            name: "remove_effect",
            description: "Retire un effet d'une chaîne.",
            parameters: obj(json!({"track": track, "effect": {"type": "string"}}), &["track", "effect"]),
        },
        ToolDef {
            name: "get_effect_params",
            description: "Liste les paramètres d'un type d'effet (id, plage, défaut, unité).",
            parameters: obj(json!({"effect": {"type": "string", "enum": effect_keys}}), &["effect"]),
        },
        ToolDef {
            name: "build_vocal_chain",
            description: "Analyse une voix et construit une chaîne complète adaptée (débruitage, gate, EQ, compression, de-esser, autotune, doubleur, delay, réverbe) dans un style : clean, natural, autotune, melodic, aggressive, ambient. Remplace la chaîne existante.",
            parameters: obj(
                json!({"track": track, "style": {"type": "string", "enum": ["clean", "natural", "autotune", "melodic", "aggressive", "ambient"]}}),
                &["track"],
            ),
        },
        ToolDef {
            name: "balance_mix",
            description: "Équilibre les volumes et le panoramique de toutes les pistes à partir de leur loudness (voix au-dessus de l'instru, adlibs en retrait et écartés, deux voix principales légèrement écartées).",
            parameters: obj(json!({}), &[]),
        },
        ToolDef {
            name: "master",
            description: "Construit le mastering (EQ, compression de bus, largeur, limiteur) et calibre le niveau final. style : streaming (défaut), loud, dynamic, warm. target_lufs optionnel.",
            parameters: obj(json!({"style": {"type": "string"}, "target_lufs": {"type": "number"}}), &[]),
        },
        ToolDef {
            name: "match_reference",
            description: "Rapproche une voix (et le master) de l'esthétique d'un morceau de référence : équilibre tonal, réverbe, style d'autotune, largeur, niveau.",
            parameters: obj(json!({"track": track, "reference": {"type": "string"}}), &["track"]),
        },
        ToolDef {
            name: "create_variants",
            description: "Prépare plusieurs variantes de mix comparables (A/B) d'une voix dans différents styles, sans changer le mix courant.",
            parameters: obj(
                json!({"track": track, "styles": {"type": "array", "items": {"type": "string"}}}),
                &[],
            ),
        },
        ToolDef {
            name: "apply_variant",
            description: "Applique une variante de mix enregistrée (par son nom).",
            parameters: obj(json!({"name": {"type": "string"}}), &["name"]),
        },
        ToolDef {
            name: "finish_song",
            description: "Termine le morceau d'un coup : chaînes de voix, place pour les voix dans l'instru, équilibre, mastering. style de voix optionnel.",
            parameters: obj(json!({"style": {"type": "string"}, "master_style": {"type": "string"}}), &[]),
        },
        ToolDef {
            name: "set_layout",
            description: "Modifie l'interface : action show/hide/move/resize/preset/only/focus_track. Panneaux : timeline, lyrics, assistant, effects, mixer, master, session, browser, plugins, history. Zones : left, right, bottom, center. Presets : default, recording, writing, mixing, minimal.",
            parameters: obj(
                json!({"action": {"type": "string", "enum": ["show", "hide", "move", "resize", "preset", "only", "focus_track"]},
                       "panel": {"type": "string"}, "area": {"type": "string"}, "factor": {"type": "number"},
                       "preset": {"type": "string"}, "panels": {"type": "array", "items": {"type": "string"}}, "track": {"type": "string"}}),
                &["action"],
            ),
        },
        ToolDef {
            name: "set_song_info",
            description: "Change le tempo (bpm) et/ou la tonalité (ex. « A minor », « Do majeur ») du projet ; l'autotune s'en sert.",
            parameters: obj(json!({"bpm": {"type": "number"}, "key": {"type": "string"}}), &[]),
        },
        ToolDef {
            name: "add_lyrics",
            description: "Ajoute une section de paroles (titre, texte) au carnet de paroles.",
            parameters: obj(json!({"title": {"type": "string"}, "text": {"type": "string"}}), &["title", "text"]),
        },
        ToolDef {
            name: "list_plugins",
            description: "Liste les plugins VST3 / CLAP installés sur la machine.",
            parameters: obj(json!({}), &[]),
        },
        ToolDef {
            name: "add_plugin",
            description: "Insère un plugin VST3 / CLAP (par nom) sur une piste.",
            parameters: obj(json!({"track": track, "plugin": {"type": "string"}}), &["track", "plugin"]),
        },
        ToolDef {
            name: "undo_last_ai",
            description: "Annule la dernière modification faite par l'IA.",
            parameters: obj(json!({}), &[]),
        },
    ]
}

/// The result of a tool call.
#[derive(Clone, Debug, Default)]
pub struct ToolOutcome {
    /// What the model reads back.
    pub output: String,
    /// Transactions committed (id, label), for the UI's undo buttons.
    pub committed: Vec<(Id, String)>,
    pub error: bool,
}

impl ToolOutcome {
    fn ok(s: impl Into<String>) -> ToolOutcome {
        ToolOutcome { output: s.into(), ..Default::default() }
    }
    fn err(s: impl Into<String>) -> ToolOutcome {
        ToolOutcome { output: format!("Erreur : {}", s.into()), error: true, ..Default::default() }
    }
}

fn s<'a>(v: &'a Value, k: &str) -> Option<&'a str> {
    v.get(k).and_then(Value::as_str).filter(|s| !s.trim().is_empty())
}
fn f(v: &Value, k: &str) -> Option<f64> {
    v.get(k).and_then(|x| x.as_f64().or_else(|| x.as_str().and_then(|s| s.trim().parse().ok())))
}
fn b(v: &Value, k: &str) -> Option<bool> {
    v.get(k).and_then(Value::as_bool)
}

/// Resolves what users call a track ("la voix", "adlibs", "instru", a name).
pub fn resolve_track(p: &Project, q: &str) -> Option<Target> {
    let ql = q.trim().to_lowercase();
    if ql == "master" || ql == "bus master" || ql == "mix" {
        return Some(Target::Master);
    }
    if let Some(t) = p.find_track(q) {
        return Some(Target::Track(t.id));
    }
    let kind = TrackKind::parse(ql.trim_start_matches("la ").trim_start_matches("les ").trim_start_matches("l'"))
        .or_else(|| if ql.contains("voix") || ql.contains("vocal") { Some(TrackKind::Vocal) } else { None })
        .or_else(|| if ql.contains("adlib") || ql.contains("back") { Some(TrackKind::Adlib) } else { None })
        .or_else(|| if ql.contains("instru") || ql.contains("beat") || ql.contains("prod") { Some(TrackKind::Instrumental) } else { None })?;
    p.tracks.iter().find(|t| t.kind == kind).map(|t| Target::Track(t.id))
}

fn target_name(p: &Project, t: Target) -> String {
    match t {
        Target::Master => "master".into(),
        Target::Track(id) => p.track(id).map(|t| t.name.clone()).unwrap_or_default(),
    }
}

/// Finds a slot by id or kind key in a chain.
fn find_slot(p: &Project, t: Target, q: &str) -> Option<EffectSlot> {
    let ch = chain(p, t)?;
    if let Ok(id) = q.parse::<Id>() {
        if let Some(s) = ch.iter().find(|s| s.id == id) {
            return Some(s.clone());
        }
    }
    let kind = EffectKind::from_key(q);
    ch.iter()
        .rev()
        .find(|s| Some(&s.kind) == kind.as_ref() || effects::display_name(&s.kind).to_lowercase().contains(&q.to_lowercase()))
        .cloned()
}

pub fn project_json(p: &Project) -> Value {
    let chain_json = |c: &Vec<EffectSlot>| -> Value {
        c.iter()
            .map(|s| {
                json!({"id": s.id.to_string(), "effect": s.kind.key(), "name": effects::display_name(&s.kind),
                       "enabled": s.enabled, "params": s.params, "by": format!("{:?}", s.added_by)})
            })
            .collect()
    };
    json!({
        "name": p.name, "bpm": p.bpm, "key": p.key_name(), "duration_s": (p.duration() * 10.0).round() / 10.0,
        "tracks": p.tracks.iter().map(|t| json!({
            "id": t.id.to_string(), "name": t.name, "kind": t.kind, "volume_db": t.volume_db, "pan": t.pan,
            "mute": t.mute, "solo": t.solo, "clips": t.clips.len(), "owner": t.owner,
            "effects": chain_json(&t.effects)})).collect::<Vec<_>>(),
        "master": {"volume_db": p.master.volume_db, "effects": chain_json(&p.master.effects)},
        "lyrics": p.lyrics.iter().map(|l| l.title.clone()).collect::<Vec<_>>(),
        "references": p.references.iter().map(|r| json!({"id": r.id.to_string(), "name": r.name, "note": r.note})).collect::<Vec<_>>(),
        "variants": p.variants.iter().map(|v| v.name.clone()).collect::<Vec<_>>(),
    })
}

fn commit(host: &mut dyn StudioHost, out: &mut ToolOutcome, label: &str, ops: Vec<Op>) -> Result<(), String> {
    if ops.is_empty() {
        return Ok(());
    }
    let id = host.commit(Transaction::new(label, Actor::Ai, ops))?;
    out.committed.push((id, label.to_string()));
    Ok(())
}

/// Parses "A minor", "La mineur", "F#m", "Do" → (root, minor).
pub fn parse_key(s: &str) -> Option<(u8, bool)> {
    let l = s.trim().to_lowercase();
    let minor = l.contains("min") || l.contains("moll") || (l.ends_with('m') && !l.contains("maj"));
    let names: [(&str, u8); 19] = [
        ("do#", 1), ("ré#", 3), ("re#", 3), ("fa#", 6), ("sol#", 8), ("la#", 10), ("do", 0), ("ré", 2), ("re", 2),
        ("mi", 4), ("fa", 5), ("sol", 7), ("la", 9), ("si", 11), ("c#", 1), ("d#", 3), ("f#", 6), ("g#", 8), ("a#", 10),
    ];
    for (n, k) in names {
        if l.starts_with(n) {
            return Some((k, minor));
        }
    }
    let first = l.chars().next()?;
    let base = match first {
        'c' => 0,
        'd' => 2,
        'e' => 4,
        'f' => 5,
        'g' => 7,
        'a' => 9,
        'b' => 11,
        _ => return None,
    };
    let rest: String = l.chars().skip(1).collect();
    let k = if rest.starts_with('#') {
        base + 1
    } else if rest.starts_with('b') && !rest.starts_with("b ") {
        base + 11
    } else {
        base
    };
    Some(((k % 12) as u8, minor))
}

pub fn execute(name: &str, args: &Value, host: &mut dyn StudioHost) -> ToolOutcome {
    match run(name, args, host) {
        Ok(o) => o,
        Err(e) => ToolOutcome::err(e),
    }
}

fn need_track(p: &Project, args: &Value) -> Result<Target, String> {
    let q = s(args, "track").unwrap_or("voix");
    resolve_track(p, q).ok_or_else(|| format!("piste « {q} » introuvable"))
}

fn run(name: &str, args: &Value, host: &mut dyn StudioHost) -> Result<ToolOutcome, String> {
    let p = host.project();
    let mut out = ToolOutcome::default();
    match name {
        "get_project" => Ok(ToolOutcome::ok(project_json(&p).to_string())),
        "analyze_track" => {
            let Target::Track(id) = need_track(&p, args)? else { return host.analyze_mix().map(|a| ToolOutcome::ok(json!({"analysis": a}).to_string())) };
            let a = host.analyze_track(id, b(args, "processed").unwrap_or(false))?;
            let words = studio_analysis::describe(&a);
            Ok(ToolOutcome::ok(json!({"track": target_name(&p, Target::Track(id)), "diagnostic": words, "analysis": a}).to_string()))
        }
        "analyze_mix" => {
            let a = host.analyze_mix()?;
            let mut per = Vec::new();
            for t in &p.tracks {
                if t.clips.is_empty() {
                    continue;
                }
                if let Ok(ta) = host.analyze_track(t.id, true) {
                    per.push(json!({"track": t.name, "kind": t.kind, "lufs_at_0db": ta.loudness_lufs, "fader_db": t.volume_db}));
                }
            }
            Ok(ToolOutcome::ok(json!({"mix": a, "diagnostic": studio_analysis::describe(&a), "tracks": per}).to_string()))
        }
        "analyze_reference" => {
            let r = match s(args, "reference") {
                Some(q) => p.references.iter().find(|r| r.id.to_string() == q || r.name.to_lowercase().contains(&q.to_lowercase())),
                None => p.references.last(),
            }
            .ok_or("aucune référence importée (glisse un morceau dans « Références »)")?;
            let a = host.analyze_reference(r.id)?;
            Ok(ToolOutcome::ok(json!({"reference": r.name, "note": r.note, "analysis": a}).to_string()))
        }
        "set_track" => {
            let Target::Track(id) = need_track(&p, args)? else { return Err("le master n'est pas une piste".into()) };
            let mut ops = Vec::new();
            if let Some(v) = f(args, "volume_db") {
                ops.push(Op::SetTrack { id, field: TrackField::VolumeDb(v) });
            }
            if let Some(v) = f(args, "pan") {
                ops.push(Op::SetTrack { id, field: TrackField::Pan(v) });
            }
            if let Some(v) = b(args, "mute") {
                ops.push(Op::SetTrack { id, field: TrackField::Mute(v) });
            }
            if let Some(v) = b(args, "solo") {
                ops.push(Op::SetTrack { id, field: TrackField::Solo(v) });
            }
            if let Some(v) = s(args, "name") {
                ops.push(Op::SetTrack { id, field: TrackField::Name(v.into()) });
            }
            if let Some(k) = s(args, "kind").and_then(TrackKind::parse) {
                ops.push(Op::SetTrack { id, field: TrackField::Kind(k) });
            }
            let label = format!("Réglage de « {} »", target_name(&p, Target::Track(id)));
            commit(host, &mut out, &label, ops)?;
            out.output = "ok".into();
            Ok(out)
        }
        "add_effect" => {
            let t = need_track(&p, args)?;
            let key = s(args, "effect").ok_or("effet manquant")?;
            let kind = EffectKind::from_key(key).ok_or_else(|| format!("effet inconnu : {key}"))?;
            let mut slot = EffectSlot::new(kind.clone()).by(Actor::Ai);
            if let Some(params) = args.get("params").and_then(Value::as_object) {
                for (k, v) in params {
                    if let (Some(spec), Some(v)) = (effects::param_spec(&kind, k), v.as_f64()) {
                        slot.params.insert(k.clone(), spec.clamp(v));
                    }
                }
            }
            let len = chain(&p, t).map(|c| c.len()).unwrap_or(0);
            let index = args.get("position").and_then(Value::as_u64).map(|i| i as usize).unwrap_or(len);
            let id = slot.id;
            let label = format!("{} sur « {} »", effects::spec(&kind).name, target_name(&p, t));
            commit(host, &mut out, &label, vec![Op::AddEffect { target: t, slot, index }])?;
            out.output = json!({"effect_id": id.to_string()}).to_string();
            Ok(out)
        }
        "set_effect" => {
            let t = need_track(&p, args)?;
            let q = s(args, "effect").ok_or("effet manquant")?;
            let slot = match find_slot(&p, t, q) {
                Some(s) => s,
                None => {
                    // asked to tweak an effect that isn't there: add it
                    let kind = EffectKind::from_key(q).ok_or_else(|| format!("effet « {q} » absent de la chaîne"))?;
                    let s = EffectSlot::new(kind).by(Actor::Ai);
                    let len = chain(&p, t).map(|c| c.len()).unwrap_or(0);
                    commit(host, &mut out, "Ajout d'effet", vec![Op::AddEffect { target: t, slot: s.clone(), index: len }])?;
                    s
                }
            };
            let mut ops = Vec::new();
            if let Some(params) = args.get("params").and_then(Value::as_object) {
                for (k, v) in params {
                    let Some(v) = v.as_f64() else { continue };
                    if effects::param_spec(&slot.kind, k).is_none() && !slot.kind.is_plugin() {
                        return Err(format!("paramètre « {k} » inconnu pour {}", slot.kind.key()));
                    }
                    ops.push(Op::SetParam { target: t, slot: slot.id, param: k.clone(), value: v });
                }
            }
            if let Some(e) = b(args, "enabled") {
                ops.push(Op::SetEffectEnabled { target: t, slot: slot.id, enabled: e });
            }
            let label = format!("{} de « {} » ajusté", effects::display_name(&slot.kind), target_name(&p, t));
            commit(host, &mut out, &label, ops)?;
            out.output = "ok".into();
            Ok(out)
        }
        "remove_effect" => {
            let t = need_track(&p, args)?;
            let q = s(args, "effect").ok_or("effet manquant")?;
            let slot = find_slot(&p, t, q).ok_or_else(|| format!("effet « {q} » introuvable"))?;
            let label = format!("{} retiré de « {} »", effects::display_name(&slot.kind), target_name(&p, t));
            commit(host, &mut out, &label, vec![Op::RemoveEffect { target: t, slot: slot.id }])?;
            out.output = "ok".into();
            Ok(out)
        }
        "get_effect_params" => {
            let key = s(args, "effect").ok_or("effet manquant")?;
            let kind = EffectKind::from_key(key).ok_or("effet inconnu")?;
            let sp = effects::spec(&kind);
            let params: Vec<Value> = sp
                .params
                .iter()
                .map(|p| json!({"id": p.id, "label": p.label, "min": p.min, "max": p.max, "default": p.default,
                                "unit": format!("{:?}", p.unit), "choices": p.choices}))
                .collect();
            Ok(ToolOutcome::ok(json!({"effect": sp.name, "description": sp.description, "params": params}).to_string()))
        }
        "build_vocal_chain" => {
            let Target::Track(id) = need_track(&p, args)? else { return Err("choisis une piste de voix".into()) };
            let style = VocalStyle::parse(s(args, "style").unwrap_or("clean"));
            let t = p.track(id).ok_or("piste introuvable")?;
            let a = host.analyze_track(id, false).ok();
            let chain = engineer::vocal_chain(a.as_ref(), style, t.kind, p.key, p.minor);
            let names: Vec<String> = chain.iter().map(|s| effects::display_name(&s.kind)).collect();
            let label = format!("Chaîne voix « {} » sur « {} »", style.label(), t.name);
            commit(host, &mut out, &label, vec![Op::SetChain { target: Target::Track(id), chain }])?;
            let diag = a.map(|a| engineer::summary(&t.name, &a)).unwrap_or_default();
            out.output = json!({"diagnostic": diag, "chain": names}).to_string();
            Ok(out)
        }
        "balance_mix" => {
            let mut loud = Vec::new();
            for t in &p.tracks {
                if t.clips.is_empty() {
                    continue;
                }
                if let Ok(a) = host.analyze_track(t.id, true) {
                    loud.push((t.id, a.loudness_lufs as f64));
                }
            }
            let moves = engineer::balance(&p, &loud);
            let mut ops = Vec::new();
            for (id, v, pan) in &moves {
                ops.push(Op::SetTrack { id: *id, field: TrackField::VolumeDb((*v * 2.0).round() / 2.0) });
                ops.push(Op::SetTrack { id: *id, field: TrackField::Pan(*pan) });
            }
            commit(host, &mut out, "Équilibre du mix", ops)?;
            let desc: Vec<Value> = moves
                .iter()
                .filter_map(|(id, v, pan)| p.track(*id).map(|t| json!({"track": t.name, "volume_db": v, "pan": pan})))
                .collect();
            out.output = json!({"faders": desc}).to_string();
            Ok(out)
        }
        "master" => {
            let mut style = engineer::master_style(s(args, "style").unwrap_or("streaming"));
            if let Some(t) = f(args, "target_lufs") {
                style.target_lufs = t.clamp(-20.0, -6.0);
            }
            // calibrate the limiter: measure, correct, measure again
            let mut trial = p.clone();
            trial.master.effects = engineer::master_chain(style, 0.0);
            let l0 = host.measure_lufs(&trial)?;
            let mut gain = (style.target_lufs - l0).clamp(0.0, 20.0);
            for _ in 0..2 {
                trial.master.effects = engineer::master_chain(style, gain);
                let l = host.measure_lufs(&trial)?;
                gain = (gain + (style.target_lufs - l)).clamp(0.0, 24.0);
            }
            let chain = engineer::master_chain(style, (gain * 10.0).round() / 10.0);
            commit(
                host,
                &mut out,
                &format!("Mastering ({:.0} LUFS)", style.target_lufs),
                vec![Op::SetChain { target: Target::Master, chain }, Op::SetProject { field: ProjectField::MasterVolumeDb(0.0) }],
            )?;
            out.output = json!({"target_lufs": style.target_lufs, "before_lufs": l0, "limiter_gain_db": gain}).to_string();
            Ok(out)
        }
        "match_reference" => {
            let Target::Track(id) = need_track(&p, args)? else { return Err("choisis une piste de voix".into()) };
            let r = match s(args, "reference") {
                Some(q) => p.references.iter().find(|r| r.id.to_string() == q || r.name.to_lowercase().contains(&q.to_lowercase())),
                None => p.references.last(),
            }
            .ok_or("aucune référence importée")?;
            let ra = host.analyze_reference(r.id)?;
            let va = host.analyze_track(id, false)?;
            let m = studio_analysis::reference::match_reference(&va, &ra);
            let t = p.track(id).ok_or("piste introuvable")?;
            // start from a chain suited to the reference's style, then tune it
            let style = match m.autotune_speed {
                Some(sp) if sp > 0.9 => VocalStyle::Autotune,
                Some(sp) if sp > 0.6 => VocalStyle::Melodic,
                Some(_) => VocalStyle::Natural,
                None => VocalStyle::Clean,
            };
            let mut chain = engineer::vocal_chain(Some(&va), style, t.kind, p.key.or(ra.key.map(|k| k.0)), p.key.map(|_| p.minor).unwrap_or(ra.key.map(|k| k.1).unwrap_or(false)));
            for s in chain.iter_mut() {
                match s.kind {
                    EffectKind::Eq => {
                        for (k, v) in &m.eq {
                            let cur = s.param(k);
                            s.params.insert(k.clone(), (cur + v).clamp(-12.0, 12.0));
                        }
                    }
                    EffectKind::Reverb => {
                        if let Some(v) = m.reverb_mix {
                            s.params.insert("mix".into(), v);
                        }
                        if let Some(v) = m.reverb_size {
                            s.params.insert("size".into(), v);
                        }
                    }
                    EffectKind::Autotune => {
                        if let Some(v) = m.autotune_speed {
                            s.params.insert("speed".into(), v);
                        }
                        if let Some(v) = m.autotune_amount {
                            s.params.insert("amount".into(), v);
                        }
                    }
                    _ => {}
                }
            }
            if let Some(d) = m.doubler_mix {
                if !chain.iter().any(|s| s.kind == EffectKind::Doubler) {
                    let at = chain.len().saturating_sub(1);
                    chain.insert(at, EffectSlot::new(EffectKind::Doubler).with("mix", d).by(Actor::Ai));
                }
            }
            let label = format!("Esthétique de « {} » sur « {} »", r.name, t.name);
            commit(host, &mut out, &label, vec![Op::SetChain { target: Target::Track(id), chain }])?;
            out.output = json!({"found": m.notes, "style": style.key(), "target_lufs": m.target_lufs}).to_string();
            Ok(out)
        }
        "create_variants" => {
            let t = match s(args, "track") {
                Some(q) => match resolve_track(&p, q) {
                    Some(Target::Track(id)) => p.track(id).cloned(),
                    _ => None,
                },
                None => p.tracks.iter().find(|t| t.kind == TrackKind::Vocal).cloned(),
            }
            .ok_or("aucune voix à décliner")?;
            let styles: Vec<VocalStyle> = match args.get("styles").and_then(Value::as_array) {
                Some(a) if !a.is_empty() => a.iter().filter_map(Value::as_str).map(VocalStyle::parse).collect(),
                _ => vec![VocalStyle::Clean, VocalStyle::Melodic, VocalStyle::Aggressive],
            };
            let a = host.analyze_track(t.id, false).ok();
            let mut ops = Vec::new();
            let mut names = Vec::new();
            // the current mix first, to come back to it
            let mut current = p.snapshot_mix();
            ops.push(Op::AddVariant { variant: MixVariant { id: new_id(), name: "Avant l'IA".into(), snapshot: current.clone(), actor: Actor::Ai } });
            for st in styles {
                let chain = engineer::vocal_chain(a.as_ref(), st, t.kind, p.key, p.minor);
                if let Some(m) = current.tracks.get_mut(&t.id) {
                    m.effects = chain;
                }
                let name = format!("{} – {}", t.name, st.label());
                names.push(name.clone());
                ops.push(Op::AddVariant { variant: MixVariant { id: new_id(), name, snapshot: current.clone(), actor: Actor::Ai } });
            }
            commit(host, &mut out, "Variantes de mix", ops)?;
            out.output = json!({"variants": names, "hint": "apply_variant pour écouter une variante"}).to_string();
            Ok(out)
        }
        "apply_variant" => {
            let q = s(args, "name").ok_or("nom manquant")?.to_lowercase();
            let v = p
                .variants
                .iter()
                .rev()
                .find(|v| v.name.to_lowercase() == q)
                .or_else(|| p.variants.iter().rev().find(|v| v.name.to_lowercase().contains(&q)))
                .ok_or("variante introuvable")?;
            commit(host, &mut out, &format!("Variante « {} »", v.name), vec![Op::ApplyMix { mix: v.snapshot.clone() }])?;
            out.output = "ok".into();
            Ok(out)
        }
        "finish_song" => {
            let style = VocalStyle::parse(s(args, "style").unwrap_or("clean"));
            let mut notes = Vec::new();
            let has_vocals = p.tracks.iter().any(|t| matches!(t.kind, TrackKind::Vocal | TrackKind::Adlib) && !t.clips.is_empty());
            for t in &p.tracks {
                if t.clips.is_empty() {
                    continue;
                }
                match t.kind {
                    TrackKind::Vocal | TrackKind::Adlib => {
                        let o = execute("build_vocal_chain", &json!({"track": t.id.to_string(), "style": style.key()}), host);
                        if o.error {
                            return Err(o.output);
                        }
                        out.committed.extend(o.committed);
                        notes.push(format!("chaîne {} sur {}", style.label().to_lowercase(), t.name));
                    }
                    TrackKind::Instrumental if has_vocals && t.effects.is_empty() => {
                        commit(host, &mut out, "Place pour les voix dans l'instru", vec![Op::SetChain { target: Target::Track(t.id), chain: engineer::instrumental_chain() }])?;
                        notes.push(format!("place faite aux voix dans {}", t.name));
                    }
                    _ => {}
                }
            }
            let o = execute("balance_mix", &json!({}), host);
            out.committed.extend(o.committed);
            if !o.error {
                notes.push("volumes et panoramiques équilibrés".into());
            }
            let o = execute("master", &json!({"style": s(args, "master_style").unwrap_or("streaming")}), host);
            out.committed.extend(o.committed);
            if !o.error {
                notes.push("mastering calibré".into());
            } else {
                notes.push(o.output);
            }
            out.output = json!({"done": notes}).to_string();
            Ok(out)
        }
        "set_layout" => {
            let action = s(args, "action").ok_or("action manquante")?;
            let panel = s(args, "panel").unwrap_or("").to_string();
            let cmd = match action {
                "show" => LayoutCommand::Show { panel },
                "hide" => LayoutCommand::Hide { panel },
                "move" => LayoutCommand::Move { panel, area: s(args, "area").unwrap_or("right").into() },
                "resize" => LayoutCommand::Resize { panel, factor: f(args, "factor").unwrap_or(1.3) },
                "preset" => LayoutCommand::Preset { name: s(args, "preset").or(s(args, "panel")).unwrap_or("default").into() },
                "only" => LayoutCommand::Only {
                    panels: args
                        .get("panels")
                        .and_then(Value::as_array)
                        .map(|a| a.iter().filter_map(Value::as_str).map(String::from).collect())
                        .unwrap_or_default(),
                },
                "focus_track" => LayoutCommand::FocusTrack { track: s(args, "track").unwrap_or("voix").into() },
                other => return Err(format!("action inconnue : {other}")),
            };
            Ok(ToolOutcome::ok(host.layout(cmd)?))
        }
        "set_song_info" => {
            let mut ops = Vec::new();
            if let Some(bpm) = f(args, "bpm") {
                ops.push(Op::SetProject { field: ProjectField::Bpm(bpm) });
            }
            if let Some(k) = s(args, "key") {
                let (root, minor) = parse_key(k).ok_or_else(|| format!("tonalité illisible : {k}"))?;
                ops.push(Op::SetProject { field: ProjectField::Key(Some(root)) });
                ops.push(Op::SetProject { field: ProjectField::Minor(minor) });
                // autotunes follow the key
                for t in &p.tracks {
                    for sl in t.effects.iter().filter(|s| s.kind == EffectKind::Autotune) {
                        ops.push(Op::SetParam { target: Target::Track(t.id), slot: sl.id, param: "key".into(), value: root as f64 });
                        ops.push(Op::SetParam { target: Target::Track(t.id), slot: sl.id, param: "scale".into(), value: if minor { 1.0 } else { 0.0 } });
                    }
                }
            }
            commit(host, &mut out, "Tempo / tonalité", ops)?;
            out.output = "ok".into();
            Ok(out)
        }
        "add_lyrics" => {
            let section = LyricSection {
                id: new_id(),
                title: s(args, "title").unwrap_or("Idées").into(),
                text: s(args, "text").unwrap_or("").into(),
                start: None,
            };
            let index = p.lyrics.len();
            commit(host, &mut out, "Paroles proposées par l'IA", vec![Op::AddSection { section, index }])?;
            out.output = "ok".into();
            Ok(out)
        }
        "list_plugins" => {
            let list: Vec<Value> = host
                .plugins()
                .iter()
                .map(|p| json!({"name": p.name, "vendor": p.vendor, "format": p.format, "category": p.category}))
                .collect();
            Ok(ToolOutcome::ok(json!({"plugins": list}).to_string()))
        }
        "add_plugin" => {
            let t = need_track(&p, args)?;
            let q = s(args, "plugin").ok_or("plugin manquant")?.to_lowercase();
            let pl = host
                .plugins()
                .into_iter()
                .find(|x| x.name.to_lowercase().contains(&q))
                .ok_or_else(|| format!("aucun plugin « {q} » installé"))?;
            let kind = EffectKind::Plugin { format: pl.format, path: pl.path.clone(), plugin_id: pl.id.clone(), name: pl.name.clone() };
            let slot = EffectSlot { id: new_id(), kind, enabled: true, params: Default::default(), plugin_state: None, added_by: Actor::Ai };
            let len = chain(&p, t).map(|c| c.len()).unwrap_or(0);
            let fmt = if pl.format == PluginFormat::Clap { "CLAP" } else { "VST3" };
            commit(host, &mut out, &format!("{} ({fmt}) sur « {} »", pl.name, target_name(&p, t)), vec![Op::AddEffect { target: t, slot, index: len }])?;
            out.output = "ok".into();
            Ok(out)
        }
        "undo_last_ai" => Ok(ToolOutcome::ok(host.undo_ai()?)),
        other => Err(format!("outil inconnu : {other}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys() {
        assert_eq!(parse_key("A minor"), Some((9, true)));
        assert_eq!(parse_key("La mineur"), Some((9, true)));
        assert_eq!(parse_key("F#m"), Some((6, true)));
        assert_eq!(parse_key("Do majeur"), Some((0, false)));
        assert_eq!(parse_key("Bb"), Some((10, false)));
        assert_eq!(parse_key("sol"), Some((7, false)));
    }
}
