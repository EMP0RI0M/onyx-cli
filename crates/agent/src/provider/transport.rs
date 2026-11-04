//! HTTP/1.1 transport with real TLS, no async runtime.
//!
//! The agent originally spoke only plain HTTP over a blocking `TcpStream`, which
//! is fine for a loopback gateway but cannot reach any cloud provider (OpenAI,
//! Anthropic, Google all require HTTPS). This module adds an `https` path backed
//! by rustls using the **ring** crypto provider — deliberately not the default
//! `aws-lc-rs`, which needs a CMake build that isn't available on this arm64 box.
//!
//! It keeps the exact same call shape as the old helpers (`method/host/port/path/
//! auth/body`) plus an `extra_headers` slice so protocol adapters can send
//! provider-specific headers (Anthropic's `x-api-key`, Gemini's `x-goog-api-key`).
//! `http://` still uses a raw socket, so the offline loopback path is unchanged
//! and costs nothing when TLS isn't needed.

use rustls::pki_types::ServerName;
use rustls::{ClientConfig, RootCertStore};
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::Arc;
use std::time::Duration;

/// One `(name, value)` header pair sent verbatim on the request line set.
pub type Header<'a> = (&'a str, &'a str);

/// True when a host:port should be reached over TLS. Loopback and plain-http
/// bases stay cleartext (offline-capable, card-free); everything else upgrades.
fn use_tls(host: &str, port: u16) -> bool {
    if port == 443 {
        return true;
    }
    // A non-443 port on a public host is still assumed TLS unless it's local.
    !(is_local_host(host) && port != 443)
}

fn is_local_host(host: &str) -> bool {
    host == "localhost"
        || host == "127.0.0.1"
        || host == "::1"
        || host.starts_with("127.")
        || host.starts_with("[::1]")
}

