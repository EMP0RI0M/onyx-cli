//! End-to-end proof that streamed model *reasoning* reaches the UI on its own
//! channel, kept distinct from the answer text. A fake OpenAI-compatible SSE
//! gateway emits `reasoning_content` deltas then an answer + a terminate tool
//! call; we drive the REAL agent loop (`run_turn_observed`) with a minimal
//! tools-only collection, project every `AgentEvent` through the core bridge,
//! and assert:
//!   - reasoning arrives as Thinking{delta} events (never as AgentMessage),
//!   - the answer's content deltas arrive as AgentMessage,
//!   - reasoning text never leaks into the answer channel.

use agent::model::{GatewayCfg, HttpModel};
use agent::{Message, ToolCallAgent};
use onyx_core::protocol::{ItemKind, ItemStatus};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::thread;

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if hay.len() < needle.len() {
        return None;
    }
    hay.windows(needle.len()).position(|w| w == needle)
}

fn read_request(sock: &mut TcpStream) {
    let mut buf = [0u8; 8192];
    let mut raw = Vec::new();
    loop {
        let n = sock.read(&mut buf).unwrap_or(0);
        if n == 0 {
            break;
        }
        raw.extend_from_slice(&buf[..n]);
        if let Some(pos) = find(&raw, b"\r\n\r\n") {
            let head = String::from_utf8_lossy(&raw[..pos]).to_ascii_lowercase();
            let len = head
                .find("content-length: ")
                .map(|i| {
                    let rest = &head[i + 16..];
                    let end = rest.find("\r\n").unwrap_or(rest.len());
                    rest[..end].trim().parse::<usize>().unwrap_or(0)
                })
                .unwrap_or(0);
            if raw.len() >= pos + 4 + len {
                break;
            }
        }
        if raw.len() > 1 << 20 {
            break;
        }
    }
}

fn chunk(s: &str) -> String {
    format!("{:x}\r\n{}\r\n", s.len(), s)
}

/// Serve one scripted SSE response, replayed for each incoming request.
fn spawn_sse(pieces: Vec<String>) -> u16 {
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    thread::spawn(move || {
        for stream in listener.incoming() {
            let mut sock: TcpStream = stream.unwrap();
            read_request(&mut sock);
            let chunks: String = pieces.iter().map(|p| chunk(p)).collect();
            let raw = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n{chunks}0\r\n\r\n"
            );
            let all = raw.as_bytes();
            let mut sent = 0usize;
            while sent < all.len() {
                let end = (sent + 43).min(all.len());
                if sock.write_all(&all[sent..end]).is_err() {
                    break;
                }
                sock.flush().ok();
                sent = end;
            }
            drop(sock);
        }
    });
    port
}

#[test]
fn reasoning_streams_live_and_stays_out_of_the_answer() {
    // reasoning_content deltas first (as a real R1-style endpoint sends them),
    // then the visible answer, then a terminate tool call that ends the turn.
    let pieces = vec![
        "data: {\"choices\":[{\"delta\":{\"reasoning_content\":\"Let me \"}}]}\n\n".to_string(),
        "data: {\"choices\":[{\"delta\":{\"reasoning_content\":\"check 2+2=4.\"}}]}\n\n".to_string(),
        "data: {\"choices\":[{\"delta\":{\"content\":\"The answer is four.\"}}]}\n\n".to_string(),
        "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"c0\",\"type\":\"function\",\"function\":{\"name\":\"terminate\",\"arguments\":\"{\\\"status\\\":\\\"success\\\",\\\"content\\\":\\\"The answer is four.\\\"}\"}}]}}]}\n\n".to_string(),
        "data: [DONE]\n\n".to_string(),
    ];
    let port = spawn_sse(pieces);

    let cfg = GatewayCfg {
        host: "127.0.0.1".to_string(),
        port,
        base: format!("http://127.0.0.1:{port}/v1"),
        key: String::new(),
        model: "auto".to_string(),
    };
    let mut hm = HttpModel::from_cfg(&cfg, "- terminate: done".to_string(), Vec::new());
    hm.set_endpoint_for(&format!("http://127.0.0.1:{port}/v1"), "");
    assert!(hm.endpoint.is_some(), "resolved protocol endpoint present");

    // Minimal collection: only terminate, so the run terminates cleanly.
    let mut tools = tools::ToolCollection::new();
    tools.add(tools::control::Terminate);
    let gw = tools::audit::Gateway::new(tools, tools::policy::AllowAll);
    let agent = ToolCallAgent::new(&gw);

    let mut reasoning = String::new();
    let mut answer = String::new();
    let mut thinking_started = 0usize;
    let mut history = vec![Message::text("system", "test")];
    let res = agent.run_turn_observed(&mut hm, "what is 2+2?", &mut history, &mut |ev| {
        let core = ev.to_core("t", 1, 0);
        match (core.status, core.kind) {
            (ItemStatus::Started, ItemKind::Thinking { delta: None, .. }) => {
                thinking_started += 1;
            }
            (_, ItemKind::Thinking { delta: Some(d), .. }) => reasoning.push_str(&d),
            (_, ItemKind::AgentMessage { delta: Some(d), .. }) => answer.push_str(&d),
            _ => {}
        }
    });
    assert!(res.is_ok(), "loop completes: {res:?}");

    assert_eq!(reasoning, "Let me check 2+2=4.", "reasoning forwarded verbatim");
    assert_eq!(answer, "The answer is four.", "answer content forwarded verbatim");
    assert!(thinking_started >= 1, "a step-start Thinking event fired");
    // The whole point: reasoning must never appear in the answer channel.
    assert!(!answer.contains("Let me "), "reasoning did not leak into the answer");
    assert!(!reasoning.contains("The answer"), "answer did not leak into reasoning");
}
