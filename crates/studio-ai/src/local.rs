//! The local engineer: understands common requests (French and English)
//! without any language model and turns them into tool calls. It makes
//! the studio useful offline and is the fallback when a provider fails.

use serde_json::{json, Value};
use studio_core::effects::EffectKind;
use studio_core::model::*;
use studio_core::ops::Target;

use crate::host::StudioHost;
use crate::tools::{self, resolve_track, ToolOutcome};

/// Lowercase without accents, for matching.
pub fn normalize(s: &str) -> String {
    s.to_lowercase()
        .chars()
        .map(|c| match c {
            'à' | 'â' | 'ä' => 'a',
            'é' | 'è' | 'ê' | 'ë' => 'e',
            'î' | 'ï' => 'i',
            'ô' | 'ö' => 'o',
            'ù' | 'û' | 'ü' => 'u',
            'ç' => 'c',
            '’' => '\'',
            c => c,
        })
        .collect()
}

fn has(t: &str, words: &[&str]) -> bool {
    words.iter().any(|w| t.contains(w))
}

/// A planned call and the sentence that explains it.
#[derive(Clone, Debug)]
pub struct Step {
    pub tool: &'static str,
    pub args: Value,
    pub say: String,
}

fn step(tool: &'static str, args: Value, say: impl Into<String>) -> Step {
    Step { tool, args, say: say.into() }
}

const PANEL_WORDS: [(&str, &[&str]); 10] = [
    ("timeline", &["timeline", "pistes", "multipiste", "frise"]),
    ("lyrics", &["parole", "lyrics", "texte", "carnet"]),
    ("assistant", &["assistant", "ia", "chat", "ingenieur"]),
    ("effects", &["effet", "reglage", "autotune", "inspecteur"]),
    ("mixer", &["mixer", "table de mix", "console", "faders"]),
    ("master", &["mastering", "master"]),
    ("session", &["session", "collab", "participant"]),
    ("browser", &["fichier", "navigateur", "reference", "browser"]),
    ("plugins", &["plugin", "vst", "clap"]),
    ("history", &["historique", "history"]),
];

fn panels_in(t: &str) -> Vec<&'static str> {
    PANEL_WORDS.iter().filter(|(_, w)| has(t, w)).map(|(p, _)| *p).collect()
}

/// Tracks the request is about.
fn targets(p: &Project, t: &str, selected: Option<Id>) -> Vec<Id> {
    let mut out: Vec<Id> = p.tracks.iter().filter(|tr| t.contains(&normalize(&tr.name))).map(|tr| tr.id).collect();
    if !out.is_empty() {
        return out;
    }
    let of = |k: TrackKind| p.tracks.iter().filter(move |tr| tr.kind == k).map(|tr| tr.id);
    if has(t, &["adlib", "backs", "back ", "doubl"]) {
        out.extend(of(TrackKind::Adlib));
    }
    if has(t, &["instru", "beat", "prod"]) {
        out.extend(of(TrackKind::Instrumental));
    }
    if has(t, &["nos voix", "nos deux voix", "les voix", "toutes les voix", "nos 2 voix", "deux voix", "both"]) {
        out.extend(of(TrackKind::Vocal));
        out.extend(of(TrackKind::Adlib));
    } else if has(t, &["deuxieme voix", "seconde voix", "voix 2", "2e voix"]) {
        out.extend(of(TrackKind::Vocal).skip(1).take(1));
    } else if out.is_empty() {
        if let Some(s) = selected.filter(|s| p.track(*s).is_some_and(|t| t.kind != TrackKind::Instrumental)) {
            out.push(s);
        } else {
            out.extend(of(TrackKind::Vocal).take(1));
        }
    }
    out.dedup();
    out
}

fn chain_has(p: &Project, id: Id, k: &EffectKind) -> Option<EffectSlot> {
    p.track(id)?.effects.iter().find(|s| &s.kind == k).cloned()
}

