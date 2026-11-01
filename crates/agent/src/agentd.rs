//! tektile-agentd — the agentic loop as an OpenAI-compatible HTTP server.
//!
//! It speaks the SAME `/v1/chat/completions` contract the agy-maus app already
//! uses against CLIProxyAPI, but behind each reply sits the Rust `ToolCallAgent`
//! loop: the model (HttpModel → proxy :8317) decides steps, and every tool call
//! (vecmath/NEON asm, bash, editor, python, planning, control, websearch) is
//! policy-checked + audited by the Gateway before it runs. Point one bot's
//! baseUrl at this server and the chat AI gains the Rust tools.
//!
//! Zero external crates: `std::net` + `serde_json`. Loopback HTTP, no TLS, no key.

use agent::model::{http_request, omniroute_config, HttpModel};
use agent::{ToolCallAgent};
use serde_json::{json, Value};
use std::env;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{IpAddr, TcpListener, TcpStream};
use std::process::exit;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;
use tools::audit::Gateway;
use tools::policy::{AllowAll, BashGuard};

/// The upstream slug that actually drives the agent. The client's `model` string
/// is cosmetic here (this server is an agent, not a raw model), so we resolve it
/// to a real proxy slug: honor a genuine one, but map our synthetic advertised id
/// ("tektile-agent") or an empty value to $TEK_MODEL (default gemini-3-flash).
fn resolve_model(raw: &str) -> String {
    let default = env::var("ONYX_MODEL")
        .or_else(|_| env::var("TEK_MODEL"))
        .unwrap_or_else(|_| "auto".to_string());
    let t = raw.trim();
    if t.is_empty() || t == "tektile-agent" {
        default
    } else {
        raw.to_string()
    }
}

fn catalog_for(gw: &Gateway<impl tools::policy::Policy>) -> String {
    let mut s = String::new();
    for name in gw.tools.names() {
        if let Some(t) = gw.tools.get(&name) {
            s.push_str(&format!("  - {name}: {}\n", t.description()));
        }
    }
    s
}

struct Boot {
    host: String,
    port: u16,
    key: String,
    allow_all: bool,
}

/// Run one user turn through the Rust loop; return (answer, [tool names used]).
/// `persona` is the caller's system prompt (the bot's identity) — applied on top
/// of the tool protocol so a routed bot keeps its voice while still using tools.
fn run_turn(boot: &Boot, model_slug: &str, user: &str, persona: &str) -> (String, Vec<String>) {
    let base = env::temp_dir().join("tektile-agentd-editor");
    let max_steps: usize = env::var("ONYX_MAX_STEPS")
        .or_else(|_| env::var("TEK_MAX_STEPS"))
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(8);
    let persona = persona.trim().to_string();

    if boot.allow_all {
        let gw = Gateway::new(tools::default_collection(base), AllowAll);
        let mut hm = HttpModel::new(
            boot.host.clone(), boot.port, model_slug.to_string(), boot.key.clone(), catalog_for(&gw),
        );
        if !persona.is_empty() { hm.system = persona; }
        let mut agent = ToolCallAgent::new(&gw);
        agent.max_steps = max_steps;
        match agent.run(&mut hm, user) {
            Ok(rep) => {
                let tools = tool_names(&rep);
                (clean_answer(&rep), tools)
            }
            Err(e) => (format!("[agent error: {e:?}]"), Vec::new()),
        }
    } else {
        let gw = Gateway::new(tools::default_collection(base), BashGuard::default());
        let mut hm = HttpModel::new(
            boot.host.clone(), boot.port, model_slug.to_string(), boot.key.clone(), catalog_for(&gw),
        );
        if !persona.is_empty() { hm.system = persona; }
        let mut agent = ToolCallAgent::new(&gw);
        agent.max_steps = max_steps;
        match agent.run(&mut hm, user) {
            Ok(rep) => {
                let tools = tool_names(&rep);
                (clean_answer(&rep), tools)
            }
            Err(e) => (format!("[agent error: {e:?}]"), Vec::new()),
        }
    }
}

fn tool_names(rep: &agent::RunReport) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for m in &rep.history {
        if let Some(tc) = &m.tool_call {
            if !out.iter().any(|n| n == &tc.name) && tc.name != "terminate" {
                out.push(tc.name.clone());
            }
        }
    }
    out
}

