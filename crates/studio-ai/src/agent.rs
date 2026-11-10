//! The agent loop over language models: the model reads the studio through
//! tools and acts with them. Providers: OpenAI (and any OpenAI-compatible
//! server: Ollama, LM Studio, llama.cpp, OpenRouter...), Anthropic, and
//! bridges to the official command-line tools that carry the user's own
//! subscription (Codex CLI with "Sign in with ChatGPT", Claude Code).
//! Without a model, or when one fails, the local engineer answers.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use studio_core::model::Id;

use crate::host::StudioHost;
use crate::tools::{self, definitions, project_json};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    /// The built-in rule-based engineer (offline).
    #[default]
    Local,
    /// OpenAI API with a key.
    OpenAi,
    /// Anthropic API with a key.
    Anthropic,
    /// Any OpenAI-compatible server (Ollama, LM Studio, llama.cpp...).
    OpenAiCompatible,
    /// ChatGPT subscription through the official Codex CLI
    /// (`codex login` = Sign in with ChatGPT).
    CodexCli,
    /// Claude subscription through Claude Code (`claude -p`).
    ClaudeCli,
}

impl ProviderKind {
    pub const ALL: [ProviderKind; 6] = [
        ProviderKind::Local,
        ProviderKind::CodexCli,
        ProviderKind::ClaudeCli,
        ProviderKind::OpenAi,
        ProviderKind::Anthropic,
        ProviderKind::OpenAiCompatible,
    ];
    pub fn label(self) -> &'static str {
        match self {
            ProviderKind::Local => "Ingénieur local (hors ligne)",
            ProviderKind::OpenAi => "OpenAI (clé API)",
            ProviderKind::Anthropic => "Claude (clé API Anthropic)",
            ProviderKind::OpenAiCompatible => "Modèle local (Ollama, LM Studio…)",
            ProviderKind::CodexCli => "ChatGPT (Se connecter avec ChatGPT, via Codex)",
            ProviderKind::ClaudeCli => "Claude (abonnement, via Claude Code)",
        }
    }
    pub fn default_model(self) -> &'static str {
        match self {
            ProviderKind::OpenAi => "gpt-5",
            ProviderKind::Anthropic => "claude-opus-5-5",
            ProviderKind::OpenAiCompatible => "qwen3",
            _ => "",
        }
    }
    pub fn default_base_url(self) -> &'static str {
        match self {
            ProviderKind::OpenAi => "https://api.openai.com/v1",
            ProviderKind::Anthropic => "https://api.anthropic.com",
            ProviderKind::OpenAiCompatible => "http://localhost:11434/v1",
            _ => "",
        }
    }
    pub fn needs_key(self) -> bool {
        matches!(self, ProviderKind::OpenAi | ProviderKind::Anthropic)
    }
    /// The command that signs the user in, for CLI bridges.
    pub fn login_command(self) -> Option<&'static [&'static str]> {
        match self {
            ProviderKind::CodexCli => Some(&["codex", "login"]),
            ProviderKind::ClaudeCli => Some(&["claude", "setup-token"]),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, Default, PartialEq)]
pub struct ProviderConfig {
    pub kind: ProviderKind,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub api_key: String,
    #[serde(default)]
    pub base_url: String,
    /// CLI bridges: the command, `{prompt}` replaced by the prompt (else
    /// the prompt goes to stdin).
    #[serde(default)]
    pub command: String,
    /// Propose improvements after each recording.
    #[serde(default = "yes")]
    pub proactive: bool,
}

fn yes() -> bool {
    true
}

impl ProviderConfig {
    pub fn model(&self) -> String {
        if self.model.trim().is_empty() {
            self.kind.default_model().to_string()
        } else {
            self.model.clone()
        }
    }
    pub fn base_url(&self) -> String {
        let b = if self.base_url.trim().is_empty() { self.kind.default_base_url() } else { self.base_url.trim() };
        b.trim_end_matches('/').to_string()
    }
    pub fn command(&self) -> String {
        if !self.command.trim().is_empty() {
            return self.command.clone();
        }
        match self.kind {
            ProviderKind::CodexCli => "codex exec --skip-git-repo-check {prompt}".into(),
            ProviderKind::ClaudeCli => "claude -p {prompt}".into(),
            _ => String::new(),
        }
    }
}

/// A message of the visible conversation.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ChatMessage {
    pub role: String,
    pub text: String,
}