/// Plans the tool calls for a request. Empty when not understood.
pub fn plan(p: &Project, request: &str, selected: Option<Id>) -> Vec<Step> {
    let t = normalize(request);
    let mut steps = Vec::new();
    let name_of = |id: Id| p.track(id).map(|t| t.name.clone()).unwrap_or_default();

    // undo
    if has(&t, &["annule", "undo", "reviens en arriere", "remets comme avant", "retour en arriere"]) {
        return vec![step("undo_last_ai", json!({}), "J'annule ma dernière intervention.")];
    }

    // interface
    let layout_words = has(&t, &["interface", "espace de travail", "workspace", "a droite", "a gauche", "en bas", "agrandi", "plus grand", "reduis", "plus petit", "affiche", "montre", "cache", "masque", "simplifi", "panneau", "sous ma piste", "sous la piste", "dispos"]);
    if layout_words {
        if has(&t, &["simplifi", "minimal", "epure", "que les outils", "seulement les outils"]) {
            steps.push(step("set_layout", json!({"action": "preset", "preset": "minimal"}), "J'épure l'interface : timeline, effets et assistant seulement."));
        } else if has(&t, &["enregistr", "record"]) && has(&t, &["espace", "interface", "mode", "optimis"]) {
            steps.push(step("set_layout", json!({"action": "preset", "preset": "recording"}), "Voici un espace taillé pour enregistrer : grande timeline, paroles sous les yeux, réglages de la voix."));
        } else if has(&t, &["ecri", "parole"]) && has(&t, &["espace", "mode", "optimis"]) {
            steps.push(step("set_layout", json!({"action": "preset", "preset": "writing"}), "Espace d'écriture : les paroles en grand, l'instru à portée."));
        } else if has(&t, &["mix"]) && has(&t, &["espace", "mode", "optimis"]) {
            steps.push(step("set_layout", json!({"action": "preset", "preset": "mixing"}), "Espace de mixage : table de mixage, effets et mastering."));
        } else if has(&t, &["par defaut", "d'origine", "reinitialise"]) {
            steps.push(step("set_layout", json!({"action": "preset", "preset": "default"}), "Interface remise par défaut."));
        } else if has(&t, &["sous ma piste", "sous la piste", "sous le piste"]) {
            let id = targets(p, &t, selected).first().copied();
            let track = id.map(name_of).unwrap_or_else(|| "voix".into());
            steps.push(step("set_layout", json!({"action": "focus_track", "track": track}), format!("Les réglages de « {track} » sont maintenant juste sous la timeline.")));
            steps.push(step("set_layout", json!({"action": "move", "panel": "effects", "area": "bottom"}), ""));
        } else {
            // clause by clause: "mets les paroles à droite et agrandis la timeline"
            for clause in t.split([',', ';', '.']).flat_map(|c| c.split(" et ")) {
                for panel in panels_in(clause) {
                    // the panel's name as the interface shows it
                    let name = crate::host::PANELS.iter().find(|p| p.0 == panel).map(|p| p.1).unwrap_or(panel);
                    let area = if has(clause, &["a droite"]) {
                        Some("right")
                    } else if has(clause, &["a gauche"]) {
                        Some("left")
                    } else if has(clause, &["en bas", "dessous"]) {
                        Some("bottom")
                    } else if has(clause, &["au centre", "au milieu"]) {
                        Some("center")
                    } else {
                        None
                    };
                    if let Some(area) = area {
                        steps.push(step("set_layout", json!({"action": "move", "panel": panel, "area": area}), format!("« {name} » passe {}.", match area {
                            "right" => "à droite",
                            "left" => "à gauche",
                            "bottom" => "en bas",
                            _ => "au centre",
                        })));
                    } else if has(clause, &["agrandi", "plus grand", "plus de place", "elargi"]) {
                        steps.push(step("set_layout", json!({"action": "resize", "panel": panel, "factor": 1.4}), format!("J'agrandis « {name} ».")));
                    } else if has(clause, &["reduis", "plus petit", "rapetiss"]) {
                        steps.push(step("set_layout", json!({"action": "resize", "panel": panel, "factor": 0.7}), format!("Je réduis « {name} ».")));
                    } else if has(clause, &["cache", "masque", "enleve", "ferme", "retire"]) {
                        steps.push(step("set_layout", json!({"action": "hide", "panel": panel}), format!("« {name} » est masqué.")));
                    } else if has(clause, &["affiche", "montre", "ouvre", "ajoute"]) {
                        steps.push(step("set_layout", json!({"action": "show", "panel": panel}), format!("« {name} » est affiché.")));
                    }
                }
            }
        }
        if !steps.is_empty() {
            return steps;
        }
    }

    // song info: key / tempo from the beat
    if has(&t, &["tonalite", "la gamme", "bpm", "tempo"]) && has(&t, &["detect", "trouve", "quelle", "c'est quoi", "regle", "cale"]) {
        if let Some(beat) = p.tracks.iter().find(|tr| tr.kind == TrackKind::Instrumental) {
            steps.push(step("analyze_track", json!({"track": beat.id.to_string()}), "J'écoute l'instru pour trouver sa tonalité et son tempo."));
            return steps;
        }
    }

    let vocals = targets(p, &t, selected);
    let style_word = if has(&t, &["melodi"]) {
        Some("melodic")
    } else if has(&t, &["agressi", "drill", "hard", "energique"]) && !has(&t, &["moins agress", "trop agress"]) {
        Some("aggressive")
    } else if has(&t, &["planant", "aerien", "ambian"]) && !has(&t, &["reference"]) {
        Some("ambient")
    } else {
        None
    };

    // whole song
    if has(&t, &["finis", "finalise", "termine", "fini le son", "son pro", "pret a sortir", "finir le son", "finish"]) {
        let ms = if has(&t, &["fort", "loud", "club"]) { "loud" } else if has(&t, &["dynamique", "doux"]) { "dynamic" } else { "streaming" };
        steps.push(step(
            "finish_song",
            json!({"style": style_word.unwrap_or(if has(&t, &["autotune"]) { "autotune" } else { "clean" }), "master_style": ms}),
            "Je finalise le morceau : traitement des voix, place dans l'instru, équilibre puis mastering.",
        ));
        return steps;
    }

    // reference
    if has(&t, &["reference", "ce genre", "comme ce son", "comme ce morceau", "meme ambiance", "s'en inspir", "inspire"]) && !p.references.is_empty() {
        for id in &vocals {
            steps.push(step("match_reference", json!({"track": id.to_string()}), format!("J'analyse la référence et je rapproche « {} » de son esthétique.", name_of(*id))));
        }
        if has(&t, &["master", "niveau", "fort"]) {
            steps.push(step("master", json!({}), "Et le master suit le niveau de la référence."));
        }
        return steps;
    }

    if has(&t, &["variante", "compare", "plusieurs versions", "differentes versions", "a/b", "options de mix"]) {
        let id = vocals.first().copied();
        steps.push(step(
            "create_variants",
            json!({"track": id.map(|i| i.to_string())}),
            "Je prépare trois variantes (propre, mélodique, agressive) : tu peux les écouter et choisir.",
        ));
        return steps;
    }

    if has(&t, &["mastering", "master"]) && !has(&t, &["mix"]) {
        let style = if has(&t, &["fort", "loud", "club", "puissant"]) { "loud" } else if has(&t, &["dynamique", "doux"]) { "dynamic" } else if has(&t, &["chaud"]) { "warm" } else { "streaming" };
        steps.push(step("master", json!({"style": style}), "Mastering : égalisation légère, compression de bus, largeur et limiteur calibré au bon niveau."));
        return steps;
    }

    // mixing several voices together
    if has(&t, &["mix", "equilibr", "complet", "se completent", "ensemble"]) {
        let all_vocals: Vec<&Track> = p.tracks.iter().filter(|tr| matches!(tr.kind, TrackKind::Vocal | TrackKind::Adlib)).collect();
        for tr in all_vocals {
            if tr.effects.is_empty() && !tr.clips.is_empty() {
                steps.push(step("build_vocal_chain", json!({"track": tr.id.to_string(), "style": style_word.unwrap_or("clean")}), format!("Je traite « {} ».", tr.name)));
            }
        }
        steps.push(step("balance_mix", json!({}), "J'équilibre les volumes : les voix au-dessus de l'instru, les voix principales légèrement écartées pour qu'elles se complètent, les adlibs en retrait sur les côtés."));
        if has(&t, &["master"]) {
            steps.push(step("master", json!({}), "Puis le mastering."));
        }
        return steps;
    }

    // per-track requests
    for id in &vocals {
        let name = name_of(*id);
        let ids = id.to_string();
        if has(&t, &["propre", "nettoi", "nettoy", "clean", "pro "]) || style_word.is_some() {
            let style = style_word.unwrap_or(if has(&t, &["autotune"]) { "autotune" } else { "clean" });
            steps.push(step("build_vocal_chain", json!({"track": ids, "style": style}), format!("J'écoute « {name} » et je construis une chaîne adaptée, rendu {}.", match style {
                "natural" => "naturel",
                "autotune" => "autotune",
                "melodic" => "mélodique",
                "aggressive" => "agressif",
                "ambient" => "aérien",
                _ => "propre",
            })));
            continue;
        }
        if has(&t, &["autotune", "auto-tune", "auto tune", "justesse", "faux", "chante juste"]) {
            let (speed, amount, what) = if has(&t, &["fort", "robot", "marque", "t-pain", "a fond", "extreme"]) {
                (1.0, 1.0, "marqué, correction instantanée")
            } else if has(&t, &["naturel", "leger", "discret", "subtil", "transparent"]) {
                (0.35, 0.7, "naturel et discret")
            } else {
                (0.85, 1.0, "bien calibré")
            };
            let mut params = json!({"speed": speed, "amount": amount});
            if let Some(k) = p.key {
                params["key"] = json!(k);
                params["scale"] = json!(if p.minor { 1 } else { 0 });
            } else {
                params["scale"] = json!(2);
            }
            let beat = p.tracks.iter().find(|t| t.kind == TrackKind::Instrumental && !t.clips.is_empty());
            let note = match (p.key, beat) {
                (Some(_), _) => "",
                (None, Some(_)) => "",
                (None, None) => " (tonalité inconnue : gamme chromatique — dis-moi la tonalité)",
            };
            steps.push(step("set_effect", json!({"track": ids, "effect": "autotune", "params": params}), format!("Autotune {what} sur « {name} »{note}.")));
            if let (None, Some(b)) = (p.key, beat) {
                // calibrate on the beat's key (set_song_info retunes the autotunes)
                if !steps.iter().any(|s| s.tool == "analyze_track" && s.args["track"] == json!(b.id.to_string())) {
                    steps.push(step("analyze_track", json!({"track": b.id.to_string()}), "Je cale l'autotune sur la tonalité de l'instru."));
                }
            }
        }
        if has(&t, &["etouffe", "sourd", "terne", "eteint", "manque d'aigu", "pas clair", "pas assez clair", "muffled", "dull", "derriere un mur"]) {
            let eq = chain_has(p, *id, &EffectKind::Eq);
            let get = |k: &str| eq.as_ref().map(|e| e.param(k)).unwrap_or(0.0);
            steps.push(step(
                "set_effect",
                json!({"track": ids, "effect": "eq", "params": {"lowmid_gain": (get("lowmid_gain") - 3.0).max(-12.0), "lowmid_freq": 300.0,
                       "presence_gain": (get("presence_gain") + 3.0).min(10.0), "air_gain": (get("air_gain") + 2.5).min(10.0), "hp_freq": get("hp_freq").max(90.0)}}),
                format!("« {name} » : je retire de la boue vers 300 Hz et j'ouvre la présence et l'air."),
            ));
        }
        if has(&t, &["siffl", "les s ", "sifflante", "agress", "perce", "crie", "harsh"]) {
            steps.push(step("set_effect", json!({"track": ids, "effect": "deesser", "params": {"threshold_db": -36.0, "amount_db": 10.0}}), format!("De-esser sur « {name} » pour adoucir les sifflantes.")));
        }
        if has(&t, &["bruit", "souffle", "parasite", "ventil", "noise"]) {
            steps.push(step("set_effect", json!({"track": ids, "effect": "denoise", "params": {"amount": 0.7}}), format!("Débruitage de « {name} ».")));
            steps.push(step("set_effect", json!({"track": ids, "effect": "gate", "params": {"threshold_db": -55.0, "range_db": 20.0}}), "Et un gate qui coupe le bruit entre les phrases."));
        }
        if has(&t, &["reverb", "echo", "profondeur", "espace", "loin", "ambiance"]) {
            let rv = chain_has(p, *id, &EffectKind::Reverb);
            let cur = rv.as_ref().map(|r| r.param("mix")).unwrap_or(0.0);
            let less = has(&t, &["moins", "trop", "sec", "enleve", "retire"]);
            let mix = if less { (cur - 0.08).max(0.0) } else { (cur + 0.08).clamp(0.12, 0.5) };
            steps.push(step("set_effect", json!({"track": ids, "effect": "reverb", "params": {"mix": mix}}), format!("Réverbe de « {name} » {} ({:.0} %).", if less { "réduite" } else { "augmentée" }, mix * 100.0)));
        }
        let up = has(&t, &["ressortir", "plus fort", "monte", "en avant", "devant", "entend pas", "entend mal", "louder"]);
        let down = has(&t, &["moins fort", "baisse", "en retrait", "trop fort", "derriere", "quieter"]);
        if up || down {
            let cur = p.track(*id).map(|t| t.volume_db).unwrap_or(0.0);
            let v = if up { cur + 3.0 } else { cur - 3.0 };
            let mut say = format!("« {name} » {} de 3 dB.", if up { "monte" } else { "baisse" });
            let mut args = json!({"track": ids, "volume_db": v});
            if up && p.track(*id).is_some_and(|t| t.kind == TrackKind::Adlib) {
                // adlibs come out better wide and bright than just louder
                let side = if p.track(*id).map(|t| t.pan).unwrap_or(0.0) < 0.0 { -0.4 } else { 0.4 };
                args["pan"] = json!(side);
                say.push_str(" Je les écarte aussi sur le côté pour qu'elles ne masquent pas la voix principale.");
                steps.push(step("set_track", args, say));
                steps.push(step("set_effect", json!({"track": ids, "effect": "eq", "params": {"presence_gain": 3.0, "hp_freq": 180.0}}), ""));
            } else {
                steps.push(step("set_track", args, say));
            }
        }
    }
    if steps.is_empty() && has(&t, &["analyse", "ecoute", "diagnostic", "t'en penses", "tu en penses", "avis", "qu'est-ce qui va pas", "probleme"]) {
        for id in &vocals {
            steps.push(step("analyze_track", json!({"track": id.to_string()}), ""));
        }
        if vocals.is_empty() {
            steps.push(step("analyze_mix", json!({}), ""));
        }
    }
    let _ = Target::Master;
    let _ = resolve_track;
    steps
}