/// The final user-facing answer. If the model ended by echoing a tool-action line
/// ("→ name(...)") instead of calling `terminate` with real content, fall back to
/// the ACTUAL tool results so the reply is useful, not a trace. When several tools
/// ran (a multi-part request), compile every result instead of only the last.
fn clean_answer(rep: &agent::RunReport) -> String {
    let a = rep.answer.trim();
    let weak = a.is_empty() || a.starts_with("→ ") || a.starts_with("(calling a tool)");
    if weak {
        // Pair each tool call with the observation that follows it.
        let mut pairs: Vec<(String, String)> = Vec::new();
        let mut pending: Option<String> = None;
        for m in &rep.history {
            if let Some(tc) = &m.tool_call {
                pending = Some(tc.name.clone());
            } else if m.role == "tool" && !m.content.trim().is_empty() {
                let name = pending.take().unwrap_or_else(|| "tool".to_string());
                pairs.push((name, m.content.trim().to_string()));
            }
        }
        if pairs.len() == 1 {
            return pairs[0].1.clone();
        }
        if !pairs.is_empty() {
            return pairs
                .iter()
                .map(|(n, r)| format!("{n} → {r}"))
                .collect::<Vec<_>>()
                .join("\n");
        }
    }
    rep.answer.clone()
}

/// Flatten the OpenAI `messages` array into a single user prompt for the loop:
/// the newest user turn is the request, earlier turns are prefixed as context.
fn compose_prompt(messages: &[Value]) -> String {
    let last_user = messages
        .iter()
        .rev()
        .filter(|m| m.get("role").and_then(|r| r.as_str()) == Some("user"))
        .find_map(|m| m.get("content").and_then(|c| c.as_str()).map(|s| s.to_string()))
        .unwrap_or_default();

    let mut prior: Vec<String> = Vec::new();
    let mut seen_last = false;
    for m in messages.iter().rev() {
        let role = m.get("role").and_then(|r| r.as_str()).unwrap_or("");
        let content = m.get("content").and_then(|c| c.as_str()).unwrap_or("");
        if role == "user" && !seen_last {
            seen_last = true; // this is the current turn; skip
            continue;
        }
        if role == "assistant" && !content.is_empty() {
            prior.push(format!("ASSISTANT: {content}"));
        } else if role == "user" && !content.is_empty() {
            prior.push(format!("USER: {content}"));
        }
    }
    prior.reverse();
    if prior.is_empty() {
        last_user
    } else {
        format!("Earlier in this conversation:\n{}\n\nUser now: {}", prior.join("\n"), last_user)
    }
}

/// Best-effort: POST the completed turn to the host bridge's /persist so it lands
/// in the shared SQLite DB. Always called AFTER the response is flushed, so a
/// failure or a slow bridge never delays the user's reply.
fn persist_turn(persona: &str, prompt: &str, answer: &str, tools: &[String], model: &str) {
    let port: u16 = env::var("PERSIST_PORT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(8318);
    let body = json!({
        "persona": persona, "prompt": prompt, "answer": answer,
        "tools": tools, "model": model,
    })
    .to_string();
    let _ = http_request("POST", "127.0.0.1", port, "/persist", None, Some(&body));
}

fn send_headers(stream: &mut TcpStream, status: &str, ctype: &str, content_length: usize) {
    let resp = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {ctype}\r\nContent-Length: {content_length}\r\nAccess-Control-Allow-Origin: *\r\nConnection: close\r\n\r\n"
    );
    let _ = stream.write_all(resp.as_bytes());
}

fn send_json(stream: &mut TcpStream, status: &str, body: &Value) {
    let s = body.to_string();
    send_headers(stream, status, "application/json", s.as_bytes().len());
    let _ = stream.write_all(s.as_bytes());
}

/// Reject peers outside loopback + private/link-local ranges. agentd runs
/// bash/python with NO auth, so it must never become an open shell to a wider
/// network. Set ALLOW_PUBLIC=1 to disable (only if you front it yourself).
fn client_allowed(stream: &TcpStream) -> bool {
    if env::var("ALLOW_PUBLIC").map(|v| v == "1" || v == "true").unwrap_or(false) {
        return true;
    }
    match stream.peer_addr() {
        Ok(a) => match a.ip() {
            IpAddr::V4(v4) => v4.is_loopback() || v4.is_private() || v4.is_link_local(),
            IpAddr::V6(v6) => v6.is_loopback() || (v6.segments()[0] & 0xfe00) == 0xfc00,
        },
        Err(_) => false,
    }
}

