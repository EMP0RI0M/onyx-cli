//! Offline integration test for the Phase-1 provider wiring: when an `HttpModel`
//! carries a resolved `ProviderProtocol` endpoint (as it does after a `/provider`
//! switch), requests must route through the adapter's `build_body` + the shared
//! TLS/plain transport + `parse_stream_event`, NOT the legacy hardcoded OpenAI
//! branch. A fake loopback gateway lets us assert both directions without keys.

use agent::model::{GatewayCfg, HttpModel};
use agent::provider;
use agent::{Message, Step};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread;

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if hay.len() < needle.len() {
        return None;
    }
    hay.windows(needle.len()).position(|w| w == needle)
}

/// Read one HTTP request (headers + Content-Length body) from the socket.
fn read_request(sock: &mut TcpStream) -> String {
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
    String::from_utf8_lossy(&raw).to_string()
}

fn chunk(s: &str) -> String {
    format!("{:x}\r\n{}\r\n", s.len(), s)
}

/// Fake SSE gateway that records each request line so we can prove which wire
/// path was taken, then replays scripted `data:` events in odd-sized slices.
fn spawn_sse_gateway(pieces: Vec<String>, seen: Arc<Mutex<Vec<String>>>) -> u16 {
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    thread::spawn(move || {
        for stream in listener.incoming() {
            let mut sock: TcpStream = stream.unwrap();
            let req = read_request(&mut sock);
            let first = req.lines().next().unwrap_or("").to_string();
            seen.lock().unwrap().push(first);
            let chunks: String = pieces.iter().map(|p| chunk(p)).collect();
            let raw = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n{chunks}0\r\n\r\n"
            );
            let all = raw.as_bytes();
            let mut sent = 0usize;
            while sent < all.len() {
                let end = (sent + 41).min(all.len());
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
fn resolved_endpoint_routes_through_protocol_adapter() {
    // Two content deltas + one stitched tool call, then [DONE].
    let pieces = vec![
        "data: {\"choices\":[{\"delta\":{\"content\":\"Hello \"}}]}\n\n".to_string(),
        "data: {\"choices\":[{\"delta\":{\"content\":\"world\\n\"}}]}\n\n".to_string(),
        "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"c0\",\"type\":\"function\",\"function\":{\"name\":\"terminate\",\"arguments\":\"{\\\"status\\\":\\\"success\\\",\\\"content\\\":\\\"done\\\"}\"}}]}}]}\n\n".to_string(),
        "data: [DONE]\n\n".to_string(),
    ];
    let seen = Arc::new(Mutex::new(Vec::<String>::new()));
    let port = spawn_sse_gateway(pieces, seen.clone());

    let cfg = GatewayCfg {
        host: "127.0.0.1".to_string(),
        port,
        base: format!("http://127.0.0.1:{port}/v1"),
        key: String::new(),
        model: "auto".to_string(),
    };
    let mut hm = HttpModel::from_cfg(&cfg, "- terminate: done".to_string(), Vec::new());

    // Inject a resolved OpenAI-compat endpoint exactly as a /provider switch does.
    assert!(provider::find_spec("custom_openai").is_some(), "custom_openai in catalog");
    hm.set_endpoint_for(&format!("http://127.0.0.1:{port}/v1"), "");
    // The endpoint must have been resolved (protocol present).
    assert!(hm.endpoint.is_some(), "resolved endpoint should be present");

    let history = vec![Message::text("user", "go")];
    let mut streamed = String::new();
    let step = hm.next_step_streaming(&history, &[], &mut |d| streamed.push_str(d), &mut |_| {});

    match step {
        Step::Calls(calls) => {
            assert_eq!(calls.len(), 1);
            assert_eq!(calls[0].name, "terminate");
            assert_eq!(calls[0].args["content"], "done");
        }
        other => panic!("expected Calls, got {other:?}"),
    }
    assert!(streamed.contains("Hello "), "adapter deltas forwarded to UI");
    assert!(streamed.contains("world"), "second delta forwarded");

    // Prove the request went to the adapter-derived chat path, not a stale one.
    let reqs = seen.lock().unwrap();
    assert!(
        reqs.iter().any(|l| l.contains("/chat/completions")),
        "request hit the OpenAI-compat chat path: {reqs:?}"
    );
}