/// Runs a plan and writes the reply.
pub fn run(host: &mut dyn StudioHost, request: &str, selected: Option<Id>) -> (String, Vec<(Id, String)>) {
    run_with_progress(host, request, selected, &mut |_| {})
}

/// Same, saying what it is doing step by step.
pub fn run_with_progress(host: &mut dyn StudioHost, request: &str, selected: Option<Id>, progress: &mut dyn FnMut(&str)) -> (String, Vec<(Id, String)>) {
    if let Some(reply) = app_request(host, &normalize(request)) {
        return (reply, Vec::new());
    }
    let p = host.project();
    let steps = plan(&p, request, selected);
    if steps.is_empty() {
        return (help_text(&p), Vec::new());
    }
    let mut lines = Vec::new();
    let mut committed = Vec::new();
    for st in steps {
        progress(&crate::agent::tool_progress(st.tool));
        let o: ToolOutcome = tools::execute(st.tool, &st.args, host);
        if o.error {
            lines.push(o.output);
            continue;
        }
        if !st.say.is_empty() {
            lines.push(st.say.clone());
        }
        // add what the tools found
        if let Ok(v) = serde_json::from_str::<Value>(&o.output) {
            // listening to the beat for its key: its diagnostic is noise here
            let key_only = st.tool == "analyze_track" && v.pointer("/analysis/tempo_bpm").is_some_and(|t| t.is_number());
            if let Some(d) = v.get("diagnostic").filter(|_| !key_only) {
                match d {
                    Value::String(s) if !s.is_empty() => lines.push(s.clone()),
                    Value::Array(a) if !a.is_empty() => {
                        let name = v.get("track").and_then(Value::as_str).unwrap_or("la piste");
                        let words: Vec<&str> = a.iter().filter_map(Value::as_str).collect();
                        lines.push(format!("« {name} » : {}.", words.join(", ")));
                    }
                    Value::Array(_) => {
                        let name = v.get("track").and_then(Value::as_str).unwrap_or("la piste");
                        lines.push(format!("« {name} » sonne déjà bien, rien d'alarmant."));
                    }
                    _ => {}
                }
            }
            if let Some(a) = v.get("analysis") {
                if let (Some(k), Some(m)) = (a.pointer("/key/0").and_then(Value::as_u64), a.pointer("/key/1").and_then(Value::as_bool)) {
                    let bpm = a.get("tempo_bpm").and_then(Value::as_f64);
                    if bpm.is_some() {
                        let key = format!("{} {}", NOTE_NAMES[k as usize % 12], if m { "minor" } else { "major" });
                        lines.push(format!(
                            "Tonalité estimée : {} {}{}.",
                            NOTE_NAMES_FR[k as usize % 12],
                            if m { "mineur" } else { "majeur" },
                            bpm.map(|b| format!(", tempo ≈ {b:.0} BPM")).unwrap_or_default()
                        ));
                        let o2 = tools::execute("set_song_info", &json!({"key": key, "bpm": bpm}), host);
                        committed.extend(o2.committed);
                        lines.push("Je l'ai réglée sur le projet : l'autotune suit cette tonalité.".into());
                    }
                }
            }
            if let Some(found) = v.get("found").and_then(Value::as_array) {
                let words: Vec<&str> = found.iter().filter_map(Value::as_str).collect();
                lines.push(format!("Ce que j'entends dans la référence : {}.", words.join(", ")));
            }
            if let Some(done) = v.get("done").and_then(Value::as_array) {
                let words: Vec<&str> = done.iter().filter_map(Value::as_str).collect();
                lines.push(format!("Fait : {}.", words.join(" ; ")));
            }
            if let Some(t) = v.get("target_lufs").and_then(Value::as_f64) {
                if v.get("limiter_gain_db").is_some() {
                    lines.push(format!("Niveau final visé : {t:.0} LUFS (normes des plateformes de streaming)."));
                }
            }
        } else if !o.output.is_empty() && o.output != "ok" && st.tool != "set_layout" {
            lines.push(o.output.clone());
        }
        committed.extend(o.committed);
    }
    if !committed.is_empty() {
        lines.push("Tout est annulable (bouton « Annuler » ou « annule »).".into());
    }
    (lines.join("\n"), committed)
}