#[derive(Clone, Debug, Default)]
pub struct TurnResult {
    pub reply: String,
    /// Transactions made (id, label): the UI shows an undo for each.
    pub committed: Vec<(Id, String)>,
    /// Tool calls made, for the "what I did" details.
    pub log: Vec<String>,
    /// Set when the provider failed and the local engineer answered.
    pub fallback_reason: Option<String>,
}

pub const SYSTEM_PROMPT: &str = "Tu es l'ingénieur du son d'Awra Studio, un studio musical pensé pour des artistes \
qui ne connaissent pas forcément le mixage. Tu AGIS sur le studio avec tes outils plutôt que d'expliquer comment faire.\n\
Principes :\n\
- Écoute avant de décider : analyse les pistes (analyze_track, analyze_mix, analyze_reference) puis règle.\n\
- Préfère les outils de haut niveau (build_vocal_chain, balance_mix, master, match_reference, finish_song, create_variants) \
puis affine avec set_effect / set_track.\n\
- Garde l'autotune calé sur la tonalité du projet ; si elle est inconnue, analyse l'instru (elle renvoie key et tempo_bpm) \
et règle-la avec set_song_info.\n\
- Toute modification est annulable : n'hésite pas à essayer, propose des variantes quand le goût entre en jeu.\n\
- L'interface est personnalisable : set_layout déplace, affiche, masque ou agrandit les panneaux.\n\
- Tu as un accès INTÉGRAL au logiciel : get_app_state lit tout (micro et sortie utilisés et disponibles, niveau du micro, \
latence du retour, lecture, réglages, apparence, session, historique, fichier du projet…) ; app_action fait tout ce que \
l'utilisateur ferait à la main (lecture, enregistrement, choix du micro, réglages, apparence, enregistrer, exporter, importer, \
ouvrir, sessions, annuler…) ; get_project_full et apply_ops lisent et modifient n'importe quel détail du projet (clips, fondus, \
paroles, pistes, effets) en une transaction annulable. Quand on te pose une question sur le studio, lis l'état avant de \
répondre ; quand on te demande quelque chose, fais-le plutôt que d'expliquer où cliquer.\n\
- Réponds dans la langue de l'utilisateur, brièvement, avec des mots simples : ce que tu as entendu, ce que tu as fait, \
et éventuellement une suggestion. Pas de jargon inutile.";

fn http_err(e: ureq::Error) -> String {
    match e {
        ureq::Error::Status(code, resp) => {
            let body = resp.into_string().unwrap_or_default();
            let msg = serde_json::from_str::<Value>(&body)
                .ok()
                .and_then(|v| v.pointer("/error/message").and_then(Value::as_str).map(String::from))
                .unwrap_or(body);
            format!("HTTP {code} : {}", msg.chars().take(300).collect::<String>())
        }
        ureq::Error::Transport(t) => format!("connexion impossible : {t}"),
    }
}

fn context_note(host: &mut dyn StudioHost) -> String {
    let mut n = format!("État actuel du projet (JSON) : {}", project_json(&host.project()));
    // the essentials of the application; get_app_state gives the rest
    if let Ok(a) = host.app_state() {
        let brief = json!({"transport": a.get("transport"), "audio": a.get("audio").map(|x| json!({
            "input": x.get("input"), "output": x.get("output"), "monitor_latency_ms": x.get("monitor_latency_ms")})),
            "selection": a.get("selection"), "project_file": a.get("project_file")});
        n.push_str(&format!("\nLogiciel (extrait, get_app_state pour tout) : {brief}"));
    }
    n
}

const MAX_STEPS: usize = 24;

/// One turn of the conversation: the model may call tools several times.
pub fn run_turn(
    cfg: &ProviderConfig,
    history: &[ChatMessage],
    request: &str,
    host: &mut dyn StudioHost,
    selected: Option<Id>,
    progress: &mut dyn FnMut(&str),
) -> TurnResult {
    let r = match cfg.kind {
        ProviderKind::Local => Err(String::new()),
        ProviderKind::Anthropic => anthropic_turn(cfg, history, request, host, progress),
        ProviderKind::OpenAi | ProviderKind::OpenAiCompatible => openai_turn(cfg, history, request, host, progress),
        ProviderKind::CodexCli | ProviderKind::ClaudeCli => cli_turn(cfg, history, request, host, progress),
    };
    match r {
        Ok(t) => t,
        Err(reason) => {
            let (reply, committed) = crate::local::run_with_progress(host, request, selected, progress);
            TurnResult {
                reply,
                committed,
                log: Vec::new(),
                fallback_reason: if reason.is_empty() { None } else { Some(reason) },
            }
        }
    }
}

