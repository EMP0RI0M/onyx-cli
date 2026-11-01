//! WebSocket PTY server — a drop-in replacement for ws_bridge.py at the wire level.
//!
//! Protocol (matches /root/ws_bridge.py exactly so the existing cockpit / xterm.js
//! frontend keeps working):
//!   * server → client: TEXT frames, one per pty read chunk (UTF-8 lossy).
//!   * client → server:
//!       - binary frame  → written to pty verbatim
//!       - text frame    → if it parses as JSON control `{"type":"resize","rows":N,"cols":M}`
//!                          or `{"type":"bin","data":"<base64>"}`, handled; otherwise written
//!                          to pty as raw UTF-8.
//!
//! Single-owner WebSocket + non-blocking socket + poll loop: tungstenite's write/flush
//! is resumable (a WouldBlock leaves the frame in the codec's internal buffer, retried
//! on the next `flush()`), so both directions can be driven from one thread.

use std::io::{Error as IoError, ErrorKind};
use std::net::TcpListener;
use std::os::raw::c_int;
use std::os::unix::io::AsRawFd;
use std::thread;

use tungstenite::{accept, Error as WsError, Message};

use crate::pty;

pub struct ServeOpts {
    pub bind: String,
    pub command: Vec<String>,
    pub rows: u16,
    pub cols: u16,
}

pub fn run(opts: ServeOpts) {
    let listener = match TcpListener::bind(&opts.bind) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("[tektile-term] bind {}: {e}", opts.bind);
            std::process::exit(1);
        }
    };
    println!(
        "[tektile-term] ws pty listening on ws://{}  (rows={} cols={} cmd={:?})",
        opts.bind, opts.rows, opts.cols, opts.command
    );

    for stream in listener.incoming() {
        match stream {
            Ok(tcp) => {
                let cmd = opts.command.clone();
                let (rows, cols) = (opts.rows, opts.cols);
                thread::spawn(move || {
                    if let Err(e) = session(tcp, cmd, rows, cols) {
                        eprintln!("[ws] session ended: {e}");
                    }
                });
            }
            Err(e) => eprintln!("[ws] accept: {e}"),
        }
    }
}