/// The device of a list the request names (by any word of its name).
fn named_device(t: &str, list: &[String]) -> Option<String> {
    list.iter()
        .find(|d| t.contains(&normalize(d)))
        .or_else(|| {
            list.iter().find(|d| {
                normalize(d).split(|c: char| !c.is_alphanumeric()).filter(|w| w.len() >= 3 && !matches!(*w, "micro" | "audio" | "sortie" | "entree" | "default")).any(|w| t.contains(w))
            })
        })
        .cloned()
}

fn say(v: Result<Value, String>) -> String {
    match v {
        Ok(v) => v.get("say").and_then(Value::as_str).unwrap_or("C'est fait.").to_string(),
        Err(e) => format!("Je n'ai pas pu : {e}."),
    }
}

/// "1:20", "1 min 20", "80 s", "à 45" → seconds.
fn time_in(t: &str) -> Option<f64> {
    let words: Vec<&str> = t.split_whitespace().collect();
    for (i, w) in words.iter().enumerate() {
        if let Some((m, s)) = w.split_once(':') {
            if let (Ok(m), Ok(s)) = (m.parse::<f64>(), s.trim_end_matches(|c: char| !c.is_ascii_digit()).parse::<f64>()) {
                return Some(m * 60.0 + s);
            }
        }
        if let Some((m, s)) = w.split_once("min") {
            if let Ok(m) = m.parse::<f64>() {
                let s = s.trim_end_matches('s').parse::<f64>().ok().or_else(|| words.get(i + 1).and_then(|x| x.trim_end_matches('s').parse().ok()));
                return Some(m * 60.0 + s.unwrap_or(0.0));
            }
        }
        if let Ok(n) = w.trim_end_matches('s').replace(',', ".").parse::<f64>() {
            let next = words.get(i + 1).copied().unwrap_or("");
            if next.starts_with("min") {
                let s = words.get(i + 2).and_then(|x| x.trim_end_matches('s').parse::<f64>().ok()).unwrap_or(0.0);
                return Some(n * 60.0 + s);
            }
            return Some(n);
        }
    }
    None
}