fn exec_tool(name: &str, args: &Value, host: &mut dyn StudioHost, out: &mut TurnResult, progress: &mut dyn FnMut(&str)) -> (String, bool) {
    progress(&tool_progress(name));
    let o = tools::execute(name, args, host);
    out.log.push(format!("{name} {}", args));
    out.committed.extend(o.committed.iter().cloned());
    (o.output, o.error)
}

pub fn tool_progress(name: &str) -> String {
    match name {
        "analyze_track" | "analyze_mix" | "analyze_reference" => "J'écoute…",
        "build_vocal_chain" => "Je construis la chaîne de la voix…",
        "balance_mix" => "J'équilibre le mix…",
        "master" => "Je calibre le mastering…",
        "match_reference" => "Je compare avec la référence…",
        "finish_song" => "Je finalise le morceau…",
        "create_variants" => "Je prépare des variantes…",
        "set_layout" => "J'arrange l'interface…",
        _ => "Je règle…",
    }
    .to_string()
}

fn anthropic_turn(
    cfg: &ProviderConfig,
    history: &[ChatMessage],
    request: &str,
    host: &mut dyn StudioHost,
    progress: &mut dyn FnMut(&str),
) -> Result<TurnResult, String> {
    if cfg.api_key.trim().is_empty() {
        return Err("aucune clé API Anthropic".into());
    }
    let tools: Vec<Value> = definitions()
        .into_iter()
        .map(|t| json!({"name": t.name, "description": t.description, "input_schema": t.parameters}))
        .collect();
    let mut messages: Vec<Value> = history
        .iter()
        .filter(|m| !m.text.is_empty())
        .map(|m| json!({"role": if m.role == "user" { "user" } else { "assistant" }, "content": m.text}))
        .collect();
    // the API wants alternating roles starting with the user
    while messages.first().is_some_and(|m| m["role"] != "user") {
        messages.remove(0);
    }
    messages.push(json!({"role": "user", "content": format!("{}\n\n{}", context_note(host), request)}));
    let mut out = TurnResult::default();
    let url = format!("{}/v1/messages", cfg.base_url());
    for _ in 0..MAX_STEPS {
        let body = json!({
            "model": cfg.model(),
            "max_tokens": 16000,
            "system": SYSTEM_PROMPT,
            "tools": tools,
            "messages": messages,
            "fallbacks": "default",
        });
        let resp: Value = ureq::post(&url)
            .timeout(Duration::from_secs(300))
            .set("x-api-key", cfg.api_key.trim())
            .set("anthropic-version", "2026-10-21")
            .set("anthropic-beta", "server-side-fallback-2026-07-01")
            .set("content-type", "application/json")
            .send_json(body)
            .map_err(http_err)?
            .into_json()
            .map_err(|e| e.to_string())?;
        let stop = resp.get("stop_reason").and_then(Value::as_str).unwrap_or("");
        let content = resp.get("content").cloned().unwrap_or(json!([]));
        if stop == "refusal" {
            return Err("le modèle a refusé la demande".into());
        }
        let mut text = String::new();
        let mut results = Vec::new();
        for block in content.as_array().into_iter().flatten() {
            match block.get("type").and_then(Value::as_str) {
                Some("text") => {
                    text.push_str(block.get("text").and_then(Value::as_str).unwrap_or(""));
                }
                Some("tool_use") => {
                    let name = block.get("name").and_then(Value::as_str).unwrap_or("");
                    let id = block.get("id").and_then(Value::as_str).unwrap_or("");
                    let input = block.get("input").cloned().unwrap_or(json!({}));
                    let (output, is_error) = exec_tool(name, &input, host, &mut out, progress);
                    results.push(json!({"type": "tool_result", "tool_use_id": id, "content": output, "is_error": is_error}));
                }
                _ => {}
            }
        }
        // the assistant turn goes back unchanged (thinking blocks included)
        messages.push(json!({"role": "assistant", "content": content}));
        if results.is_empty() || stop != "tool_use" {
            out.reply = text.trim().to_string();
            return Ok(out);
        }
        messages.push(json!({"role": "user", "content": results}));
    }
    out.reply = "J'ai atteint la limite d'étapes pour cette demande.".into();
    Ok(out)
}