fn session(
    tcp: std::net::TcpStream,
    cmd: Vec<String>,
    rows: u16,
    cols: u16,
) -> Result<(), Box<dyn std::error::Error>> {
    tcp.set_nodelay(true).ok();
    let peer = tcp.peer_addr().map(|a| a.to_string()).unwrap_or_default();

    let mut ws = accept(tcp)?;
    ws.get_ref().set_nonblocking(true)?;
    let tcp_fd = ws.get_ref().as_raw_fd();

    let (master, pid) = pty::spawn_argv(&cmd)?;
    pty::set_winsize(master, rows, cols);
    pty::set_nonblock(master);
    println!("[ws] {peer} attached -> {:?} (pid {pid})", cmd.join(" "));

    let mut buf = [0u8; 65536];
    let mut running = true;

    while running {
        let mut pfds: [libc::pollfd; 2] = [
            libc::pollfd { fd: tcp_fd, events: libc::POLLIN | libc::POLLHUP, revents: 0 },
            libc::pollfd { fd: master, events: libc::POLLIN | libc::POLLHUP, revents: 0 },
        ];
        let pr = unsafe { libc::poll(pfds.as_mut_ptr(), 2, 20) }; // 20ms tick to drive flushes
        if pr < 0 {
            let e = pty::errno();
            if e == libc::EINTR {
                continue;
            }
            eprintln!("[ws] poll errno {e}");
            break;
        }

        // ── master -> ws ──────────────────────────────────────────────────
        if pfds[1].revents & (libc::POLLIN | libc::POLLHUP) != 0 {
            loop {
                let n = unsafe {
                    libc::read(
                        master,
                        buf.as_mut_ptr() as *mut std::os::raw::c_void,
                        buf.len(),
                    )
                };
                if n > 0 {
                    let text = String::from_utf8_lossy(&buf[..n as usize]).into_owned();
                    // send() = write()+flush(); on WouldBlock the frame is already
                    // queued in the codec's out buffer, so STOP draining and let the
                    // per-iteration flush below push it — do not treat it as fatal.
                    if let Err(e) = ws.send(Message::Text(text)) {
                        if !matches!(e, WsError::Io(ref io) if io.kind() == ErrorKind::WouldBlock) {
                            running = false;
                        }
                        break;
                    }
                } else if n == 0 {
                    running = false; // child closed the pty
                    break;
                } else {
                    let e = pty::errno();
                    if e == libc::EAGAIN || e == libc::EWOULDBLOCK {
                        break; // drained for now
                    }
                    running = false; // EIO on pty close etc.
                    break;
                }
            }
        }

        // ── ws -> master ──────────────────────────────────────────────────
        if running && pfds[0].revents & libc::POLLIN != 0 {
            loop {
                match ws.read() {
                    Ok(Message::Text(s)) => handle_text(&s, master, &mut rows_cols(master)),
                    Ok(Message::Binary(b)) => pty::write_all(master, &b),
                    Ok(Message::Ping(p)) => {
                        let _ = ws.write(Message::Pong(p));
                    }
                    Ok(Message::Pong(_)) => {}
                    Ok(Message::Close(_)) => {
                        running = false;
                        break;
                    }
                    Ok(Message::Frame(_)) => {}
                    Err(WsError::Io(e)) if e.kind() == ErrorKind::WouldBlock => break,
                    Err(WsError::ConnectionClosed) | Err(WsError::AlreadyClosed) => {
                        running = false;
                        break;
                    }
                    Err(e) => {
                        eprintln!("[ws] read: {e}");
                        running = false;
                        break;
                    }
                }
            }
        }

        // ── push any queued/automatic frames (pty output, pong replies) ─────
        // Ignores WouldBlock (bytes stay buffered, retried next tick). Only a real
        // IO error or closed state tears the session down.
        if running {
            if let Err(e) = ws.flush() {
                match e {
                    WsError::Io(io) if io.kind() == ErrorKind::WouldBlock => {}
                    WsError::ConnectionClosed | WsError::AlreadyClosed => running = false,
                    other => {
                        eprintln!("[ws] flush: {other}");
                        running = false;
                    }
                }
            }
        }
    }

    // ── teardown ──────────────────────────────────────────────────────────
    unsafe {
        libc::kill(pid, libc::SIGHUP);
        let mut status: c_int = 0;
        libc::waitpid(pid, &mut status, libc::WNOHANG);
        libc::close(master);
    }
    let _ = ws.close(None);
    let _ = ws.flush();
    println!("[ws] {peer} detached, pty cleaned up (pid {pid})");
    Ok(())
}

// Handle one text message per ws_bridge semantics. Resize is applied here; `bin`
// unwraps base64 to master. Anything not matching either is written verbatim.
fn handle_text(s: &str, master: c_int, _resize_fn: &mut dyn FnMut(u16, u16)) {
    let t = s.trim();
    if t.starts_with('{') && t.ends_with('}') {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(t) {
            let typ = v.get("type").and_then(|x| x.as_str()).unwrap_or("");
            if typ == "resize" {
                let r = v.get("rows").and_then(|x| x.as_u64()).unwrap_or(40) as u16;
                let c = v.get("cols").and_then(|x| x.as_u64()).unwrap_or(120) as u16;
                _resize_fn(r, c);
                return;
            }
            if typ == "bin" {
                if let Some(b64) = v.get("data").and_then(|x| x.as_str()) {
                    if let Ok(bytes) = base64_decode(b64) {
                        pty::write_all(master, &bytes);
                    }
                }
                return;
            }
        }
    }
    pty::write_all(master, s.as_bytes());
}

// Bind master to a closure so handle_text can drive TIOCSWINSZ without
// needing the pid (SIGWINCH is best-effort — most shells notice the ioctl alone).
fn rows_cols<'a>(master: c_int) -> impl FnMut(u16, u16) + 'a {
    move |r, c| {
        pty::set_winsize(master, r, c);
    }
}

// Minimal base64 decoder so we don't pull a crate for one small control path.
fn base64_decode(s: &str) -> Result<Vec<u8>, IoError> {
    const TBL: &[u8; 64] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let mut acc: u32 = 0;
    let mut bits: u32 = 0;
    for b in s.bytes() {
        if b == b'=' {
            break;
        }
        if b == b'\n' || b == b'\r' || b == b' ' {
            continue;
        }
        let idx = TBL.iter().position(|&x| x == b).ok_or_else(|| {
            IoError::new(ErrorKind::InvalidData, "invalid base64 char")
        })? as u32;
        acc = (acc << 6) | idx;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Ok(out)
}
