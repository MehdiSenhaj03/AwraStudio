//! The agent loops against a local mock of the Anthropic and OpenAI APIs:
//! tool calls are executed, results go back in the right shape, the
//! assistant's content is replayed unchanged.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};
use studio_ai::host::{LayoutCommand, PluginDesc, StudioHost};
use studio_ai::{ProviderConfig, ProviderKind};
use studio_analysis::Analysis;
use studio_core::model::*;
use studio_core::ops::Transaction;
use studio_core::Document;

struct Host {
    doc: Document,
}

impl StudioHost for Host {
    fn project(&self) -> Project {
        self.doc.project.clone()
    }
    fn analyze_track(&mut self, _t: Id, _p: bool) -> Result<Analysis, String> {
        Ok(Analysis::default())
    }
    fn analyze_mix(&mut self) -> Result<Analysis, String> {
        Ok(Analysis::default())
    }
    fn analyze_reference(&mut self, _r: Id) -> Result<Analysis, String> {
        Err("none".into())
    }
    fn measure_lufs(&mut self, _p: &Project) -> Result<f64, String> {
        Ok(-14.0)
    }
    fn commit(&mut self, tx: Transaction) -> Result<Id, String> {
        let id = tx.id;
        self.doc.commit(tx).map_err(|e| e.to_string())?;
        Ok(id)
    }
    fn layout(&mut self, _c: LayoutCommand) -> Result<String, String> {
        Ok("ok".into())
    }
    fn plugins(&self) -> Vec<PluginDesc> {
        Vec::new()
    }
    fn undo_ai(&mut self) -> Result<String, String> {
        Err("none".into())
    }
}

fn host() -> Host {
    let mut p = Project::new("t");
    p.tracks.push(Track::new("Voix", TrackKind::Vocal));
    Host { doc: Document::new(p) }
}

type Seen = Arc<Mutex<Vec<(String, Value)>>>;

/// Serves canned JSON responses in order; records (headers, body).
fn mock(responses: Vec<Value>) -> (String, Seen) {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = format!("http://{}", l.local_addr().unwrap());
    let seen: Seen = Arc::new(Mutex::new(Vec::new()));
    let s2 = seen.clone();
    std::thread::spawn(move || {
        let mut queue = responses.into_iter();
        'conns: for stream in l.incoming() {
            let Ok(stream) = stream else { break };
            let mut w = stream.try_clone().unwrap();
            let mut r = BufReader::new(stream);
            // several requests may share a kept-alive connection
            loop {
                let mut headers = String::new();
                let mut len = 0usize;
                loop {
                    let mut line = String::new();
                    if r.read_line(&mut line).unwrap_or(0) == 0 {
                        continue 'conns;
                    }
                    if line == "\r\n" {
                        break;
                    }
                    let low = line.to_lowercase();
                    if let Some(v) = low.strip_prefix("content-length:") {
                        len = v.trim().parse().unwrap_or(0);
                    }
                    headers.push_str(&low);
                }
                let mut body = vec![0u8; len];
                r.read_exact(&mut body).unwrap();
                s2.lock().unwrap().push((headers, serde_json::from_slice(&body).unwrap_or(Value::Null)));
                let Some(resp) = queue.next() else { return };
                let text = resp.to_string();
                let _ = write!(w, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}", text.len(), text);
                let _ = w.flush();
            }
        }
    });
    (addr, seen)
}