fn handle_conn(mut stream: TcpStream, boot: &Boot) {
    stream.set_read_timeout(Some(std::time::Duration::from_secs(180))).ok();
    if !client_allowed(&stream) {
        let _ = stream.write_all(
            b"HTTP/1.1 403 Forbidden\r\nContent-Type: text/plain\r\nContent-Length: 18\r\nConnection: close\r\n\r\npeer not permitted",
        );
        return;
    }
    let mut reader = BufReader::new(stream.try_clone().unwrap());

    let mut request_line = String::new();
    if reader.read_line(&mut request_line).is_err() || request_line.is_empty() {
        return;
    }
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let path = parts.next().unwrap_or("").to_string();

    // headers
    let mut content_length = 0usize;
    loop {
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) | Err(_) => break,
            Ok(_) => {
                let t = line.trim_end_matches(['\r', '\n']);
                if t.is_empty() {
                    break;
                }
                let lower = t.to_ascii_lowercase();
                if let Some(v) = lower.strip_prefix("content-length:") {
                    content_length = v.trim().parse().unwrap_or(0);
                }
            }
        }
    }

    let mut body = vec![0u8; content_length];
    if content_length > 0 {
        let _ = reader.read_exact(&mut body);
    }

    // OPTIONS (CORS preflight)
    if method == "OPTIONS" {
        let resp = "HTTP/1.1 204 No Content\r\nAccess-Control-Allow-Origin: *\r\nAccess-Control-Allow-Methods: GET, POST, OPTIONS\r\nAccess-Control-Allow-Headers: Content-Type, Authorization\r\nConnection: close\r\n\r\n";
        let _ = stream.write_all(resp.as_bytes());
        return;
    }

    if path.starts_with("/health") {
        send_json(&mut stream, "200 OK", &json!({ "ok": true, "engine": "tektile-agentd" }));
        return;
    }

    if path.starts_with("/v1/models") {
        // advertise the REAL upstream slug the agent uses, so the app's model
        // list / testConnection stay consistent (the agent drives this slug).
        send_json(
            &mut stream,
            "200 OK",
            &json!({ "object": "list", "data": [{ "id": resolve_model(""), "object": "model" }] }),
        );
        return;
    }

    if path.starts_with("/v1/chat/completions") && method == "POST" {
        let v: Value = match serde_json::from_slice(&body) {
            Ok(v) => v,
            Err(e) => {
                send_json(&mut stream, "400 Bad Request", &json!({ "error": format!("bad json: {e}") }));
                return;
            }
        };
        let model_slug = resolve_model(v.get("model").and_then(|m| m.as_str()).unwrap_or(""));
        let stream_wanted = v.get("stream").and_then(|s| s.as_bool()).unwrap_or(false);
        let messages = v.get("messages").and_then(|m| m.as_array()).cloned().unwrap_or_default();
        // The caller's persona = first system message (the bot's identity). The
        // Rust loop applies it as its system base so routed bots keep their voice.
        let persona = messages
            .iter()
            .find(|m| m.get("role").and_then(|r| r.as_str()) == Some("system"))
            .and_then(|m| m.get("content").and_then(|c| c.as_str()))
            .unwrap_or("")
            .to_string();
        let prompt = compose_prompt(&messages);
        if prompt.trim().is_empty() {
            send_json(&mut stream, "400 Bad Request", &json!({ "error": "empty prompt" }));
            return;
        }

        if stream_wanted {
            // Flush headers + a comment immediately so the client sees a live
            // stream right away. The Rust loop can take several seconds; without
            // an early byte + keepalive the phone's XHR treats the silence as a
            // dropped connection ("network error during stream").
            let hdr = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\nAccess-Control-Allow-Origin: *\r\nConnection: close\r\n\r\n";
            let _ = stream.write_all(hdr.as_bytes());
            let _ = stream.write_all(b": ping\n\n");
            let _ = stream.flush();

            // Keepalive pings on a cloned handle while the (blocking) loop runs.
            let stop = Arc::new(AtomicBool::new(false));
            let ka_handle = match stream.try_clone() {
                Ok(mut ka) => {
                    let stop2 = stop.clone();
                    Some(thread::spawn(move || {
                        while !stop2.load(Ordering::Relaxed) {
                            thread::sleep(Duration::from_millis(2000));
                            if stop2.load(Ordering::Relaxed) {
                                break;
                            }
                            let _ = ka.write_all(b": ping\n\n");
                            let _ = ka.flush();
                        }
                    }))
                }
                Err(_) => None,
            };

            let (answer, tools_used) = run_turn(boot, &model_slug, &prompt, &persona);
            stop.store(true, Ordering::Relaxed);
            if let Some(h) = ka_handle {
                let _ = h.join();
            }

            // Surface executed Rust tools first (the app renders these as activity badges).
            for name in &tools_used {
                let ev = json!({ "choices": [{ "index": 0, "delta": { "tool_calls": [{ "index": 0, "function": { "name": name } }] } }] });
                let _ = stream.write_all(format!("data: {}\n\n", ev).as_bytes());
                let _ = stream.flush();
            }
            // Then the answer, chunked so the caret animates.
            for chunk in chunk_text(&answer, 40) {
                let ev = json!({ "choices": [{ "index": 0, "delta": { "content": chunk } }] });
                let _ = stream.write_all(format!("data: {}\n\n", ev).as_bytes());
                let _ = stream.flush();
            }
            let _ = stream.write_all(b"data: [DONE]\n\n");
            let _ = stream.flush();
            persist_turn(&persona, &prompt, &answer, &tools_used, &model_slug);
        } else {
            let (answer, tools_used) = run_turn(boot, &model_slug, &prompt, &persona);
            send_json(
                &mut stream,
                "200 OK",
                &json!({
                    "id": format!("agentd-{}", std::time::UNIX_EPOCH.elapsed().map(|d| d.as_millis()).unwrap_or(0)),
                    "object": "chat.completion",
                    "model": model_slug,
                    "choices": [{ "index": 0, "message": { "role": "assistant", "content": answer }, "finish_reason": "stop" }],
                }),
            );
            persist_turn(&persona, &prompt, &answer, &tools_used, &model_slug);
        }
        return;
    }

    send_json(&mut stream, "404 Not Found", &json!({ "error": "not found" }));
}

