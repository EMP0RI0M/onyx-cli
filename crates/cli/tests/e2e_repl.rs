//! End-to-end CLI test: spawn the `onyx` binary with a scripted fake gateway
//! and drive its REPL over stdin, asserting the file tool actually ran and the
//! audit trail printed. No network beyond loopback, no live model.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::thread;

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if hay.len() < needle.len() {
        return None;
    }
    hay.windows(needle.len()).position(|w| w == needle)
}

/// One-shot fake gateway: reply to each request in script order — first an
/// editor tool_call, then terminate. The CLI always streams (`"stream":true`),
/// so replies are SSE deltas with the tool call split across chunks (name in
/// one, arguments in the next), exactly like a real OpenAI-compatible backend.
fn spawn_scripted_gateway() -> u16 {
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    thread::spawn(move || {
        let sse: [Vec<String>; 2] = [
            vec![
                "{\"choices\":[{\"delta\":{\"role\":\"assistant\",\"tool_calls\":[{\"index\":0,\"id\":\"c1\",\"type\":\"function\",\"function\":{\"name\":\"editor\",\"arguments\":\"\"}}]}}]}".into(),
                "{\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"{\\\"command\\\":\\\"create\\\",\\\"path\\\":\\\"notes.txt\\\",\\\"file_text\\\":\\\"hello from onyx\\\"}\"}}]}}]}".into(),
                "{\"choices\":[{\"delta\":{},\"finish_reason\":\"tool_call\"}]}".into(),
            ],
            vec![
                "{\"choices\":[{\"delta\":{\"role\":\"assistant\",\"tool_calls\":[{\"index\":0,\"id\":\"c2\",\"type\":\"function\",\"function\":{\"name\":\"terminate\",\"arguments\":\"\"}}]}}]}".into(),
                "{\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"{\\\"status\\\":\\\"success\\\",\\\"content\\\":\\\"created notes.txt\\\"}\"}}]}}]}".into(),
                "{\"choices\":[{\"delta\":{},\"finish_reason\":\"tool_call\"}]}".into(),
            ],
        ];
        for (i, stream) in listener.incoming().enumerate() {
            let mut sock = stream.unwrap();
            let mut buf = vec![0u8; 8192];
            let mut raw = Vec::new();
            loop {
                match sock.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        raw.extend_from_slice(&buf[..n]);
                        if find(&raw, b"\r\n\r\n").is_some() && raw.len() > 200 {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
            let events = &sse[i % sse.len()];
            let body: String = events.iter().map(|e| format!("data: {e}\n\n")).collect::<Vec<_>>().join("") + "data: [DONE]\n\n";
            let head = format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
            let _ = sock.write_all(head.as_bytes());
            let _ = sock.write_all(body.as_bytes());
            let _ = sock.flush();
            drop(sock);
        }
    });
    port
}

fn onyx_bin() -> PathBuf {
    // Prefer cargo's explicit bin path; fall back to scanning deps for the real
    // CLI (identified by its --help banner, since another binary may share the
    // "onyx-" prefix).
    if let Ok(p) = std::env::var("CARGO_BIN_EXE_onyx") {
        return PathBuf::from(p);
    }
    let target_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/release");
    let candidate = target_dir.join("onyx");
    if candidate.exists() {
        return candidate;
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            for p in std::fs::read_dir(dir).into_iter().flatten().filter_map(|e| e.ok().map(|e| e.path())) {
                let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("").to_string();
                if !name.starts_with("onyx-") || name.ends_with(".d") {
                    continue;
                }
                let out = Command::new(&p).arg("--help").output();
                let text = out.map(|o| format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr))).unwrap_or_default();
                if text.contains("usage: onyx") {
                    return p;
                }
            }
        }
    }
    let debug_candidate = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/debug/onyx");
    if debug_candidate.exists() {
        return debug_candidate;
    }
    panic!("onyx CLI binary not found next to test binary");
}

#[test]
fn repl_end_to_end_creates_file_and_audits() {
    let workdir = std::env::temp_dir().join(format!("onyx-e2e-{}", std::process::id()));
    std::fs::remove_dir_all(&workdir).ok();
    std::fs::create_dir_all(&workdir).unwrap();
    let port = spawn_scripted_gateway();

    let out = Command::new(onyx_bin())
        .arg("--legacy") // piped stdin is not a tty; drive the line REPL, not the TUI
        .env("ONYX_BASE_URL", format!("http://127.0.0.1:{port}/v1"))
        .env("ONYX_WORKDIR", &workdir)
        .env("ONYX_MAX_TOKENS", "64")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut child = out;
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(b"create notes.txt with hello\n/quit\n")
        .unwrap();
    drop(child.stdin.take());
    let status = child.wait_with_output().unwrap();
    let stdout = String::from_utf8_lossy(&status.stdout).to_string();

    assert!(status.status.success(), "exit {:?}\nstdout: {stdout}\nstderr: {}", status.status, String::from_utf8_lossy(&status.stderr));
    assert!(stdout.contains("ANSWER: created notes.txt"), "got: {stdout}");
    assert!(stdout.contains("editor"), "trace should show the editor call: {stdout}");
    assert!(stdout.contains("audit"), "audit summary expected: {stdout}");

    let written = std::fs::read_to_string(workdir.join("notes.txt")).unwrap();
    assert_eq!(written, "hello from onyx");
    std::fs::remove_dir_all(&workdir).ok();
}