/// Requests about the application rather than the sound: devices,
/// monitoring, transport, files, appearance. None when it isn't one.
fn app_request(host: &mut dyn StudioHost, t: &str) -> Option<String> {
    let short = t.split_whitespace().count() <= 6;
    let device_words = has(t, &["micro", "entree", "input", "carte son", "interface audio", "sortie audio", "sortie son", "casque", "haut-parleur", "haut parleur", "enceinte", "peripherique"]);
    let asks = has(t, &["quel", "quelle", "c'est quoi", "lequel", "laquelle", "liste", "dispo", "j'ai", "as-tu", "info", "branche", "detect", "reconnai"]);
    let change = has(t, &["change", "utilise le", "utilise la", "utilise mon", "utilise ma", "prends", "passe sur", "passe sous", "bascule", "choisis", "selectionne", "mets le micro", "mets la sortie"]);
    // "passe la sortie sur…", "mets le casque", "bascule vers…"
    let change = change || (!asks && has(t, &["passe ", "mets ", "met ", "veux ", "vers "]));
    let output_words = has(t, &["sortie", "casque", "haut-parleur", "haut parleur", "enceinte", "ecoute sur", "son sort"]);

    // latency of the return / monitoring
    if has(t, &["latence", "retour micro", "retour casque", "retour de voix", "delai", "decalage"]) && !has(t, &["clip", "piste"]) {
        if has(t, &["reduis", "baisse", "diminue", "plus rapide", "moins", "optimise", "minimum", "instantan", "supprime", "enleve"]) {
            let r = host.app_action(&json!({"action": "set_audio", "buffer_frames": 64, "monitor_mode": if has(t, &["effet", "autotune"]) { "low_latency" } else { "direct" }}));
            let ms = r.as_ref().ok().and_then(|v| v.get("latency_ms")).and_then(Value::as_f64);
            return Some(format!(
                "Retour au plus court : tampon de 64 échantillons{}.{}",
                if has(t, &["effet", "autotune"]) { ", effets en faible latence" } else { " et retour direct (ta voix telle quelle, sans attendre les effets)" },
                ms.map(|m| format!(" Latence micro → casque ≈ {m:.0} ms.")).unwrap_or_default()
            ));
        }
        let a = host.app_state().ok()?;
        let au = a.get("audio")?;
        let ms = au.get("monitor_latency_ms").and_then(Value::as_f64);
        return Some(match ms {
            Some(m) => format!(
                "Ton retour micro met ≈ {m:.0} ms (tampon {} échantillons, mode {}). Dis « réduis la latence » pour le plus rapide possible.",
                au.get("buffer_frames").and_then(Value::as_u64).unwrap_or(0),
                match au.get("monitor_mode").and_then(Value::as_str) {
                    Some("direct") => "direct",
                    Some("full") => "avec tous les effets",
                    _ => "effets en faible latence",
                }
            ),
            None => "Aucun micro n'est ouvert, donc pas de retour. Choisis-en un avec le bouton micro en haut, ou dis « utilise le micro … ».".into(),
        });
    }

    // hear oneself outside a take
    if has(t, &["m'entendre", "entendre ma voix", "m'ecouter", "retour dans le casque", "monitoring"]) {
        let on = !has(t, &["coupe", "desactive", "arrete", "plus m'entendre", "enleve"]);
        return Some(say(host.app_action(&json!({"action": "set_audio", "monitor_armed": on}))).replace(
            "Réglage audio appliqué.",
            if on { "Tu t'entends maintenant dès que la piste est armée (mets un casque)." } else { "Tu ne t'entends plus qu'en enregistrant." },
        ));
    }

    if device_words && (change || (!asks && has(t, &["micro "]) && has(t, &["utilise", "prends"]))) {
        let d = host.app_action(&json!({"action": "list_audio_devices"})).ok()?;
        let list: Vec<String> = d.get(if output_words { "outputs" } else { "inputs" })?.as_array()?.iter().filter_map(|x| x.as_str().map(String::from)).collect();
        let key = if output_words { "output" } else { "input" };
        let target = if has(t, &["systeme", "par defaut", "defaut"]) {
            Some("system".to_string())
        } else if !output_words && has(t, &["aucun micro", "pas de micro", "sans micro", "coupe le micro"]) {
            Some("none".to_string())
        } else {
            named_device(t, &list)
        };
        return Some(match target {
            Some(n) => say(host.app_action(&json!({"action": "set_audio", key: n}))),
            None if list.is_empty() => "Je ne trouve aucun périphérique audio sur cette machine.".into(),
            None => format!("Lequel ? Disponibles : {}.", list.join(", ")),
        });
    }
    if device_words && asks {
        let a = host.app_state().ok()?;
        let au = a.get("audio")?;
        let list = |k: &str| au.get(k).and_then(Value::as_array).map(|l| l.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", ")).unwrap_or_default();
        let input = au.get("input").and_then(Value::as_str);
        let mut lines = vec![match (input, au.get("input_open").and_then(Value::as_bool).unwrap_or(false)) {
            (Some(i), true) => format!("Ton micro : {i}."),
            (Some(i), false) => format!("Micro choisi : {i}, mais il n'a pas pu être ouvert."),
            (None, _) => "Aucun micro n'est utilisé.".into(),
        }];
        if let Some(o) = au.get("output").and_then(Value::as_str) {
            lines.push(format!("Sortie : {o}."));
        }
        let ins = list("available_inputs");
        if !ins.is_empty() {
            lines.push(format!("Micros disponibles : {ins}."));
        }
        let outs = list("available_outputs");
        if !outs.is_empty() && output_words {
            lines.push(format!("Sorties disponibles : {outs}."));
        }
        if let Some(m) = au.get("monitor_latency_ms").and_then(Value::as_f64) {
            lines.push(format!("Retour micro ≈ {m:.0} ms."));
        }
        if let Some(e) = au.get("problem").and_then(Value::as_str) {
            lines.push(format!("Attention : {e}."));
        }
        lines.push("Pour changer : « utilise le micro … » ou le bouton micro en haut.".into());
        return Some(lines.join("\n"));
    }

    // transport
    if short && has(t, &["joue", "lecture", "play", "lance le son", "fais ecouter", "ecoute le morceau"]) && !has(t, &["joue avec", "rejoue"]) {
        return Some(say(host.app_action(&json!({"action": "play"}))));
    }
    if short && has(t, &["stop", "arrete", "pause", "coupe le son"]) && !has(t, &["enregistr", "micro"]) {
        return Some(say(host.app_action(&json!({"action": if has(t, &["stop"]) && !has(t, &["pause"]) { "stop" } else { "pause" }}))));
    }
    if has(t, &["va a ", "vas a ", "place-toi", "place toi", "aller a ", "saute a", "rembobine", "reviens au debut", "au debut"]) {
        let at = if has(t, &["debut", "rembobine"]) { Some(0.0) } else { time_in(t) };
        if let Some(at) = at {
            return Some(say(host.app_action(&json!({"action": "seek", "time": at}))));
        }
    }
    if has(t, &["metronome"]) && short {
        let on = !has(t, &["coupe", "desactive", "enleve", "arrete", "sans"]);
        return Some(say(host.app_action(&json!({"action": "metronome", "on": on}))));
    }
    if short && has(t, &["lance l'enregistrement", "enregistre-moi", "lance une prise", "commence la prise", "on enregistre", "record"]) {
        return Some(say(host.app_action(&json!({"action": "record", "on": true}))));
    }
    if short && has(t, &["arrete l'enregistrement", "stoppe la prise", "fin de la prise"]) {
        return Some(say(host.app_action(&json!({"action": "record", "on": false}))));
    }
    if has(t, &["retablis", "refais", "redo"]) && short {
        return Some(say(host.app_action(&json!({"action": "redo"}))));
    }

    // files
    if has(t, &["sauvegarde", "enregistre le projet", "save", "sauve le projet"]) {
        return Some(say(host.app_action(&json!({"action": "save"}))));
    }
    if has(t, &["pistes separees", "stems", "piste par piste"]) && has(t, &["export", "sors", "donne"]) {
        return Some(say(host.app_action(&json!({"action": "export_stems"}))));
    }
    if has(t, &["exporte", "export", "bounce", "sors le morceau", "fichier wav", "en wav"]) && !has(t, &["reference"]) {
        let fmt = if has(t, &["16"]) { "wav16" } else if has(t, &["32"]) { "wav32" } else { "wav24" };
        return Some(say(host.app_action(&json!({"action": "export", "format": fmt}))));
    }

    // appearance
    if has(t, &["theme", "mode sombre", "mode clair", "dark mode", "light mode", "opaque", "transparen", "effet verre", "fond "]) && !has(t, &["voix", "son ", "reverb"]) {
        let mut a = json!({"action": "set_appearance"});
        if has(t, &["sombre", "dark", "noir"]) {
            a["theme"] = json!("dark");
        } else if has(t, &["clair", "light", "blanc"]) {
            a["theme"] = json!("light");
        }
        if has(t, &["opaque", "pas transparent", "plus transparent"]) && !has(t, &["moins opaque"]) {
            a["preset"] = json!("opaque");
        } else if has(t, &["transparen", "verre"]) {
            a["preset"] = json!(if has(t, &["plus", "leger"]) { "clear" } else { "glass" });
        }
        if a.as_object().is_some_and(|m| m.len() > 1) {
            return Some(say(host.app_action(&a)));
        }
    }
    if has(t, &["plein ecran", "fullscreen"]) {
        return Some(say(host.app_action(&json!({"action": "window", "state": "fullscreen"}))));
    }
    None
}