fn chunk_text(text: &str, max: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for ch in text.chars() {
        cur.push(ch);
        if cur.chars().count() >= max {
            out.push(std::mem::take(&mut cur));
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    if out.is_empty() {
        out.push(String::new());
    }
    out
}

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    let mut host = "127.0.0.1".to_string();
    let mut port: u16 = 8399;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--host" => { if i + 1 < args.len() { host = args[i + 1].clone(); i += 2; } else { i += 1; } }
            "--port" => { if i + 1 < args.len() { port = args[i + 1].parse().unwrap_or(8399); i += 2; } else { i += 1; } }
            "-h" | "--help" => {
                eprintln!("usage: tektile-agentd [--host 127.0.0.1] [--port 8399]   (env TEK_ALLOW_ALL=1, TEK_MAX_STEPS=N)");
                exit(0);
            }
            _ => i += 1,
        }
    }

    let cfg = omniroute_config();
    let allow_all = env::var("ONYX_ALLOW_ALL")
        .or_else(|_| env::var("TEK_ALLOW_ALL"))
        .map(|v| v == "1" || v == "true")
        .unwrap_or(false);
    println!(
        "tektile-agentd on http://{host}:{port}  → gateway {}:{} (key len {})  policy={}",
        cfg.host,
        cfg.port,
        cfg.key.chars().count(),
        if allow_all { "AllowAll" } else { "BashGuard" }
    );

    let boot = Boot { host: cfg.host, port: cfg.port, key: cfg.key, allow_all };
    let listener = match TcpListener::bind((host.as_str(), port)) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("tektile-agentd: bind {host}:{port} failed: {e}");
            exit(1);
        }
    };
    for conn in listener.incoming() {
        match conn {
            Ok(stream) => {
                let boot = Boot { host: boot.host.clone(), port: boot.port, key: boot.key.clone(), allow_all: boot.allow_all };
                thread::spawn(move || handle_conn(stream, &boot));
            }
            Err(_) => continue,
        }
    }
}