fn openai_turn(
    cfg: &ProviderConfig,
    history: &[ChatMessage],
    request: &str,
    host: &mut dyn StudioHost,
    progress: &mut dyn FnMut(&str),
) -> Result<TurnResult, String> {
    if cfg.kind == ProviderKind::OpenAi && cfg.api_key.trim().is_empty() {
        return Err("aucune clé API OpenAI".into());
    }
    let tools: Vec<Value> = definitions()
        .into_iter()
        .map(|t| json!({"type": "function", "function": {"name": t.name, "description": t.description, "parameters": t.parameters}}))
        .collect();
    let mut messages = vec![json!({"role": "system", "content": SYSTEM_PROMPT})];
    for m in history.iter().filter(|m| !m.text.is_empty()) {
        messages.push(json!({"role": if m.role == "user" { "user" } else { "assistant" }, "content": m.text}));
    }
    messages.push(json!({"role": "user", "content": format!("{}\n\n{}", context_note(host), request)}));
    let mut out = TurnResult::default();
    let url = format!("{}/chat/completions", cfg.base_url());
    for _ in 0..MAX_STEPS {
        let body = json!({"model": cfg.model(), "messages": messages, "tools": tools});
        let mut req = ureq::post(&url).timeout(Duration::from_secs(300)).set("content-type", "application/json");
        if !cfg.api_key.trim().is_empty() {
            req = req.set("authorization", &format!("Bearer {}", cfg.api_key.trim()));
        }
        let resp: Value = req.send_json(body).map_err(http_err)?.into_json().map_err(|e| e.to_string())?;
        let msg = resp.pointer("/choices/0/message").cloned().ok_or("réponse vide")?;
        let calls = msg.get("tool_calls").and_then(Value::as_array).cloned().unwrap_or_default();
        messages.push(msg.clone());
        if calls.is_empty() {
            out.reply = msg.get("content").and_then(Value::as_str).unwrap_or("").trim().to_string();
            return Ok(out);
        }
        for c in calls {
            let id = c.get("id").and_then(Value::as_str).unwrap_or("");
            let name = c.pointer("/function/name").and_then(Value::as_str).unwrap_or("");
            let args_s = c.pointer("/function/arguments").and_then(Value::as_str).unwrap_or("{}");
            let args: Value = serde_json::from_str(args_s).unwrap_or(json!({}));
            let (output, _) = exec_tool(name, &args, host, &mut out, progress);
            messages.push(json!({"role": "tool", "tool_call_id": id, "content": output}));
        }
    }
    out.reply = "J'ai atteint la limite d'étapes pour cette demande.".into();
    Ok(out)
}

/// The first JSON object in a text (models wrap it in prose or fences).
pub fn extract_json(s: &str) -> Option<Value> {
    let bytes = s.as_bytes();
    let mut start = 0;
    while let Some(off) = s[start..].find('{') {
        let i = start + off;
        let mut depth = 0i32;
        let mut in_str = false;
        let mut esc = false;
        for (j, &c) in bytes.iter().enumerate().skip(i) {
            if in_str {
                if esc {
                    esc = false;
                } else if c == b'\\' {
                    esc = true;
                } else if c == b'"' {
                    in_str = false;
                }
                continue;
            }
            match c {
                b'"' => in_str = true,
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        if let Ok(v) = serde_json::from_str::<Value>(&s[i..=j]) {
                            return Some(v);
                        }
                        break;
                    }
                }
                _ => {}
            }
        }
        start = i + 1;
    }
    None
}

fn cli_prompt(history: &[ChatMessage], request: &str, host: &mut dyn StudioHost, results: &[String]) -> String {
    let mut p = String::new();
    p.push_str(SYSTEM_PROMPT);
    p.push_str("\n\nTu pilotes le studio par un protocole JSON. Outils disponibles (nom : description ; paramètres JSON Schema) :\n");
    for t in definitions() {
        p.push_str(&format!("- {} : {} ; {}\n", t.name, t.description, t.parameters));
    }
    p.push_str("\nRéponds UNIQUEMENT avec un objet JSON de la forme {\"say\": \"message pour l'utilisateur\", \"calls\": [{\"tool\": \"nom\", \"args\": {...}}], \"done\": true}. \
Mets done à false si tu as besoin de voir le résultat des appels avant de conclure (par exemple après une analyse). N'exécute aucune commande, ne modifie aucun fichier : seuls ces outils agissent.\n\n");
    p.push_str(&context_note(host));
    p.push_str("\n\nConversation :\n");
    for m in history.iter().rev().take(10).collect::<Vec<_>>().into_iter().rev() {
        p.push_str(&format!("{} : {}\n", if m.role == "user" { "Utilisateur" } else { "Ingénieur" }, m.text));
    }
    p.push_str(&format!("Utilisateur : {request}\n"));
    for (i, r) in results.iter().enumerate() {
        p.push_str(&format!("\nRésultats des appels (étape {}) : {r}\n", i + 1));
    }
    p
}