#[test]
fn anthropic_tool_loop() {
    let first = json!({
        "id": "msg_1", "type": "message", "role": "assistant", "stop_reason": "tool_use",
        "content": [
            {"type": "thinking", "thinking": "", "signature": "sig"},
            {"type": "text", "text": "Je baisse la voix."},
            {"type": "tool_use", "id": "toolu_1", "name": "set_track", "input": {"track": "Voix", "volume_db": -6.0}}
        ]
    });
    let second = json!({
        "id": "msg_2", "type": "message", "role": "assistant", "stop_reason": "end_turn",
        "content": [{"type": "text", "text": "C'est fait : -6 dB."}]
    });
    let (url, seen) = mock(vec![first.clone(), second]);
    let cfg = ProviderConfig { kind: ProviderKind::Anthropic, api_key: "test-key".into(), base_url: url, ..Default::default() };
    let mut h = host();
    let r = studio_ai::run_turn(&cfg, &[], "baisse ma voix", &mut h, None, &mut |_| {});
    assert_eq!(r.fallback_reason, None);
    assert_eq!(r.reply, "C'est fait : -6 dB.");
    assert_eq!(h.doc.project.tracks[0].volume_db, -6.0);
    assert_eq!(r.committed.len(), 1);
    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 2);
    let (headers, body) = &seen[0];
    assert!(headers.contains("x-api-key: test-key"));
    assert!(headers.contains("anthropic-version: 2026-10-21"));
    assert_eq!(body["model"], "claude-opus-5-5");
    assert!(body["tools"].as_array().unwrap().iter().any(|t| t["name"] == "build_vocal_chain" && t["input_schema"]["type"] == "object"));
    assert!(body.get("temperature").is_none());
    // the second request replays the assistant turn unchanged, then the result
    let msgs = seen[1].1["messages"].as_array().unwrap().clone();
    let n = msgs.len();
    assert_eq!(msgs[n - 2]["content"], first["content"]);
    assert_eq!(msgs[n - 1]["content"][0]["type"], "tool_result");
    assert_eq!(msgs[n - 1]["content"][0]["tool_use_id"], "toolu_1");
}

#[test]
fn openai_compatible_tool_loop() {
    let first = json!({"choices": [{"message": {"role": "assistant", "content": null, "tool_calls": [
        {"id": "call_1", "type": "function", "function": {"name": "set_track", "arguments": "{\"track\":\"Voix\",\"pan\":0.5}"}}
    ]}, "finish_reason": "tool_calls"}]});
    let second = json!({"choices": [{"message": {"role": "assistant", "content": "Voix à droite."}, "finish_reason": "stop"}]});
    let (url, seen) = mock(vec![first, second]);
    let cfg = ProviderConfig { kind: ProviderKind::OpenAiCompatible, base_url: url, model: "local-model".into(), ..Default::default() };
    let mut h = host();
    let r = studio_ai::run_turn(&cfg, &[], "mets ma voix à droite", &mut h, None, &mut |_| {});
    assert_eq!(r.fallback_reason, None);
    assert_eq!(r.reply, "Voix à droite.");
    assert_eq!(h.doc.project.tracks[0].pan, 0.5);
    let seen = seen.lock().unwrap();
    assert_eq!(seen[0].1["model"], "local-model");
    assert_eq!(seen[0].1["tools"][0]["type"], "function");
    let msgs = seen[1].1["messages"].as_array().unwrap();
    let last = msgs.last().unwrap();
    assert_eq!(last["role"], "tool");
    assert_eq!(last["tool_call_id"], "call_1");
}

#[test]
fn failing_provider_falls_back_to_the_local_engineer() {
    let cfg = ProviderConfig { kind: ProviderKind::OpenAiCompatible, base_url: "http://127.0.0.1:9".into(), ..Default::default() };
    let mut h = host();
    let r = studio_ai::run_turn(&cfg, &[], "fais ressortir la voix", &mut h, None, &mut |_| {});
    assert!(r.fallback_reason.is_some());
    assert_eq!(h.doc.project.tracks[0].volume_db, 3.0);
}

#[test]
fn cli_bridge_protocol() {
    // a fake CLI: prints a JSON action whatever the prompt
    let dir = std::env::temp_dir().join(format!("awra-cli-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let script = dir.join("fake-codex.sh");
    std::fs::write(
        &script,
        "#!/bin/sh\necho 'Voici :'\necho '{\"say\": \"Réverbe ajoutée.\", \"calls\": [{\"tool\": \"add_effect\", \"args\": {\"track\": \"Voix\", \"effect\": \"reverb\"}}], \"done\": true}'\n",
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    if cfg!(unix) {
        let cfg = ProviderConfig { kind: ProviderKind::CodexCli, command: format!("{} {{prompt}}", script.display()), ..Default::default() };
        let mut h = host();
        let r = studio_ai::run_turn(&cfg, &[], "ajoute de la reverb", &mut h, None, &mut |_| {});
        assert_eq!(r.fallback_reason, None, "{}", r.reply);
        assert_eq!(r.reply, "Réverbe ajoutée.");
        assert_eq!(h.doc.project.tracks[0].effects.len(), 1);
    }
    std::fs::remove_dir_all(dir).ok();
}
