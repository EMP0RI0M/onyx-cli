//! Offline integration test: a fake OpenAI-compatible SSE server on loopback.
//! Exercises HttpModel::next_step_streaming end to end — chunked transfer,
//! partial lines split across socket writes, content deltas, and native
//! tool_call fragments stitched by index.

use agent::model::{GatewayCfg, HttpModel};
use agent::{Message, Model, Step};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::thread;

fn read_http_request(sock: &mut TcpStream) -> String {
    let mut buf = vec![0u8; 8192];
    let mut raw = Vec::new();
    loop {
        let n = sock.read(&mut buf).unwrap();
        if n == 0 {
            break;
        }
        raw.extend_from_slice(&buf[..n]);
        if let Some(pos) = find(&raw, b"\r\n\r\n") {
            let content_len = {
                let head = String::from_utf8_lossy(&raw[..pos]).to_ascii_lowercase();
                if let Some(i) = find(head.as_bytes(), b"content-length: ") {
                    let rest = &head[i + 16..];
                    let end = rest.find("\r\n").unwrap_or(rest.len());
                    rest[..end].trim().parse().unwrap_or(0)
                } else {
                    0
                }
            };
            if raw.len() >= pos + 4 + content_len {
                break;
            }
        }
        if raw.len() > 1 << 20 {
            break; // unbounded request: give up, the test will fail loudly
        }
    }
    String::from_utf8_lossy(&raw).to_string()
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if hay.len() < needle.len() {
        return None;
    }
    hay.windows(needle.len()).position(|w| w == needle)
}

fn chunk(s: &str) -> String {
    format!("{:x}\r\n{}\r\n", s.len(), s)
}

/// Serve one scripted response (body pieces written in fixed 37-byte slices so
/// chunk framing AND mid-line splits are both exercised), then disconnect.
/// `sse` selects event-stream/chunked vs application/json/content-length.
fn spawn_fake_gateway(pieces: Vec<String>, sse: bool) -> u16 {
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    thread::spawn(move || {
        for stream in listener.incoming() {
            let mut sock: TcpStream = stream.unwrap();
            read_http_request(&mut sock);
            let body = pieces.concat();
            let raw = if sse {
                let mut chunks = String::new();
                for piece in &pieces {
                    chunks.push_str(&chunk(piece));
                }
                chunks.push_str("0\r\n\r\n");
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n{chunks}"
                )
            } else {
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
                    body.len()
                )
            };
            let all = raw.as_bytes();
            let mut sent = 0usize;
            let mut failed = false;
            while sent < all.len() {
                let end = (sent + 37).min(all.len()); // odd size: breaks frames
                if sock.write_all(&all[sent..end]).is_err() {
                    failed = true; // client parsed enough and closed early
                    break;
                }
                sock.flush().ok();
                sent = end;
            }
            if !failed {
                let mut drain = [0u8; 128];
                sock.set_read_timeout(Some(std::time::Duration::from_millis(50))).ok();
                let _ = sock.read(&mut drain);
            }
            drop(sock);
        }
    });
    port
}

fn model_for(port: u16) -> HttpModel {
    let cfg = GatewayCfg {
        host: "127.0.0.1".to_string(),
        port,
        base: format!("http://127.0.0.1:{port}/v1"),
        key: String::new(),
        model: "auto".to_string(),
    };
    HttpModel::from_cfg(&cfg, "- terminate: done".to_string(), Vec::new())
}

#[test]
fn streaming_sse_stitches_tool_calls_across_splits() {
    // Delta events: name arrives in pieces, arguments arrive in pieces, two
    // indices interleave. If stitching or partial-line carry breaks, the final
    // step will not be the exact two-call batch below.
    let pieces = vec![
        "data: {\"choices\":[{\"delta\":{\"content\":\"Let me \"}}]}\n\n".to_string(),
        "data: {\"choices\":[{\"delta\":{\"content\":\"act.\\n\"}}]}\n\n".to_string(),
        "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"c0\",\"type\":\"function\",\"function\":{\"name\":\"termi\",\"arguments\":\"\"}}]}}]}\n\n".to_string(),
        "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":1,\"id\":\"c1\",\"type\":\"function\",\"function\":{\"name\":\"vec\",\"arguments\":\"{\\\"op\\\":\"}}]}}]}\n\n".to_string(),
        "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"name\":\"nate\",\"arguments\":\"{\\\"status\\\":\\\"success\\\",\\\"content\\\":\\\"ok\\\"}\"}}]}}]}\n\n".to_string(),
        "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":1,\"function\":{\"name\":\"math\",\"arguments\":\"\\\"dot\\\",\\\"a\\\":[1.0],\\\"b\\\":[2.0]}\"}}]}}]}\n\n".to_string(),
        "data: [DONE]\n\n".to_string(),
    ];
    let port = spawn_fake_gateway(pieces, true);
    let mut hm = model_for(port);
    let history = vec![Message::text("user", "go")];

    let mut seen = String::new();
    let step = hm.next_step_streaming(&history, &[], &mut |d| seen.push_str(d), &mut |_| {});

    match step {
        Step::Calls(calls) => {
            assert_eq!(calls.len(), 2, "both stitched calls present");
            // BTreeMap order: index 0 first
            assert_eq!(calls[0].name, "terminate");
            assert_eq!(calls[0].args["status"], "success");
            assert_eq!(calls[0].args["content"], "ok");
            assert_eq!(calls[1].name, "vecmath");
            assert_eq!(calls[1].args["op"], "dot");
            assert_eq!(calls[1].args["a"][0], 1.0);
        }
        other => panic!("expected Calls, got {other:?}"),
    }
    assert!(seen.contains("Let me "), "content deltas forwarded");
}

#[test]
fn non_streaming_native_tool_calls_parse() {
    let body = "{\"choices\":[{\"message\":{\"content\":\"\",\"tool_calls\":[{\"function\":{\"name\":\"terminate\",\"arguments\":\"{\\\"status\\\":\\\"success\\\",\\\"content\\\":\\\"done\\\"}\"}}]}}]}";
    let port = spawn_fake_gateway(vec![body.to_string()], false);
    let mut hm = model_for(port);
    let history = vec![Message::text("user", "go")];
    match hm.next_step(&history, &[]) {
        Step::Calls(calls) => {
            assert_eq!(calls.len(), 1);
            assert_eq!(calls[0].name, "terminate");
            assert_eq!(calls[0].args["content"], "done");
        }
        other => panic!("expected Calls, got {other:?}"),
    }
}