fn run_cli(command: &str, prompt: &str) -> Result<String, String> {
    use std::io::Write;
    let parts = shell_words(command);
    let (prog, rest) = parts.split_first().ok_or("commande vide")?;
    let use_arg = rest.iter().any(|a| a.contains("{prompt}"));
    let args: Vec<String> = rest.iter().map(|a| a.replace("{prompt}", prompt)).collect();
    let mut cmd = std::process::Command::new(prog);
    cmd.args(&args).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped());
    if !use_arg {
        cmd.stdin(std::process::Stdio::piped());
    }
    let mut child = cmd.spawn().map_err(|e| format!("« {prog} » introuvable ({e}). Installe-le puis connecte-toi."))?;
    if !use_arg {
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(prompt.as_bytes());
        }
    }
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(format!("{prog} a échoué : {}", err.lines().last().unwrap_or("")));
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

/// Splits a command line (double quotes group words).
pub fn shell_words(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut q = false;
    for c in s.chars() {
        match c {
            '"' => q = !q,
            c if c.is_whitespace() && !q => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

fn cli_turn(
    cfg: &ProviderConfig,
    history: &[ChatMessage],
    request: &str,
    host: &mut dyn StudioHost,
    progress: &mut dyn FnMut(&str),
) -> Result<TurnResult, String> {
    let mut out = TurnResult::default();
    let mut results: Vec<String> = Vec::new();
    let mut says = Vec::new();
    for _ in 0..4 {
        progress("Je réfléchis…");
        let text = run_cli(&cfg.command(), &cli_prompt(history, request, host, &results))?;
        let v = extract_json(&text).ok_or_else(|| "réponse illisible du modèle".to_string())?;
        if let Some(s) = v.get("say").and_then(Value::as_str) {
            if !s.trim().is_empty() {
                says.push(s.trim().to_string());
            }
        }
        let calls = v.get("calls").and_then(Value::as_array).cloned().unwrap_or_default();
        let mut round = Vec::new();
        for c in &calls {
            let name = c.get("tool").and_then(Value::as_str).unwrap_or("");
            let args = c.get("args").cloned().unwrap_or(json!({}));
            let (o, _) = exec_tool(name, &args, host, &mut out, progress);
            round.push(json!({"tool": name, "result": o}));
        }
        let done = v.get("done").and_then(Value::as_bool).unwrap_or(true);
        if done || calls.is_empty() {
            break;
        }
        results.push(Value::Array(round).to_string());
    }
    out.reply = says.last().cloned().unwrap_or_else(|| "C'est fait.".into());
    Ok(out)
}

/// What the agent proposes after a recording (proactive suggestions).
pub fn after_recording_note(track_name: &str, a: &studio_analysis::Analysis) -> String {
    let d = studio_analysis::describe(a);
    if d.is_empty() {
        format!("Belle prise sur « {track_name} ». Je peux lui donner une chaîne de voix (propre, mélodique…) quand tu veux.")
    } else {
        format!(
            "J'ai écouté la prise sur « {track_name} » : {}. Tu veux que je la nettoie (« rends ma voix plus propre ») ?",
            d.join(", ")
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_extraction() {
        let v = extract_json("Voici : ```json\n{\"say\": \"ok {x}\", \"calls\": []}\n``` fin").unwrap();
        assert_eq!(v["say"], "ok {x}");
        assert!(extract_json("rien").is_none());
    }

    #[test]
    fn words() {
        assert_eq!(shell_words("claude -p {prompt}"), ["claude", "-p", "{prompt}"]);
        assert_eq!(shell_words("a \"b c\" d"), ["a", "b c", "d"]);
    }
}