pub fn help_text(p: &Project) -> String {
    let mut s = String::from(
        "Je n'ai pas compris précisément. Je peux par exemple :\n\
         • « Rends ma voix plus propre » / « Ma voix est étouffée »\n\
         • « Mets un autotune bien calibré » (ou léger, ou robotique)\n\
         • « Fais ressortir les adlibs » / « Mixe nos deux voix »\n\
         • « Fais-moi un mastering propre » / « Finalise le son »\n\
         • « Propose plusieurs variantes » / « Inspire-toi de la référence »\n\
         • « Crée un espace pour enregistrer » / « Mets les paroles à droite »",
    );
    if p.tracks.is_empty() {
        s.push_str("\nCommence par importer une instru (glisse un fichier audio dans la fenêtre).");
    }
    s.push_str("\nConnecte un modèle (ChatGPT, Claude ou local) dans les réglages de l'assistant pour des demandes plus libres.");
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn proj() -> Project {
        let mut p = Project::new("x");
        p.tracks.push(Track::new("Instru", TrackKind::Instrumental));
        p.tracks.push(Track::new("Voix Sam", TrackKind::Vocal));
        p.tracks.push(Track::new("Voix Léa", TrackKind::Vocal));
        p.tracks.push(Track::new("Adlibs", TrackKind::Adlib));
        p
    }

    fn tools_of(p: &Project, q: &str) -> Vec<&'static str> {
        plan(p, q, None).iter().map(|s| s.tool).collect()
    }

    #[test]
    fn understands_the_vision_examples() {
        let p = proj();
        assert_eq!(tools_of(&p, "Rends ma voix plus propre."), ["build_vocal_chain"]);
        assert_eq!(tools_of(&p, "Mets-moi un autotune bien calibré."), ["set_effect"]);
        assert_eq!(tools_of(&p, "Ma voix est trop étouffée, corrige ça."), ["set_effect"]);
        assert_eq!(tools_of(&p, "Fais ressortir les adlibs.")[0], "set_track");
        assert_eq!(*tools_of(&p, "Mixe nos deux voix pour qu'elles se complètent bien.").last().unwrap(), "balance_mix");
        assert_eq!(tools_of(&p, "Fais-moi un mastering propre."), ["master"]);
        assert_eq!(tools_of(&p, "Essaie un rendu plus mélodique."), ["build_vocal_chain"]);
        assert_eq!(tools_of(&p, "Mets les paroles à droite et agrandis la timeline."), ["set_layout", "set_layout"]);
        assert_eq!(tools_of(&p, "Crée-moi un espace de travail optimisé pour enregistrer des voix."), ["set_layout"]);
        assert_eq!(tools_of(&p, "Ajoute les réglages d'autotune directement sous ma piste.")[0], "set_layout");
        assert_eq!(tools_of(&p, "Simplifie mon interface, je veux seulement les outils que j'utilise."), ["set_layout"]);
        assert_eq!(tools_of(&p, "annule"), ["undo_last_ai"]);
        assert_eq!(tools_of(&p, "finalise le son"), ["finish_song"]);
    }

    #[test]
    fn targets_by_name() {
        let p = proj();
        let steps = plan(&p, "mets plus de reverb sur voix léa", None);
        assert_eq!(steps.len(), 1);
        let id = p.tracks[2].id.to_string();
        assert_eq!(steps[0].args["track"], json!(id));
    }
}