/// Build a rustls client config with the webpki root bundle and the ring
/// provider installed once (idempotent). Returns an Arc usable across calls.
fn tls_config() -> Result<Arc<ClientConfig>, String> {
    static INIT: std::sync::Once = std::sync::Once::new();
    INIT.call_once(|| {
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
    let mut roots = RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let cfg = ClientConfig::builder().with_root_certificates(roots).with_no_client_auth();
    Ok(Arc::new(cfg))
}

/// Connect + optionally wrap in TLS, returning a boxed stream so both the plain
/// and TLS paths share one read/write surface.
fn open_stream(host: &str, port: u16, tls: bool) -> Result<Box<dyn Stream>, String> {
    let addr = (host, port)
        .to_socket_addrs()
        .map_err(|e| format!("resolve {host}:{port}: {e}"))?
        .next()
        .ok_or_else(|| "no address".to_string())?;
    let tcp = TcpStream::connect_timeout(&addr, Duration::from_secs(15))
        .map_err(|e| format!("connect {host}:{port}: {e}"))?;
    tcp.set_write_timeout(Some(Duration::from_secs(30))).ok();
    tcp.set_read_timeout(Some(Duration::from_secs(220))).ok();
    if !tls {
        return Ok(Box::new(tcp));
    }
    let cfg = tls_config()?;
    let sni = ServerName::try_from(host.to_string())
        .map_err(|_| format!("invalid server name {host}"))?;
    let conn = rustls::ClientConnection::new(cfg, sni).map_err(|e| format!("tls init: {e}"))?;
    let stream = rustls::StreamOwned::new(conn, tcp);
    Ok(Box::new(stream))
}

/// Unified blocking byte stream (plain TCP or rustls-wrapped).
pub trait Stream: Read + Write + Unpin {
    fn flush_io(&mut self) -> std::io::Result<()>;
}
impl Stream for TcpStream {
    fn flush_io(&mut self) -> std::io::Result<()> {
        TcpStream::flush(self)
    }
}
impl Stream for rustls::StreamOwned<rustls::ClientConnection, TcpStream> {
    fn flush_io(&mut self) -> std::io::Result<()> {
        self.flush()
    }
}
impl Stream for Box<dyn Stream> {
    fn flush_io(&mut self) -> std::io::Result<()> {
        (**self).flush_io()
    }
}

/// Perform one complete HTTP/1.1 request and return the decoded body. Handles
/// chunked transfer encoding and surfaces non-2xx as an error string.
pub fn request(
    method: &str,
    host: &str,
    port: u16,
    path: &str,
    auth: Option<&str>,
    extra: &[Header],
    body: Option<&str>,
) -> Result<String, String> {
    let tls = use_tls(host, port);
    let mut sock = open_stream(host, port, tls)?;
    let head = build_head(method, host, port, path, auth, extra, body);
    sock.write_all(head.as_bytes()).map_err(|e| format!("write hdr: {e}"))?;
    if let Some(b) = body {
        sock.write_all(b.as_bytes()).map_err(|e| format!("write body: {e}"))?;
    }
    sock.flush_io().ok();
    let raw = read_all(&mut sock);
    parse_response(&raw)
}

/// Perform one streaming SSE request, feeding each `data:` payload to `on_data`
/// until it returns false or the stream ends. Blocking, incremental read.
pub fn sse(
    method: &str,
    host: &str,
    port: u16,
    path: &str,
    auth: Option<&str>,
    extra: &[Header],
    body: Option<&str>,
    accept: &str,
    on_data: &mut dyn FnMut(&str) -> bool,
) -> Result<(), String> {
    let tls = use_tls(host, port);
    let mut sock = open_stream(host, port, tls)?;
    let mut head = String::new();
    head.push_str(&format!(
        "{method} {path} HTTP/1.1\r\nHost: {host}:{port}\r\nAccept: {accept}\r\n"
    ));
    apply_auth_and_extra(&mut head, auth, extra);
    match body {
        Some(b) => {
            head.push_str("Content-Type: application/json\r\n");
            head.push_str(&format!("Content-Length: {}\r\n", b.as_bytes().len()));
        }
        None => head.push_str("Content-Length: 0\r\n"),
    }
    head.push_str("Connection: close\r\n\r\n");
    sock.write_all(head.as_bytes()).map_err(|e| format!("write hdr: {e}"))?;
    if let Some(b) = body {
        sock.write_all(b.as_bytes()).map_err(|e| format!("write body: {e}"))?;
    }
    sock.flush_io().ok();

    // Read incrementally, parsing SSE lines as they arrive so deltas stream out.
    let mut buf: Vec<u8> = Vec::new();
    let mut tmp = [0u8; 4096];
    let mut past_headers = false;
    loop {
        match sock.read(&mut tmp) {
            Ok(0) => break,
            Ok(n) => {
                buf.extend_from_slice(&tmp[..n]);
                if !past_headers {
                    if let Some(idx) = find_subslice(&buf, b"\r\n\r\n") {
                        buf.drain(..idx + 4);
                        past_headers = true;
                    } else {
                        continue;
                    }
                }
                // Drain complete lines from the front of buf.
                while let Some(idx) = find_subslice(&buf, b"\n") {
                    let line_raw = buf[..=idx].to_vec();
                    buf.drain(..=idx);
                    let line = String::from_utf8_lossy(&line_raw);
                    let line = line.trim_end_matches(['\r', '\n']);
                    if let Some(data) = line.strip_prefix("data:") {
                        let data = data.trim_start();
                        if data == "[DONE]" {
                            return Ok(());
                        }
                        if !data.is_empty() && !on_data(data) {
                            return Ok(());
                        }
                    }
                }
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => break, // timeout / EOF-ish: stop pumping
        }
    }
    Ok(())
}

fn build_head(
    method: &str,
    host: &str,
    port: u16,
    path: &str,
    auth: Option<&str>,
    extra: &[Header],
    body: Option<&str>,
) -> String {
    let mut req = String::new();
    req.push_str(&format!(
        "{method} {path} HTTP/1.1\r\nHost: {host}:{port}\r\nAccept: application/json\r\n"
    ));
    apply_auth_and_extra(&mut req, auth, extra);
    match body {
        Some(b) => {
            req.push_str("Content-Type: application/json\r\n");
            req.push_str(&format!("Content-Length: {}\r\n", b.as_bytes().len()));
        }
        None => req.push_str("Content-Length: 0\r\n"),
    }
    req.push_str("Connection: close\r\n\r\n");
    req
}

fn apply_auth_and_extra(req: &mut String, auth: Option<&str>, extra: &[Header]) {
    // An explicit Authorization among extras suppresses the bearer helper so a
    // caller can fully control auth (e.g. Copilot's pseudo-token form).
    let has_auth_header = extra.iter().any(|(k, _)| k.eq_ignore_ascii_case("authorization"));
    if let Some(a) = auth {
        if !has_auth_header {
            req.push_str(&format!("Authorization: Bearer {a}\r\n"));
        }
    }
    for (k, v) in extra {
        req.push_str(&format!("{k}: {v}\r\n"));
    }
}

fn read_all(sock: &mut dyn Stream) -> Vec<u8> {
    let mut raw = Vec::new();
    let mut tmp = [0u8; 8192];
    loop {
        match sock.read(&mut tmp) {
            Ok(0) => break,
            Ok(n) => raw.extend_from_slice(&tmp[..n]),
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => break, // timeout: parse what arrived
        }
    }
    raw
}

pub(crate) fn parse_response(raw: &[u8]) -> Result<String, String> {
    let sep = b"\r\n\r\n";
    let idx = find_subslice(raw, sep)
        .ok_or_else(|| "malformed response (no header/body split)".to_string())?;
    let head = String::from_utf8_lossy(&raw[..idx]).to_string();
    let body = &raw[idx + sep.len()..];
    let status_line = head.lines().next().unwrap_or("");
    let code: u16 = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|c| c.parse().ok())
        .unwrap_or(0);
    let lower = head.to_ascii_lowercase();
    let decoded = if lower.contains("transfer-encoding: chunked") {
        decode_chunked(body)
    } else {
        body.to_vec()
    };
    let decoded = String::from_utf8_lossy(&decoded).into_owned();
    if !(200..300).contains(&code) {
        return Err(format!(
            "http {code}: {}",
            decoded.chars().take(400).collect::<String>()
        ));
    }
    Ok(decoded)
}

pub(crate) fn find_subslice(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || hay.len() < needle.len() {
        return None;
    }
    hay.windows(needle.len()).position(|w| w == needle)
}

/// Byte-safe chunked transfer decoding (chunk sizes are byte counts).
pub(crate) fn decode_chunked(mut body: &[u8]) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::new();
    loop {
        let nl = match find_subslice(body, b"\r\n") {
            Some(i) => i,
            None => break,
        };
        let line = String::from_utf8_lossy(&body[..nl]);
        let size = match usize::from_str_radix(line.trim(), 16) {
            Ok(s) => s,
            Err(_) => break,
        };
        if size == 0 {
            break;
        }
        let start = nl + 2;
        let end = start + size;
        if body.len() < end {
            out.extend_from_slice(&body[start..]);
            break;
        }
        out.extend_from_slice(&body[start..end]);
        body = &body[end..];
        if body.len() >= 2 && &body[..2] == b"\r\n" {
            body = &body[2..];
        }
    }
    out
}
