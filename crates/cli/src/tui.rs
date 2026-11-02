//! onyx TUI — a bottom-anchored terminal interface written from scratch in raw
//! ANSI, so the workspace gains no rendering dependencies.
//!
//! Design goals:
//!   - a live region pinned to the foot of the screen (streaming text, spinner,
//!     input card, hint bar) is repainted in place; everything older than that
//!     is transcript history that scrolls away naturally;
//!   - tool activity renders as collapsible rows (`● name(args)` plus an
//!     `<+N lines>` hint); ctrl+o toggles them globally;
//!   - shift+tab cycles approval modes (normal → accept-edits → auto);
//!   - a braille "Thinking…" indicator while a model step runs, esc cancels;
//!   - key-hint footer; ctrl+d twice (or /exit) quits; alt+enter inserts a newline;
//!   - slash autocomplete on ↑/↓/Tab while the draft starts with "/";
//!   - inline permission cards ("y run · a always · n/N refuse") bound to the
//!     Gateway approver slot.
//!
//! Repaint correctness: every rendered entry is hard-wrapped to the terminal's
//! column count first, so one logical entry always occupies exactly one display
//! row. The frame is then emitted between synchronized-update markers and drawn
//! with absolute cursor positioning, which keeps it flicker-free and immune to
//! soft-wrap desync across terminals.
//!
//! Threading: the agent loop runs on a worker thread and talks to the world
//! through two objects shared with the main (drawing/approval) thread —
//! `UiOut` events for rendering and `AskCtl` for confirmations. The Gateway's
//! approver closure blocks on AskCtl until a key answers, which freezes the
//! loop mid-decision for free.

use crate::Opts;
use agent::model::HttpModel;
use agent::{AgentEvent, AgentError, Message, ToolCallAgent};
use std::cell::RefCell;
use std::collections::VecDeque;
use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;
use tools::audit::{ApprovalMode, AuditEntry, Gateway};
use crate::markdown::render_markdown;

// ---------------------------------------------------------------- ANSI/TTY & Ratatui Theme ----

const RESET: &str = "\x1b[0m";
const BOLD: &str = "\x1b[1m";
const DIM: &str = "\x1b[2m";
const FG_240: &str = "\x1b[38;5;240m";
const FG_245: &str = "\x1b[38;5;245m";
const FG_250: &str = "\x1b[38;5;250m";
const GREEN: &str = "\x1b[38;5;42m";     // Clean jade green for success
const RED: &str = "\x1b[38;5;196m";      // Crisp crimson for errors/denials
const YELLOW: &str = "\x1b[38;5;214m";   // Amber for warnings/approval
const BLUE: &str = "\x1b[38;5;75m";      // Subtle slate blue for info
const CYAN: &str = "\x1b[38;5;51m";      // Electric cyan for primary accents
const TRACK_COLOR: &str = "\x1b[38;5;237m"; // Unicode scrollbar track
const THUMB_COLOR: &str = "\x1b[38;5;246m"; // Unicode scrollbar thumb

// Unicode glyphs modelled directly on Grok Build's ratatui-pager glyph engine
#[allow(dead_code)]
mod glyphs {
    pub const PROMPT_ARROW: &str = "❯ ";
    pub const BALLOT_X: &str = "✗";
    pub const CHECK_MARK: &str = "✓";
    pub const ACCENT_BAR: &str = "┃";
    pub const SELECTION_BAR: &str = "▏";
    pub const CHEVRON: &str = "›";
    pub const CHEVRON_DOWN: &str = "▾";
    pub const DIAMOND_FILLED: &str = "◆";
    pub const DIAMOND_HOLLOW: &str = "◇";
    pub const TOKEN_ARROW: &str = "⇣";
    pub const COPY_ICON: &str = "⧉";
    pub const TRACK_CHAR: char = '│';
    pub const THUMB_CHAR: char = '█';
}

/// Raw-mode guard: restores termios on drop, including panic unwind.
struct RawMode(libc::termios);

impl RawMode {
    fn enter() -> Option<Self> {
        let mut t: libc::termios = unsafe { std::mem::zeroed() };
        if unsafe { libc::tcgetattr(0, &mut t) } != 0 {
            return None;
        }
        let mut raw = t;
        unsafe { libc::cfmakeraw(&mut raw) };
        // Retain standard post-processing (OPOST and ONLCR) so \n translates
        // to CR+LF (\r\n) on output and doesn't stair-step across the screen.
        raw.c_oflag |= libc::OPOST | libc::ONLCR;
        // Keep signals we want delivered as BYTES off the signal path: ISIG
        // would turn ctrl+c into SIGINT and kill us before the reader sees it.
        raw.c_lflag &= !(libc::ICANON | libc::ECHO | libc::ISIG);
        raw.c_cc[libc::VMIN] = 1;
        raw.c_cc[libc::VTIME] = 0;
        if unsafe { libc::tcsetattr(0, libc::TCSANOW, &raw) } != 0 {
            return None;
        }
        Some(Self(t))
    }
}

impl Drop for RawMode {
    fn drop(&mut self) {
        unsafe { libc::tcsetattr(0, libc::TCSANOW, &self.0) };
    }
}

fn term_size() -> (u16, u16) {
    let mut ws: libc::winsize = unsafe { std::mem::zeroed() };
    if unsafe { libc::ioctl(1, libc::TIOCGWINSZ, &mut ws) } == 0 && ws.ws_row > 0 && ws.ws_col > 0 {
        (ws.ws_row, ws.ws_col)
    } else {
        (24, 80)
    }
}

/// Terminal cells a char occupies: CJK/fullwidth blocks are 2, combining marks
/// and control-ish codepoints are 0, everything else 1. Approximate wcwidth —
/// enough for the box-drawing, braille spinner and CJK we can actually print.
pub(crate) fn char_width(c: char) -> usize {
    let cp = c as u32;
    if cp == 0 {
        return 0;
    }
    // Combining marks (the ranges that show up in real input).
    if (0x0300..=0x036F).contains(&cp)
        || (0x1AB0..=0x1AFF).contains(&cp)
        || (0x20D0..=0x20FF).contains(&cp)
        || (0xFE00..=0xFE0F).contains(&cp)
        || (0xFE20..=0xFE2F).contains(&cp)
    {
        return 0;
    }
    // East-Asian wide / fullwidth blocks.
    if (0x1100..=0x115F).contains(&cp)
        || (0x2E80..=0x303E).contains(&cp)
        || (0x3041..=0x33FF).contains(&cp)
        || (0x3400..=0x4DBF).contains(&cp)
        || (0x4E00..=0x9FFF).contains(&cp)
        || (0xA000..=0xA4CF).contains(&cp)
        || (0xAC00..=0xD7A3).contains(&cp)
        || (0xF900..=0xFAFF).contains(&cp)
        || (0xFE30..=0xFE4F).contains(&cp)
        || (0xFF00..=0xFF60).contains(&cp)
        || (0xFFE0..=0xFFE6).contains(&cp)
        || (0x1F300..=0x1F64F).contains(&cp)
        || (0x1F900..=0x1F9FF).contains(&cp)
        || (0x20000..=0x3FFFD).contains(&cp)
    {
        return 2;
    }
    1
}

/// Visible cell width of a string that may carry ANSI SGR sequences.
fn ansi_width(s: &str) -> usize {
    let mut w = 0usize;
    let mut esc = false;
    for c in s.chars() {
        if esc {
            if c.is_ascii_alphabetic() || c == 'm' {
                esc = false;
            }
            continue;
        }
        if c == '\x1b' {
            esc = true;
            continue;
        }
        w += char_width(c);
    }
    w
}

/// Shopify cli-ui style horizontal framing rule:
/// `── title ────────────────────────────────────`
fn frame_header(title: &str, width: usize, border_color: &str) -> String {
    let vis = ansi_width(title);
    let rem = width.saturating_sub(vis + 4).max(2);
    format!("{border_color}──{RESET} {title} {border_color}{}{RESET}", "─".repeat(rem))
}

/// Hard-wrap one line at exactly `width` visible cells, preserving embedded ANSI escape sequences.
fn hard_wrap_line(text: &str, width: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut used = 0usize;
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    let n = chars.len();

    while i < n {
        let c = chars[i];
        if c == '\x1b' {
            cur.push(c);
            i += 1;
            while i < n {
                let ec = chars[i];
                cur.push(ec);
                i += 1;
                if ec.is_ascii_alphabetic() || ec == 'm' {
                    break;
                }
            }
            continue;
        }

        let cw = char_width(c).min(width);
        if used + cw > width && !cur.is_empty() {
            out.push(std::mem::take(&mut cur));
            used = 0;
        }
        cur.push(c);
        used += cw;
        i += 1;
    }

    if !cur.is_empty() || out.is_empty() {
        out.push(cur);
    }
    out
}

/// Split on newlines, then hard-wrap each row to the terminal width so no
/// rendered line can ever exceed it.
fn fit_lines(text: &str, width: usize) -> Vec<String> {
    let mut out = Vec::new();
    for l in text.split('\n') {
        out.extend(hard_wrap_line(l, width));
    }
    out
}

/// Decode one stdin event into (chars, code). Escape sequences map to codes
/// ≥1000 (arrows, page keys) or 10 (shift+tab). Unrecognized → ('?', 0).
/// A merged run of printable bytes (pasted text / fast typing) yields every
/// char at once; control bytes are always single events.
fn decode_key(buf: &[u8]) -> (Vec<char>, u32) {
    let one = |c: char| vec![c];
    match buf {
        [] => (one('\0'), 0),
        [0x1b] => (one('\x1b'), 0),
        [0x1b, b'[', rest @ ..] => (one('\0'), match rest {
            [b'A'] => 1000,
            [b'B'] => 1001,
            [b'C'] => 1002,
            [b'D'] => 1003,
            [b'Z'] => 10,           // ESC [ Z = shift+tab
            [b'H'] => 1006,         // home
            [b'F'] => 1007,         // end
            [b'~', b'3'] => 1004,   // delete
            [b'5', b'~'] => 1008,   // page up
            [b'6', b'~'] => 1009,   // page down
            _ => 0,
        }),
        [0x1b, b'O', b'D'] => (one('\0'), 10),   // some terminals: ESC O D = shift+tab
        [0x1b, b'\r'] | [0x1b, b'\n'] => (one('\n'), 1), // alt+enter → newline
        [b'\t'] => (one('\t'), 0),
        [0x0d] | [0x0a] => (one('\r'), 0),
        [0x7f] | [0x08] => (one('\u{7f}'), 0),
        [0x03] => (one('\x03'), 0),
        [0x04] => (one('\x04'), 0),
        [0x0c] => (one('\x0c'), 0),
        [0x0e] => (one('\x0e'), 0), // ctrl+n: next suggestion
        [0x10] => (one('\x10'), 0), // ctrl+p: prev suggestion
        [0x0f] => (one('\x0f'), 0), // ctrl+o: expand/collapse
        [b] if *b >= 0x20 && *b < 0x7f => {
            if buf.iter().all(|b| *b >= 0x20) {
                let s = String::from_utf8_lossy(buf);
                (s.chars().collect(), 0)
            } else {
                (one(*b as char), 0)
            }
        }
        [_, ..] => {
            let s = String::from_utf8_lossy(buf);
            if buf.iter().all(|b| *b >= 0x20) {
                (s.chars().collect(), 0)
            } else {
                (one(s.chars().next().unwrap_or(' ')), 0)
            }
        }
    }
}

// ------------------------------------------------------------ input pump ----

/// Background stdin pump: reads raw bytes, splits them into key events, and
/// forwards each event over an mpsc channel. On exit the joiner drops the
/// sender, the thread sees send() fail and exits, cooked mode is restored by
/// RawMode's Drop, and any unread keystrokes flush to the shell afterwards.
struct InputPump {
    rx: Receiver<Vec<u8>>,
    stop: Arc<Mutex<bool>>,
    handle: Option<thread::JoinHandle<()>>,
}

impl InputPump {
    fn start() -> Self {
        let (tx, rx) = channel::<Vec<u8>>();
        let stop = Arc::new(Mutex::new(false));
        let stop2 = stop.clone();
        let handle = thread::spawn(move || {
            let mut tmp = [0u8; 64];
            loop {
                if *stop2.lock().unwrap() {
                    break;
                }
                let n = unsafe { libc::read(0, tmp.as_mut_ptr() as *mut libc::c_void, tmp.len()) };
                if n <= 0 {
                    if n < 0 && io::Error::last_os_error().raw_os_error() == Some(libc::EINTR) {
                        continue;
                    }
                    break;
                }
                let got = &tmp[..n as usize];
                let mut i = 0usize;
                while i < got.len() {
                    let b = got[i];
                    let end = if b == 0x1b {
                        // consume CSI: ESC [ params… final-byte, ESC O x, or ESC char
                        let mut j = i + 1;
                        if j < got.len() && (got[j] == b'[' || got[j] == b'O') {
                            j += 1;
                            while j < got.len() && !(0x40..=0x7e).contains(&got[j]) {
                                j += 1;
                            }
                            j = (j + 1).min(got.len());
                        } else {
                            j = (j + 1).min(got.len());
                        }
                        j
                    } else {
                        // UTF-8 continuation bytes belong to this one key event
                        let mut j = i + 1;
                        while j < got.len() && got[j] & 0xc0 == 0x80 {
                            j += 1;
                        }
                        j
                    };
                    if tx.send(got[i..end].to_vec()).is_err() {
                        return; // receiver gone: exit the pump
                    }
                    i = end;
                }
            }
        });
        Self { rx, stop, handle: Some(handle) }
    }

    /// Next key event, or None after `poll` of quiet. Queued escape sequences
    /// are parked in LEFTOVER and take priority on the following call;
    /// printable runs merge so pasted text doesn't lag.
    fn next_key(&self, poll: Duration) -> Option<(Vec<char>, u32)> {
        let mut buf = match LEFTOVER.with(|l| l.borrow_mut().pop_front()) {
            Some(b) => b,
            None => self.rx.recv_timeout(poll).ok()?,
        };
        while let Ok(more) = self.rx.try_recv() {
            if more.first() == Some(&0x1b) || buf.first() == Some(&0x1b) {
                LEFTOVER.with(|l| l.borrow_mut().push_back(more));
                break;
            }
            // A control byte (enter, tab, ctrl+…) ends the printable run: it
            // must stay its own event. Merging it would make decode_key see a
            // multi-byte buffer and keep only the first char — pasted or typed
            // fast text followed by enter would submit just the first line.
            if buf.len() + more.len() <= 64 && more.iter().all(|b| *b >= 0x20) {
                buf.extend(more);
            } else {
                LEFTOVER.with(|l| l.borrow_mut().push_back(more));
                break;
            }
        }
        Some(decode_key(&buf))
    }

    fn shutdown(mut self) {
        *self.stop.lock().unwrap() = true;
        drop(self.rx); // wake the thread if blocked in send()/read()
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

thread_local! {
    static LEFTOVER: RefCell<VecDeque<Vec<u8>>> = RefCell::new(VecDeque::new());
}

// ------------------------------------------------------------ ui messages ----

/// One transcript entry. Tool rows carry their output for ctrl+o expansion;
/// streaming text lives in the tail, not here.
enum Item {
    Banner,
    User(String),
    Tool {
        name: String,
        args: String,
        ok: bool,
        denied: bool,
        duration: Option<Duration>,
        output: String,
    },
    Answer(String),
    Info(String),
    Error(String),
}

/// Request from the UI thread to the worker. The worker owns `history` and
/// `hm`, so anything that mutates them (/clear, /model, /skill) or reads the
/// audit log must run there, between turns.
enum Ctl {
    Prompt(String, usize),
    Clear(String),
    Resume(Vec<Message>),
    SetModel(String),
    SetEffort(String),
    SystemAppend(String),
    Audit,
    DropTranscript,
}

/// Events the worker thread sends to the UI thread.
enum UiOut {
    Ev(AgentEvent),
    Info(String),
    TurnDone(Result<(String, usize), AgentError>),
    Audit(Vec<AuditEntry>),
    ModelName(String),
    /// (message count, total content bytes) — mirrors the worker-owned history
    /// so `/usage` can report without touching it.
    History(usize, usize),
}

/// Request/response channel for interactive approvals. The worker blocks in
/// `wait()` until the UI thread answers — freezing the agent loop mid-call.
struct AskCtl {
    state: Mutex<AskState>,
    cv: std::sync::Condvar,
}
struct AskState {
    pending: Option<(String, String, String)>,
    answer: Option<bool>,
    approve_all: bool,
}

impl AskCtl {
    fn new() -> Self {
        Self {
            state: Mutex::new(AskState { pending: None, answer: None, approve_all: false }),
            cv: std::sync::Condvar::new(),
        }
    }
    fn request(&self, tool: &str, args: &str, reason: &str) -> bool {
        let mut s = self.state.lock().unwrap();
        if s.approve_all {
            return true;
        }
        s.pending = Some((tool.to_string(), args.to_string(), reason.to_string()));
        s.answer = None;
        while s.answer.is_none() {
            s = self.cv.wait(s).unwrap();
        }
        let a = s.answer.unwrap_or(false);
        s.pending = None;
        a
    }
    fn pending(&self) -> Option<(String, String, String)> {
        self.state.lock().unwrap().pending.clone()
    }
    fn answer(&self, yes: bool, all: bool) {
        let mut s = self.state.lock().unwrap();
        s.answer = Some(yes);
        if all {
            s.approve_all = true;
        }
        self.cv.notify_all();
    }
}

// ------------------------------------------------------------- view helpers ----

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        None => String::new(),
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
    }
}

fn clip_chars(s: &str, n: usize) -> String {
    let mut out = String::new();
    for c in s.chars().take(n.saturating_sub(1)) {
        out.push(c);
    }
    if s.chars().count() > n.saturating_sub(1) {
        out.push('…');
    }
    out
}

fn first_line(s: &str) -> &str {
    s.lines().next().unwrap_or("")
}

/// Clean up LLM response text that may contain raw JSON fallback tool blocks
/// (such as {"tool":"terminate","args":{...}} or {"tool":"ask_human","args":{...}} or {"question": "..."}).
/// If present, extracts the actual message content so raw JSON schema blobs don't leak into the UI.
fn clean_model_text(text: &str) -> String {
    let t = text.trim();
    // Case 1: `→ tool_name({"question":"..."})` or `→ tool_name({...})` from agent loop tool calls
    if let Some(rest) = t.strip_prefix("→ ") {
        if let Some(open_paren) = rest.find('(') {
            let tool_name = rest[..open_paren].trim();
            if let Some(close_paren) = rest.rfind(')') {
                let inside = &rest[open_paren + 1..close_paren];
                if tool_name == "ask_human" || tool_name == "terminate" {
                    if let Ok(v) = serde_json::from_str::<serde_json::Value>(inside) {
                        if let Some(q) = v.get("question").or_else(|| v.get("content")).or_else(|| v.get("answer")).and_then(|s| s.as_str()) {
                            return q.to_string();
                        }
                    }
                    return inside.trim_matches('"').to_string();
                }
            }
        }
    }

    if t.starts_with('{') && t.contains("\"tool\"") {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(t) {
            let tool = v.get("tool").and_then(|s| s.as_str()).unwrap_or("");
            if tool == "terminate" || tool == "ask_human" {
                if let Some(args) = v.get("args") {
                    if let Some(content) = args.get("content").or_else(|| args.get("question")).or_else(|| args.get("answer")).and_then(|c| c.as_str()) {
                        return content.to_string();
                    }
                }
            }
        }
    }
    // Direct question or content objects: {"question":"..."}
    if t.starts_with('{') && t.contains("\"question\"") {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(t) {
            if let Some(q) = v.get("question").and_then(|s| s.as_str()) {
                return q.to_string();
            }
        }
    }
    // terminate-shaped result envelopes the model echoes back verbatim:
    // {"status":"success","content":"42"}. Show only the content.
    if t.starts_with('{') && t.contains("\"status\"") && t.contains("\"content\"") {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(t) {
            if v.get("tool").is_none()
                && v.as_object().map_or(false, |o| o.len() == 2)
                && v.get("status").and_then(|s| s.as_str()) == Some("success")
            {
                if let Some(c) = v.get("content").and_then(|c| c.as_str()) {
                    return c.to_string();
                }
            }
        }
    }
    text.to_string()
}

fn pretty_args(name: &str, args: &str) -> String {
    let v: serde_json::Value = serde_json::from_str(args).unwrap_or_else(|_| serde_json::json!(args));
    let obj = match v.as_object() {
        Some(o) => o,
        None => return clip_chars(&v.to_string(), 100),
    };
    if name == "bash" || name == "shell" {
        if let Some(cmd) = obj.get("command").or_else(|| obj.get("cmd")).and_then(|x| x.as_str()) {
            return format!("$ {}", clip_chars(first_line(cmd), 90));
        }
    }
    if name == "ask_human" {
        if let Some(q) = obj.get("question").or_else(|| obj.get("content")).and_then(|x| x.as_str()) {
            return format!("question: {}", clip_chars(first_line(q), 90));
        }
    }
    if name == "editor" || name == "file_write" || name == "file_read" || name == "view" {
        let path = obj.get("path").or_else(|| obj.get("file_path")).and_then(|x| x.as_str()).unwrap_or("");
        let cmd = obj.get("command").and_then(|x| x.as_str()).unwrap_or("");
        if !cmd.is_empty() && !path.is_empty() {
            return format!("{cmd} {path}");
        }
        if !path.is_empty() {
            return path.to_string();
        }
    }
    if name == "websearch" || name == "web_search" {
        if let Some(q) = obj.get("query").and_then(|x| x.as_str()) {
            return format!("query: {q}");
        }
    }
    for key in ["command", "code", "path", "text", "query", "url"] {
        if let Some(s) = obj.get(key).and_then(|x| x.as_str()) {
            if obj.len() == 1 {
                return clip_chars(first_line(s), 100);
            }
        }
    }
    let parts: Vec<String> = obj
        .iter()
        .map(|(k, val)| {
            let vs = match val {
                serde_json::Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            format!("{k}={}", clip_chars(first_line(&vs), 40))
        })
        .collect();
    clip_chars(&parts.join(", "), 110)
}

const SPINNER: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

const SLASH_COMMANDS: &[(&str, &str)] = &[
    ("/help", "commands + keys"),
    ("/model", "[name] show/set model"),
    ("/effort", "<low|med|high|max|none> reasoning level"),
    ("/settings", "view system configurations & modes"),
    ("/steps", "N set max steps"),
    ("/clear", "new conversation"),
    ("/usage", "real token metrics + tool calls"),
    ("/audit", "gateway audit log"),
    ("/tools", "list connected tools"),
    ("/skill", "<name> load a skill from ~/.onyx/skills"),
    ("/sessions", "list saved transcripts"),
    ("/resume", "<id|prefix> replay a transcript"),
    ("/mode", "cycle approval mode (Normal, Edits, YOLO)"),
    ("/exit", "quit"),
];

// -------------------------------------------------------------------- core ----

/// Raw-pointer handoff of the session borrows to the worker. Legality is the
/// documented discipline in `start_worker`: everything pointed at lives in
/// `tui_session`'s frame for the whole session and the worker is joined by
/// `finish` before that frame (or these fields) can be touched again.
struct Job {
    gw: *const Gateway<dyn tools::policy::Policy>,
    hm: *mut HttpModel,
    history: Box<Vec<Message>>,
    opts: *const Opts,
}
unsafe impl Send for Job {}

pub struct Tui<'a> {
    gw: &'a mut Gateway<dyn tools::policy::Policy>,
    hm: &'a mut HttpModel,
    opts: &'a mut Opts,
    history_len: usize,
    history_bytes: usize,
    items: Vec<Item>,
    input: String,
    cursor: usize, // char index into input
    expanded: bool,
    tick: usize,
    thinking: bool,
    tail: String,          // streamed text of the current step
    tail_scrolled: usize,  // chars dropped from the front when tail overflows
    running: bool,
    cancel: Arc<Mutex<bool>>,
    ask: Arc<AskCtl>,
    ui_rx: Option<Receiver<UiOut>>,
    ctl_tx: Option<Sender<Ctl>>,
    worker: Option<thread::JoinHandle<()>>,
    last_ctrl_d: Option<std::time::Instant>,
    quit: bool,
    height: usize,
    width: usize,
    session: Option<agent::session::Transcript>,
    scroll_offset: usize,
    active_tool_start: Option<std::time::Instant>,
    workspace_dir: String,
    slash_selected: usize,
    cached_models: Vec<String>,
    mcp_count: usize,
    skill_count: usize,
    real_prompt_tokens: usize,
    real_completion_tokens: usize,
    real_total_tokens: usize,
    real_cached_tokens: usize,
}


impl<'a> Tui<'a> {
    fn new(
        gw: &'a mut Gateway<dyn tools::policy::Policy>,
        hm: &'a mut HttpModel,
        opts: &'a mut Opts,
    ) -> Self {
        let cached_models = hm.list_models().unwrap_or_default();
        let mcp_count = if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
            let mcp_cfg = home.join(".onyx/mcp.json");
            if let Ok(content) = std::fs::read_to_string(mcp_cfg) {
                if let Ok(val) = serde_json::from_str::<serde_json::Value>(&content) {
                    val.get("mcpServers").and_then(|s| s.as_object()).map(|o| o.len()).unwrap_or(0)
                } else { 0 }
            } else { 0 }
        } else { 0 };

        let skill_count = if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
            let mut cat = skills::SkillCatalog::new();
            cat.scan_dir(&home.join(".onyx/skills"));
            cat.skills.len()
        } else { 0 };

        Self {
            gw,
            hm,
            opts,
            history_len: 0,
            history_bytes: 0,
            items: vec![Item::Banner],
            input: String::new(),
            cursor: 0,
            expanded: false,
            tick: 0,
            thinking: false,
            tail: String::new(),
            tail_scrolled: 0,
            running: false,
            cancel: Arc::new(Mutex::new(false)),
            ask: Arc::new(AskCtl::new()),
            ui_rx: None,
            ctl_tx: None,
            worker: None,
            last_ctrl_d: None,
            quit: false,
            height: term_size().0.max(4) as usize,
            width: term_size().1.max(20) as usize,
            session: None,
            scroll_offset: 0,
            active_tool_start: None,
            slash_selected: 0,
            cached_models,
            mcp_count,
            skill_count,
            real_prompt_tokens: 0,
            real_completion_tokens: 0,
            real_total_tokens: 0,
            real_cached_tokens: 0,
            workspace_dir: std::env::current_dir()
                .map(|p| {
                    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
                        if let Ok(rel) = p.strip_prefix(&home) {
                            return format!("~/{}", rel.display());
                        }
                    }
                    p.display().to_string()
                })
                .unwrap_or_else(|_| ".".into()),
        }
    }

    /// Refresh the viewport geometry (the user may have resized the window).
    fn sync_size(&mut self) {
        let (rows, cols) = term_size();
        self.height = rows.max(4) as usize;
        self.width = cols.max(20) as usize;
    }

    fn session_label(&self) -> String {
        self.session
            .as_ref()
            .and_then(|t| t.path().file_stem().and_then(|s| s.to_str()))
            .map(|s| s.to_string())
            .unwrap_or_else(|| "ready".to_string())
    }

    /// Install the TUI's own approver into the gateway and start the worker.
    /// Must be called once before the first prompt. The worker moves the
    /// gateway/model/history/opts borrows in for its whole life (reached via
    /// the raw-pointer `Job`, joined by `finish`); it shares only the Sync
    /// `UiState` with the main thread, which is what keeps the approver
    /// closure Send + Sync.
    fn start_worker(&mut self) {
        let ask = self.ask.clone();
        let cancel = self.cancel.clone();

        let (ct, cr) = channel::<Ctl>();
        let (ut, ur) = channel::<UiOut>();
        self.ui_rx = Some(ur);
        self.ctl_tx = Some(ct);

        // Open a durable transcript for this run so `/resume` can replay it later.
        // Best-effort: if persistence fails we simply run without it.
        let sid = agent::session::new_session_id();
        let transcript = agent::session::Transcript::open(&sid, &self.hm.model);
        if let Some(t) = &transcript {
            self.items.push(Item::Info(format!("session {sid} · {}", t.path().display())));
        }
        self.session = transcript;
        let transcript = self.session.take(); // move into the worker below

        // The loop needs &Gateway + &mut HttpModel + history + opts together;
        // they live in tui_session's frame for the whole session and the worker
        // is joined before anything can touch them again. Bundle them behind a
        // raw pointer handoff created here and destroyed there.
        let job = Box::new(Job {
            gw: self.gw as *const Gateway<dyn tools::policy::Policy>,
            hm: self.hm as *mut HttpModel,
            history: Box::new(Vec::new()),
            opts: self.opts as *const Opts,
        });

        let worker = thread::spawn(move || {
            // New session so bash/python children get their OWN controlling
            // terminal: under raw mode a child reading inherited fd 0 would see
            // single raw keystrokes instead of cooked line input.
            unsafe { libc::setsid() };
            let gw: &Gateway<dyn tools::policy::Policy> = unsafe { &*job.gw };
            let hm: &mut HttpModel = unsafe { &mut *job.hm };
            let mut history: Vec<Message> = *job.history;
            let _opts: &Opts = unsafe { &*job.opts };
            let mut transcript = transcript;

            // Approver runs ON the worker thread (it blocks on AskCtl). It may
            // only touch Sync state — AskCtl is enough: the main thread renders
            // the approval card from `ask.pending()` while it polls keys.
            gw.set_approver(move |tool: &str, args: &str, reason: &str| -> bool {
                ask.request(tool, args, reason)
            });

            while let Ok(msg) = cr.recv() {
                match msg {
                    Ctl::Prompt(prompt, max_steps) => {
                        *cancel.lock().unwrap() = false;
                        let cut = history.len();
                        // run_turn_observed appends the user message itself, so we
                        // do NOT pre-push it — that was the double-write. We persist
                        // the whole turn from the returned report instead, which
                        // carries the user+assistant+tool messages even when the
                        // live history is later truncated (cancel) or errored.
                        let mut agent = ToolCallAgent::new(gw);
                        agent.max_steps = max_steps;
                        let mut sink = |ev: AgentEvent| {
                            let _ = ut.send(UiOut::Ev(ev));
                        };
                        let res = agent.run_turn_observed(hm, &prompt, &mut history, &mut sink);
                        if let Some(t) = &transcript {
                            // Persist exactly the messages this turn added. On cancel
                            // or error the live history is rolled back below, but the
                            // report still holds the full turn, so nothing is lost.
                            let src: &[Message] = match &res {
                                Ok(rep) => &rep.history[cut..],
                                Err(_) => &history[cut..],
                            };
                            for m in src {
                                t.push(m);
                            }
                        }
                        if *cancel.lock().unwrap() {
                            // Drop everything this cancelled turn appended to memory.
                            history.truncate(cut);
                        }
                        let _ = ut.send(UiOut::TurnDone(match res {
                            Ok(rep) => Ok((rep.answer, rep.steps)),
                            Err(e) => Err(e),
                        }));
                    }
                    Ctl::Clear(system) => {
                        history = vec![Message::text("system", system)];
                        // Fresh conversation: seal the old transcript, start a new one.
                        if let Some(t) = &transcript {
                            let _ = ut.send(UiOut::Info(format!(
                                "archived session {}",
                                t.path().file_stem().and_then(|s| s.to_str()).unwrap_or("?")
                            )));
                        }
                        transcript = None;
                        let sid = agent::session::new_session_id();
                        if let Some(t) = agent::session::Transcript::open(&sid, &hm.model) {
                            let _ = ut.send(UiOut::Info(format!(
                                "session {sid} · {}",
                                t.path().display()
                            )));
                            transcript = Some(t);
                        }
                    }
                    Ctl::Resume(msgs) => {
                        history = if msgs.is_empty() {
                            vec![Message::text("system", hm.system.clone())]
                        } else {
                            msgs
                        };
                        transcript = None; // main thread already opened the replay transcript
                    }                    Ctl::SetModel(m) => {
                        hm.model = m;
                        let _ = ut.send(UiOut::ModelName(hm.model.clone()));
                    }
                    Ctl::SetEffort(eff) => {
                        hm.effort = eff;
                    }
                    Ctl::SystemAppend(s) => {
                        hm.system.push_str(&s);
                    }
                    Ctl::Audit => {
                        let _ = ut.send(UiOut::Audit(gw.audit.snapshot()));
                    }
                    Ctl::DropTranscript => {
                        transcript = None; // main thread owns the replacement
                    }
                }
                // Cheap copy for /usage-style reads; the worker owns history.
                let _ = ut.send(UiOut::History(history.len(), history.iter().map(|m| m.content.len()).sum()));
            }
        });
        self.worker = Some(worker);
    }

    /// Tear down the worker: close its control channel, let it finish the
    /// current recv, and join so the raw-pointer borrow ends before return.
    pub fn finish(&mut self) {
        self.ctl_tx = None;
        if let Some(h) = self.worker.take() {
            let _ = h.join();
        }
    }

    /// Redraw the live region in place. render_lines guarantees one display row
    /// per entry (hard-wrapped to `width`, no embedded newlines), so the frame
    /// occupies exactly `lines.len()` rows at the bottom of the viewport.
    ///
    /// Why ESC[H + ESC[J rather than ESC[0J: 0J erases only BELOW the cursor,
    /// and once the transcript has scrolled, stale copies of the live region
    /// can sit ABOVE it — those never get erased, which is how every frame
    /// ended up appended down the screen. Absolute-positioning the whole frame
    /// (home, clear visible screen, draw, park cursor) makes each repaint exact
    /// regardless of where the previous frame landed.
    fn draw(&mut self) {
        self.sync_size();
        let lines = self.render_lines();
        let mut out = String::new();
        // DEC private mode 2026: deferred painting for flicker-free frame sync
        out.push_str("\x1b[?2026h");
        out.push_str("\x1b[H\x1b[2J"); // home + clear entire screen

        for l in &lines {
            out.push_str(l);
            out.push('\n');
        }

        // Always park cursor at the persistent input composer line
        if self.height >= 4 {
            let cursor_row = self.height.saturating_sub(2);
            let col = (4 + self.cursor_chars()).min(self.width.max(1));
            out.push_str(&format!("\x1b[{};{}H", cursor_row, col));
        }
        out.push_str("\x1b[?2026l");
        io::stdout().write_all(out.as_bytes()).ok();
        io::stdout().flush().ok();
    }

    fn render_lines(&self) -> Vec<String> {
        let w = self.width.max(20);
        let mut out = Vec::with_capacity(self.height);

        // 1. Header (fixed at top: 2 lines)
        let header_lines = self.item_lines(&Item::Banner);
        let header_count = header_lines.len();
        out.extend(header_lines);

        // 2. Footer lines (fixed at bottom)
        let status = self.status_lines_v().into_iter().flat_map(|l| fit_lines(&l, w)).collect::<Vec<_>>();
        let slash_popup = self.slash_dropdown_lines().into_iter().flat_map(|l| fit_lines(&l, w)).collect::<Vec<_>>();
        let input = self.input_lines().into_iter().flat_map(|l| fit_lines(&l, w)).collect::<Vec<_>>();
        let hints = fit_lines(&self.hints(), w);

        let spin_row = if self.thinking {
            let spin = format!(
                "{DIM}{spin} Thinking… {RESET}{DIM}(esc to cancel){RESET}",
                spin = SPINNER[self.tick % SPINNER.len()]
            );
            fit_lines(&spin, w)
        } else {
            Vec::new()
        };

        let footer_count = spin_row.len() + status.len() + slash_popup.len() + input.len() + hints.len();
        let avail_content = self.height.saturating_sub(header_count + footer_count).max(1);

        // 3. Transcript lines (all non-banner items + tail)
        let mut content: Vec<String> = Vec::new();
        for it in &self.items {
            if matches!(it, Item::Banner) {
                continue;
            }
            content.extend(self.item_lines(it));
        }
        if self.tail_visible() {
            content.extend(self.tail_lines());
        }
        let all_content: Vec<String> = content.into_iter().flat_map(|l| fit_lines(&l, w)).collect();

        // Calculate scroll window
        let total_content = all_content.len();
        let max_scroll = total_content.saturating_sub(avail_content);
        let scroll = self.scroll_offset.min(max_scroll);
        let visible_end = total_content.saturating_sub(scroll);
        let visible_start = visible_end.saturating_sub(avail_content);

        let slice = if visible_end > visible_start && visible_start < total_content {
            &all_content[visible_start..visible_end.min(total_content)]
        } else {
            &all_content[..total_content.min(avail_content)]
        };

        // Grok-style smooth Unicode scrollbar calculations:
        // When content overflows the viewport, render a track and proportional thumb on the right margin.
        let needs_bar = total_content > avail_content && avail_content >= 2;
        let (thumb_top, thumb_len) = if needs_bar {
            let track_len = avail_content;
            let tlen = ((track_len * avail_content) / total_content).max(1).min(track_len);
            let free_track = track_len.saturating_sub(tlen);
            let pos = if max_scroll == 0 {
                0
            } else {
                let scrolled_down = max_scroll.saturating_sub(scroll);
                (scrolled_down * free_track) / max_scroll
            };
            (pos, tlen)
        } else {
            (0, 0)
        };

        // Fill content area with right-anchored scrollbar
        for (idx, line) in slice.iter().enumerate() {
            if needs_bar {
                let bar_char = if idx >= thumb_top && idx < thumb_top + thumb_len {
                    format!("{THUMB_COLOR}{}{RESET}", glyphs::THUMB_CHAR)
                } else {
                    format!("{TRACK_COLOR}{}{RESET}", glyphs::TRACK_CHAR)
                };
                let vis_len = ansi_width(line);
                let pad = w.saturating_sub(vis_len + 1);
                out.push(format!("{line}{}{bar_char}", " ".repeat(pad)));
            } else {
                out.push(line.clone());
            }
        }

        // Pad blank lines between transcript and footer to eliminate blank gap jumping
        // and keep the input composer permanently pinned to the bottom of the screen.
        let rendered_content_lines = slice.len();
        if rendered_content_lines < avail_content {
            for idx in rendered_content_lines..avail_content {
                if needs_bar {
                    let bar_char = if idx >= thumb_top && idx < thumb_top + thumb_len {
                        format!("{THUMB_COLOR}{}{RESET}", glyphs::THUMB_CHAR)
                    } else {
                        format!("{TRACK_COLOR}{}{RESET}", glyphs::TRACK_CHAR)
                    };
                    let pad = w.saturating_sub(1);
                    out.push(format!("{}{bar_char}", " ".repeat(pad)));
                } else {
                    out.push(String::new());
                }
            }
        }

        // 4. Render Footer
        out.extend(spin_row);
        out.extend(status);
        out.extend(slash_popup);
        out.extend(input);
        out.extend(hints);

        // Truncate to exact terminal height to avoid unexpected terminal line wraps
        out.truncate(self.height);
        out
    }

    fn tail_visible(&self) -> bool {
        self.running && !self.tail.is_empty()
    }

    fn tail_lines(&self) -> Vec<String> {
        let vis: String = self.tail.chars().skip(self.tail_scrolled).collect();
        let clean = clean_model_text(&vis);
        let wrap_w = self.width.saturating_sub(4).max(20);
        let mut out = Vec::new();
        for para in clean.split('\n') {
            let wrapped = wrap(para, wrap_w);
            for (i, w) in wrapped.iter().enumerate() {
                let mark = if i == 0 { "◐ " } else { "  " };
                out.push(format!("{FG_250}{mark}{t}{RESET}", t = w));
            }
        }
        out
    }

    fn item_lines(&self, it: &Item) -> Vec<String> {
        let w = self.width.max(20);
        match it {
            Item::Banner => {
                let logo_l1 = format!("{CYAN}  ██████{RESET}                              {BOLD}ONYX{RESET} {DIM}v0.1.0{RESET}");
                let logo_l2 = format!("{CYAN}██      ██{RESET}                            {DIM}Signed in Local Engine{RESET}");
                let logo_l3 = format!("{CYAN}██  ██  ██{RESET}  {BOLD}ONYX CLI{RESET} {DIM}v0.1.0{RESET}           {DIM}session: {}{RESET}", self.session_label());
                let logo_l4 = format!("{CYAN}██    ██{RESET}    {DIM}dir: {FG_245}{}{RESET}", self.workspace_dir);
                let logo_l5 = format!("{CYAN}  ████  ██{RESET}  {DIM}tools: {}{RESET}", self.gw.tools.names().len());

                let _card_w = 40;
                let c_top = format!("{FG_240}╭─ What's new (v0.1.0): ──────────╮{RESET}");
                let c_l1  = format!("{FG_240}│{RESET} - Full Grok Tools architecture  {FG_240}│{RESET}");
                let c_l2  = format!("{FG_240}│{RESET} - Interactive dropdowns & resume {FG_240}│{RESET}");
                let c_l3  = format!("{FG_240}│{RESET} - Subagents & Plan Mode          {FG_240}│{RESET}");
                let c_l4  = format!("{FG_240}│{RESET} - /effort, /usage, /settings     {FG_240}│{RESET}");
                let c_bot = format!("{FG_240}╰─────────────────────────────────╯{RESET}");

                if w >= 78 {
                    let pad = |left: &str, target: usize| {
                        let vis = ansi_width(left);
                        let sp = target.saturating_sub(vis);
                        format!("{left}{}", " ".repeat(sp))
                    };
                    let col1_w = 34;
                    vec![
                        format!("{} {}", pad(&logo_l1, col1_w), c_top),
                        format!("{} {}", pad(&logo_l2, col1_w), c_l1),
                        format!("{} {}", pad(&logo_l3, col1_w), c_l2),
                        format!("{} {}", pad(&logo_l4, col1_w), c_l3),
                        format!("{} {}", pad(&logo_l5, col1_w), c_l4),
                        format!("{} {}", pad("", col1_w), c_bot),
                        format!("{FG_240}{}{RESET}", "─".repeat(w)),
                    ]
                } else {
                    vec![
                        logo_l3,
                        logo_l4,
                        format!("{FG_240}{}{RESET}", "─".repeat(w)),
                    ]
                }
            }
            Item::User(t) => {
                let mut out = Vec::new();
                for (i, l) in fit_lines(t, w).into_iter().enumerate() {
                    let mark = if i == 0 { glyphs::PROMPT_ARROW.trim_end() } else { " " };
                    out.push(format!("{CYAN}{mark} {l}{RESET}"));
                }
                out
            }
            Item::Tool { name, args, ok, denied, duration, output } => {
                if name == "terminate" || name == "ask_human" {
                    return Vec::new();
                }
                let (marker, border) = if *denied {
                    (format!("{RED}{}{RESET}", glyphs::BALLOT_X), RED)
                } else if *ok {
                    (format!("{GREEN}{}{RESET}", glyphs::CHECK_MARK), FG_245)
                } else {
                    (format!("{YELLOW}⠋{RESET}"), YELLOW)
                };
                let dur_str = match duration {
                    Some(d) => format!(" {DIM}({:.1}s){RESET}", d.as_secs_f32()),
                    None => String::new(),
                };
                let title = format!("{marker} {BOLD}{name}{RESET} {DIM}{a}{RESET}{dur_str}", a = pretty_args(name, args));
                let mut out = vec![title];
                if self.expanded && !output.is_empty() {
                    let mut olines: Vec<String> = Vec::new();
                    for ol in output.lines().take(24) {
                        olines.extend(fit_lines(ol, w.saturating_sub(4).max(10)));
                    }
                    if output.lines().count() > 24 {
                        olines.push("…".into());
                    }
                    for ol in olines {
                        out.push(format!("{border}  {} {FG_250}{ol}{RESET}", glyphs::TRACK_CHAR));
                    }
                    out.push(format!("{border}  └────{RESET}"));
                } else if !self.expanded && !output.is_empty() {
                    let extra = output.lines().count().saturating_sub(1);
                    if extra > 0 {
                        out.push(format!("{DIM}  └ <output +{extra} lines> (ctrl+o to expand){RESET}"));
                    } else {
                        out.push(format!("{DIM}  └ {} (ctrl+o to expand){RESET}", clip_chars(first_line(output), 70)));
                    }
                }
                out
            }
            Item::Answer(t) => {
                let clean = clean_model_text(t);
                let mut out = vec![format!("{CYAN}{}{RESET} {BOLD}onyx{RESET}", glyphs::DIAMOND_FILLED)];
                out.extend(render_markdown(&clean, w));
                out
            }
            Item::Info(t) => fit_lines(t, w)
                .into_iter()
                .map(|l| format!("{BLUE}ℹ{RESET} {FG_245}{l}{RESET}"))
                .collect(),
            Item::Error(t) => fit_lines(t, w)
                .into_iter()
                .map(|l| format!("{RED}✗{RESET} {RED}{l}{RESET}"))
                .collect(),
        }
    }

    fn status_lines_v(&self) -> Vec<String> {
        let mut out = Vec::new();
        let w = self.width.max(20);
        if let Some((tool, args, reason)) = self.ask.pending() {
            let risk = match tool.as_str() {
                "bash" | "shell" => format!("{RED}[HIGH RISK]{RESET}"),
                "editor" | "file_write" => format!("{YELLOW}[MODERATE RISK]{RESET}"),
                _ => format!("{CYAN}[STANDARD]{RESET}"),
            };
            out.push(frame_header(&format!("{YELLOW}⚠ APPROVAL REQUIRED{RESET} {risk}"), w, YELLOW));
            out.push(format!(
                "{YELLOW}│{RESET} {BOLD}{tool}{RESET} {DIM}{a}{RESET}",
                a = pretty_args(&tool, &args)
            ));
            if !reason.is_empty() {
                out.push(format!("{YELLOW}│{RESET} {DIM}reason: {reason}{RESET}"));
            }
            out.push(format!(
                "{YELLOW}└{RESET}  {BOLD}[y]{RESET} Approve once   {BOLD}[a]{RESET} Always approve   {BOLD}[n]{RESET} Deny"
            ));
        }
        out
    }

    fn input_lines(&self) -> Vec<String> {
        let mut out = Vec::new();
        let sug = self.suggestion();
        if !sug.is_empty() && self.input.starts_with('/') {
            // Suggestions can be long; keep the row inside the viewport so it
            // never soft-wraps (caret column math assumes one row per line).
            out.push(format!("{DIM}{sug}{RESET}"));
        }
        let body = if self.input.is_empty() && sug.is_empty() {
            format!("{DIM}Type your message or @path/to/file{RESET}")
        } else if self.cursor_chars() + 2 >= self.width || self.input.contains('\n') {
            // Long input scrolls a one-row window around the caret instead of
            // wrapping (a wrapped row would break the fixed "input line is
            // 2nd from bottom" rewind contract).
            let win = self.input_window();
            return vec![format!("{BOLD}{CYAN}*  {RESET}{win}")];
        } else {
            format!("{}", self.input)
        };
        out.push(format!("{BOLD}{CYAN}*  {RESET}{body}"));
        out
    }

    /// A width-fitting slice of `input` centred on the caret, with the caret
    /// landing at column `cursor_chars()` within the slice. Newlines (only
    /// reachable via alt+enter) render as '⏎' so the row stays one line.
    fn input_window(&self) -> String {
        let cc = self.cursor_chars();
        // Reserve: "❯ " prefix, ≥1 cell right of the caret, ellipsis margins.
        let avail = self.width.saturating_sub(6).max(8);
        let chars: Vec<char> = self.input.chars().collect();
        let start = cc.saturating_sub(avail - 1);
        let end = (start + avail).min(chars.len());
        let start = end.saturating_sub(avail);
        let mut s = String::new();
        if start > 0 {
            s.push('…');
        }
        for &c in &chars[start..end] {
            s.push(if c == '\n' { '⏎' } else { c });
        }
        s
    }

    fn cursor_chars(&self) -> usize {
        self.input[..self.cursor].chars().count()
    }

    fn matching_dropdown_items(&self) -> (String, Vec<(String, String, String)>) {
        let input = self.input.trim_start();
        if !input.starts_with('/') {
            return (String::new(), Vec::new());
        }

        // Subcommand dropdown for /resume or /res
        if input.starts_with("/resume") || input.starts_with("/res ") {
            let arg = if let Some((_, rest)) = input.split_once(' ') {
                rest.trim()
            } else {
                ""
            };
            let sessions = agent::session::list_sessions(20);
            let mut items = Vec::new();
            for s in sessions {
                let id_short = if s.id.len() > 16 {
                    format!("…{}", &s.id[s.id.len().saturating_sub(12)..])
                } else {
                    s.id.clone()
                };
                let title = if s.preview.is_empty() {
                    "(empty session)".to_string()
                } else {
                    s.preview.clone()
                };
                let desc = format!("{title} ({msg} msgs)", msg = s.messages);
                if arg.is_empty()
                    || s.id.to_lowercase().contains(&arg.to_lowercase())
                    || title.to_lowercase().contains(&arg.to_lowercase())
                {
                    items.push((s.id, id_short, desc));
                }
            }
            return (" Resume Session (↑/↓ to choose, tab/enter to resume) ".into(), items);
        }

        // Subcommand dropdown for /model or /models
        if input.starts_with("/model") || input.starts_with("/models") {
            let arg = if let Some((_, rest)) = input.split_once(' ') {
                rest.trim()
            } else {
                ""
            };
            let mut models = self.cached_models.clone();
            if models.is_empty() {
                models = vec![
                    "gpt-oss-120b-medium".into(),
                    "claude-opus-4-6-thinking".into(),
                    "claude-sonnet-4-6".into(),
                    "gemini-3.1-flash-lite".into(),
                    "gemini-3.5-flash-lite".into(),
                ];
            }
            let mut items = Vec::new();
            for m in models {
                let active = if m == self.hm.model { " [active]" } else { "" };
                let desc = format!("model{active}");
                if arg.is_empty() || m.to_lowercase().contains(&arg.to_lowercase()) {
                    items.push((m.clone(), m, desc));
                }
            }
            return (" Switch Model (↑/↓ to choose, tab/enter to select) ".into(), items);
        }

        // Subcommand dropdown for /effort
        if input.starts_with("/effort") {
            let arg = if let Some((_, rest)) = input.split_once(' ') {
                rest.trim()
            } else {
                ""
            };
            let levels = vec![
                ("low", "Low reasoning effort for faster turns"),
                ("medium", "Balanced reasoning and latency"),
                ("high", "High reasoning effort for complex engineering tasks"),
                ("max", "Maximum reasoning budget allowed by model"),
                ("none", "Disable reasoning effort entirely"),
            ];
            let mut items = Vec::new();
            for (lvl, desc) in levels {
                let active = if lvl == self.hm.effort { " [active]" } else { "" };
                let full_desc = format!("{desc}{active}");
                if arg.is_empty() || lvl.starts_with(&arg.to_lowercase()) {
                    items.push((lvl.to_string(), lvl.to_string(), full_desc));
                }
            }
            return (" Reasoning Effort (↑/↓ to choose, tab/enter to select) ".into(), items);
        }

        // Subcommand dropdown for /skill
        if input.starts_with("/skill") {
            let arg = if let Some((_, rest)) = input.split_once(' ') {
                rest.trim()
            } else {
                ""
            };
            let mut items = Vec::new();
            if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
                let mut cat = skills::SkillCatalog::new();
                cat.scan_dir(&home.join(".onyx/skills"));
                for (_, sk) in &cat.skills {
                    if arg.is_empty() || sk.name.to_lowercase().contains(&arg.to_lowercase()) {
                        items.push((sk.name.clone(), sk.name.clone(), sk.description.clone()));
                    }
                }
            }
            return (" Load Skill (↑/↓ to choose, tab/enter to select) ".into(), items);
        }

        // Top-level slash commands
        if !input.contains(' ') {
            let lower = input.to_lowercase();
            let items = SLASH_COMMANDS
                .iter()
                .filter(|(c, _)| c.to_lowercase().starts_with(&lower))
                .map(|(c, d)| (c.to_string(), c.to_string(), d.to_string()))
                .collect();
            return (" Commands (↑/↓ to select, tab/enter to apply) ".into(), items);
        }

        (String::new(), Vec::new())
    }

    /// Renders an interactive slash command popup card above the input composer
    /// modelled after Grok Build's slash dropdown.
    fn slash_dropdown_lines(&self) -> Vec<String> {
        let (title, matches) = self.matching_dropdown_items();
        if matches.is_empty() {
            return Vec::new();
        }

        let w = self.width.max(24);
        let inner_w = w.saturating_sub(4).max(18);

        // Max visible items in dropdown popup
        let max_visible = 6;
        let total = matches.len();
        let selected = self.slash_selected.min(total.saturating_sub(1));

        // Windowing around selected item
        let start_idx = if total <= max_visible {
            0
        } else if selected < max_visible / 2 {
            0
        } else if selected + (max_visible / 2) >= total {
            total.saturating_sub(max_visible)
        } else {
            selected.saturating_sub(max_visible / 2)
        };
        let end_idx = (start_idx + max_visible).min(total);

        let mut out = Vec::new();
        // Top border with title & key hint
        let right_pad = inner_w.saturating_sub(title.chars().count() + 2);
        out.push(format!(
            "{FG_240}╭─{RESET}{BOLD}{CYAN}{title}{RESET}{FG_240}{}{RESET}",
            "─".repeat(right_pad.max(2))
        ));

        let cmd_col_w = if title.contains("Resume") {
            16
        } else if title.contains("Model") {
            26
        } else {
            14
        };

        for (idx, (_full, display_col, desc)) in matches[start_idx..end_idx].iter().enumerate() {
            let actual_idx = start_idx + idx;
            let is_sel = actual_idx == selected;

            let prefix = if is_sel {
                format!("{CYAN}{}{RESET}", glyphs::PROMPT_ARROW)
            } else {
                "  ".to_string()
            };

            let cmd_formatted = if is_sel {
                format!("{BOLD}{CYAN}{:<width$}{RESET}", display_col, width = cmd_col_w)
            } else {
                format!("{FG_250}{:<width$}{RESET}", display_col, width = cmd_col_w)
            };

            let desc_w = inner_w.saturating_sub(cmd_col_w + 3).max(8);
            let clipped_desc = clip_chars(desc, desc_w);

            let desc_formatted = if is_sel {
                format!("{BOLD}{FG_250}{:<width$}{RESET}", clipped_desc, width = desc_w)
            } else {
                format!("{DIM}{:<width$}{RESET}", clipped_desc, width = desc_w)
            };

            let row_content = format!("{prefix}{cmd_formatted} {desc_formatted}");
            let vis_len = ansi_width(&row_content);
            let pad = inner_w.saturating_sub(vis_len);

            if is_sel {
                out.push(format!(
                    "{FG_240}│{RESET} \x1b[48;5;236m{row_content}{}\x1b[0m {FG_240}│{RESET}",
                    " ".repeat(pad)
                ));
            } else {
                out.push(format!(
                    "{FG_240}│{RESET} {row_content}{} {FG_240}│{RESET}",
                    " ".repeat(pad)
                ));
            }
        }

        // Bottom border
        out.push(format!("{FG_240}╰{RESET}{}{FG_240}╯{RESET}", "─".repeat(inner_w + 2)));
        out
    }

    /// Tab-completion hint: the first slash command matching what's typed, with
    /// its description dimmed. Plain text (no ANSI) so Tab can accept it directly.
    fn suggestion(&self) -> String {
        if !self.input.starts_with('/') || self.input.contains(' ') {
            return String::new();
        }
        let lower = self.input.to_lowercase();
        SLASH_COMMANDS
            .iter()
            .find(|(c, _)| c.to_lowercase().starts_with(&lower) && *c != lower)
            .map(|(c, d)| format!("{c}  {d}"))
            .unwrap_or_default()
    }

    fn hints(&self) -> String {
        let is_auto = matches!(*self.gw.mode.lock().unwrap(), ApprovalMode::Auto);
        let mode_label = if is_auto {
            format!("{RED}{BOLD}YOLO{RESET} {DIM}Auto Mode active{RESET}")
        } else {
            format!("{YELLOW}Shift+Tab to Auto Mode{RESET}")
        };

        // Context budget and actual usage
        let max_ctx = if self.hm.context_budget > 0 { self.hm.context_budget } else { 128_000 };
        let used_tokens = if self.real_total_tokens > 0 {
            self.real_total_tokens
        } else {
            (self.history_bytes / 4).max(self.history_len * 40)
        };
        let pct = ((used_tokens as f64 / max_ctx as f64) * 100.0).clamp(0.0, 100.0) as usize;
        let filled_bars = (pct / 10).min(10);
        let empty_bars = 10 - filled_bars;
        let ctx_bar = format!("ctx {CYAN}{}{RESET}{DIM}{}{RESET} {:>2}%", "▓".repeat(filled_bars), "░".repeat(empty_bars), pct);

        let sep = format!("{FG_240}{}{RESET}", "─".repeat(self.width.max(20)));
        let mcp_label = if self.mcp_count == 1 { "1 MCP server" } else { &format!("{} MCP servers", self.mcp_count) };
        let skill_label = if self.skill_count == 1 { "1 skill" } else { &format!("{} skills", self.skill_count) };
        let line1 = format!("{mode_label}  {DIM}·  {mcp_label} · {skill_label}{RESET}");
        let effort_str = if self.hm.effort.is_empty() {
            "Disabled Effort"
        } else {
            &format!("{} Effort", capitalize(&self.hm.effort))
        };
        let ctx_k = max_ctx / 1000;
        let line2 = format!(
            "{DIM}{model} · {effort_str} · {ctx_k}K context · {ctx_bar}{RESET}",
            model = self.hm.model
        );

        format!("{sep}\n{line1}\n{sep}\n{line2}")
    }

    // ------------------------------------------------------------ turn flow ----

    fn submit(&mut self) {
        let text = self.input.clone();
        self.input.clear();
        self.cursor = 0;
        self.scroll_offset = 0;
        if text.is_empty() {
            return;
        }
        if text.starts_with('/') {
            self.items.push(Item::User(text.clone()));
            self.slash(&text);
            return;
        }
        if self.running {
            self.items.push(Item::Error("a turn is already running — esc to cancel it first".into()));
            return;
        }
        self.items.push(Item::User(text.clone()));
        self.tail.clear();
        self.tail_scrolled = 0;
        self.thinking = true;
        self.running = true;
        let ms = self.opts.max_steps;
        if let Some(tx) = &self.ctl_tx {
            let _ = tx.send(Ctl::Prompt(text, ms));
        }
    }

    fn slash(&mut self, line: &str) {
        let mut it = line.split_whitespace();
        let cmd = it.next().unwrap_or("");
        let arg = it.next();
        // history/hm live on the worker between turns: route every mutation
        // through Ctl so we never touch them while a turn is running.
        match cmd {
            "/exit" | "/quit" => self.quit = true,
            "/clear" => {
                if let Some(tx) = &self.ctl_tx {
                    let _ = tx.send(Ctl::Clear(self.hm.system.clone()));
                }
                self.items.push(Item::Info("conversation cleared".into()));
            }
            "/model" | "/models" => {
                if let Some(m) = arg {
                    if let Some(tx) = &self.ctl_tx {
                        let _ = tx.send(Ctl::SetModel(m.to_string()));
                    }
                    self.opts.model = Some(m.to_string());
                } else {
                    self.items.push(Item::Info(format!("model: {}", self.hm.model)));
                }
            }
            "/steps" => {
                if let Some(n) = arg.and_then(|v| v.parse().ok()) {
                    self.opts.max_steps = n;
                }
                self.items.push(Item::Info(format!("max steps: {}", self.opts.max_steps)));
            }
            "/usage" => {
                let calls = self.gw.audit.snapshot().len();
                if self.real_total_tokens > 0 {
                    self.items.push(Item::Info(format!(
                        "{} prompt tokens (cached: {}) · {} completion tokens · {} total tokens · {} message(s) · {calls} tool call(s)",
                        self.real_prompt_tokens,
                        self.real_cached_tokens,
                        self.real_completion_tokens,
                        self.real_total_tokens,
                        self.history_len
                    )));
                } else {
                    let est = (self.history_bytes / 4).max(self.history_len * 40);
                    self.items.push(Item::Info(format!(
                        "~{est} tokens estimated in context · {} message(s) · {calls} tool call(s)",
                        self.history_len
                    )));
                }
            }
            "/effort" => {
                if let Some(eff) = arg {
                    let eff_clean = eff.to_lowercase();
                    if let Some(tx) = &self.ctl_tx {
                        let _ = tx.send(Ctl::SetEffort(eff_clean.clone()));
                    }
                    self.hm.effort = eff_clean.clone();
                    self.items.push(Item::Info(format!("reasoning effort set to: {eff_clean}")));
                } else {
                    self.items.push(Item::Info(format!("current reasoning effort: {} (options: low, medium, high, max, none)", self.hm.effort)));
                }
            }
            "/settings" => {
                let auto = matches!(*self.gw.mode.lock().unwrap(), ApprovalMode::Auto);
                self.items.push(Item::Info("╭── ONYX SETTINGS ──────────────────────────────────────╮".into()));
                self.items.push(Item::Info(format!("│ Model:            {:<36} │", self.hm.model)));
                self.items.push(Item::Info(format!("│ Reasoning Effort: {:<36} │", self.hm.effort)));
                self.items.push(Item::Info(format!("│ Mode:             {:<36} │", if auto { "YOLO (Auto-approve all)" } else { "Interactive Approval" })));
                self.items.push(Item::Info(format!("│ Context Budget:   {:<36} │", format!("{} tokens", self.hm.context_budget))));
                self.items.push(Item::Info(format!("│ MCP Servers:      {:<36} │", format!("{} active", self.mcp_count))));
                self.items.push(Item::Info(format!("│ Installed Skills: {:<36} │", format!("{} available", self.skill_count))));
                self.items.push(Item::Info(format!("│ Max Steps:        {:<36} │", self.opts.max_steps)));
                self.items.push(Item::Info("╰───────────────────────────────────────────────────────╯".into()));
            }
            "/audit" => {
                if let Some(tx) = &self.ctl_tx {
                    let _ = tx.send(Ctl::Audit);
                }
                self.items.push(Item::Info("audit requested (see transcript below)".into()));
            }
            "/tools" => {
                for n in self.gw.tools.names() {
                    let cap = tools::taxonomy::capability_of(&n).label();
                    self.items.push(Item::Info(format!("  • {n}  {DIM}[{cap}]{RESET}")));
                }
            }
            "/mode" => {
                self.cycle_mode();
            }
            "/skill" => {
                if let Some(name) = arg {
                    self.load_skill(name);
                } else {
                    self.items.push(Item::Info("usage: /skill <name>".into()));
                }
            }
            "/sessions" => {
                let list = agent::session::list_sessions(12);
                if list.is_empty() {
                    self.items.push(Item::Info("no saved sessions yet".into()));
                }
                for s in list {
                    let pv = if s.preview.is_empty() { "(no prompt)".to_string() } else { s.preview };
                    self.items.push(Item::Info(format!(
                        "  {} · {} msg · {}",
                        s.id,
                        s.messages,
                        pv
                    )));
                }
                self.items.push(Item::Info("/resume <id|prefix> to replay one".into()));
            }
            "/res" | "/resume" => {
                match arg {
                    None => self.items.push(Item::Info("usage: /resume <id|prefix|suffix>  (see /sessions)".into())),
                    Some(q) => match agent::session::resolve_session(q) {
                        None => self.items.push(Item::Error(format!("no session matches `{q}` (try /sessions)"))),
                        Some((path, msgs)) => {
                            // Replay is a fork: the resumed transcript becomes a NEW
                            // session file so the original stays untouched. Tell the
                            // worker to drop its handle before installing ours, so it
                            // can't append to the sealed file after this point.
                            if let Some(tx) = &self.ctl_tx {
                                let _ = tx.send(Ctl::DropTranscript);
                            }
                            let sid = agent::session::new_session_id();
                            let transcript = agent::session::Transcript::open(&sid, &self.hm.model);
                            if let Some(t) = &transcript {
                                for m in &msgs {
                                    t.push(m);
                                }
                            }
                            self.session = transcript;
                            if let Some(tx) = &self.ctl_tx {
                                let _ = tx.send(Ctl::Resume(msgs.clone()));
                            }
                            // Clear previous local items except banner before replaying resumed session
                            self.items.retain(|it| matches!(it, Item::Banner));
                            // Mirror the replayed conversation into the visible scrollback.
                            for m in &msgs {
                                match m.role.as_str() {
                                    "user" => self.items.push(Item::User(m.content.clone())),
                                    "assistant" => {
                                        if let Some(tc) = &m.tool_call {
                                            if tc.name != "terminate" && tc.name != "ask_human" {
                                                self.items.push(Item::Tool {
                                                    name: tc.name.clone(),
                                                    args: tc.args.to_string(),
                                                    ok: true,
                                                    denied: false,
                                                    duration: None,
                                                    output: String::new(),
                                                });
                                            }
                                        } else if !m.content.is_empty() {
                                            let clean = clean_model_text(&m.content);
                                            if !clean.trim().is_empty() {
                                                self.items.push(Item::Answer(clean));
                                            }
                                        }
                                    }
                                    "tool" => {
                                        if let Some(Item::Tool { output, .. }) = self.items.last_mut() {
                                            if output.is_empty() {
                                                *output = m.content.clone();
                                            }
                                        }
                                    }
                                    _ => {}
                                }
                            }
                            self.items.push(Item::Info(format!(
                                "resumed {} message(s) from {} · new session {sid}",
                                msgs.len(),
                                path.file_stem().and_then(|s| s.to_str()).unwrap_or("?")
                            )));
                        }
                    },
                }
            }
            "/help" => {
                self.items.push(Item::Info(
                    "commands: /model /steps /clear /usage /audit /tools /skill /sessions /resume /mode /exit".into(),
                ));
                self.items.push(Item::Info(
                    "keys: enter send · alt+enter newline · shift+tab mode · ctrl+o expand · esc cancel · ctrl+d×2 exit".into(),
                ));
            }
            other => self.items.push(Item::Error(format!("unknown command `{other}` (try /help)"))),
        }
    }

    fn cycle_mode(&mut self) {
        let next = self.gw.mode.lock().unwrap().next();
        *self.gw.mode.lock().unwrap() = next;
        let msg = match next {
            ApprovalMode::Normal => "normal mode: policy decides, asks confirmed interactively",
            ApprovalMode::AcceptEdits => "accept-edits: file edits auto-approved (shift+tab to cycle)",
            ApprovalMode::Auto => "auto-approve ALL actions — unattended execution armed",
        };
        self.items.push(Item::Info(format!("⟳ {msg}")));
    }

    fn load_skill(&mut self, name: &str) {
        let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
            self.items.push(Item::Error("no HOME".into()));
            return;
        };
        let mut catalog = skills::SkillCatalog::new();
        catalog.scan_dir(&home.join(".onyx/skills"));
        if let Some(sk) = catalog.get(name) {
            let add = format!("\n\n[Active Skill: {}]\n{}\n{}", sk.name, sk.description, sk.instructions);
            if let Some(tx) = &self.ctl_tx {
                let _ = tx.send(Ctl::SystemAppend(add));
            }
            self.items.push(Item::Info(format!("loaded skill: {}", sk.name)));
        } else {
            self.items.push(Item::Error(format!("skill `{name}` not found in ~/.onyx/skills")));
        }
    }

    fn on_event(&mut self, ev: AgentEvent) {
        match ev {
            AgentEvent::Thinking { .. } => {
                self.tail.clear();
                self.tail_scrolled = 0;
                self.thinking = true;
            }
            AgentEvent::TextDelta(d) => {
                self.thinking = false;
                self.tail.push_str(&d);
                let n = self.tail.chars().count();
                if n > 1400 {
                    self.tail_scrolled = n - 1200;
                }
            }
            AgentEvent::ToolStart { name, args } => {
                if name == "terminate" || name == "ask_human" {
                    // Do not print internal control calls as noisy execution cards
                    return;
                }
                self.finalize_tail();
                self.thinking = false; // a card or the tool row is what's live now
                self.active_tool_start = Some(std::time::Instant::now());
                self.items.push(Item::Tool {
                    name,
                    args: args.to_string(),
                    ok: false,
                    denied: false,
                    duration: None,
                    output: String::new(),
                });
            }
            AgentEvent::ToolDone { name, ok, output } => {
                if name == "terminate" || name == "ask_human" {
                    return;
                }
                let dur = self.active_tool_start.take().map(|t| t.elapsed());
                let denied = output.starts_with("denied:") || output.contains("confirmation refused");
                for it in self.items.iter_mut().rev() {
                    if let Item::Tool { name: n, ok: o, denied: d, duration: du, output: out, .. } = it {
                        if n == &name && out.is_empty() {
                            *o = ok;
                            *d = denied;
                            *du = dur;
                            *out = output;
                            break;
                        }
                    }
                }
                self.thinking = true;
            }
            AgentEvent::Usage(u) => {
                self.real_prompt_tokens = u.prompt_tokens;
                self.real_completion_tokens = u.completion_tokens;
                self.real_total_tokens = u.total_tokens;
                self.real_cached_tokens = u.cached_prompt_tokens;
            }
        }
    }

    fn finalize_tail(&mut self) {
        if !self.tail.trim().is_empty() {
            let t = std::mem::take(&mut self.tail);
            let clean = clean_model_text(&t);
            if !clean.trim().is_empty() {
                self.items.push(Item::Answer(clean.trim().to_string()));
            }
        }
        self.tail_scrolled = 0;
    }

    fn on_turn_done(&mut self, res: Result<(String, usize), AgentError>) {
        self.finalize_tail();
        self.running = false;
        self.thinking = false;
        match res {
            Ok((answer, _steps)) => {
                let clean = clean_model_text(&answer);
                let dup = self.items.iter().rev().any(|i| matches!(i, Item::Answer(t) if t == &clean));
                if !clean.trim().is_empty() && !dup {
                    self.items.push(Item::Answer(clean));
                }
            }
            Err(AgentError::MaxSteps) => {
                self.items.push(Item::Error(format!(
                    "reached max steps ({}) without an answer",
                    self.opts.max_steps
                )));
            }
            Err(AgentError::Tool(e)) => {
                self.items.push(Item::Error(format!("tool failed at the gateway: {e}")));
            }
        }
    }

    // --------------------------------------------------------------- the loop ----

    fn run(&mut self, pump: &InputPump) {
        self.draw();
        loop {
            // Drain finished work/events first so drawing reflects them.
            let mut drained: Vec<UiOut> = Vec::new();
            if let Some(rx) = &self.ui_rx {
                while let Ok(msg) = rx.try_recv() {
                    drained.push(msg);
                }
            }
            let mut event_seen = false;
            for msg in drained {
                match msg {
                    UiOut::Ev(ev) => self.on_event(ev),
                    UiOut::Info(s) => self.items.push(Item::Info(s)),
                    UiOut::TurnDone(res) => self.on_turn_done(res),
                    UiOut::Audit(entries) => {
                        for a in entries {
                            self.items.push(Item::Info(format!(
                                "{:<14} {:<12} {}",
                                a.tool,
                                a.outcome,
                                clip_chars(first_line(&a.args), 60)
                            )));
                        }
                    }
                    UiOut::ModelName(m) => {
                        self.items.push(Item::Info(format!("model: {m}")));
                    }
                    UiOut::History(len, bytes) => {
                        self.history_len = len;
                        self.history_bytes = bytes;
                    }
                }
                event_seen = true;
            }
            if event_seen {
                self.draw();
            }

            let poll = if self.thinking || self.running {
                Duration::from_millis(120)
            } else {
                Duration::from_millis(80)
            };
            let Some((chars, code)) = pump.next_key(poll) else {
                if self.thinking {
                    self.tick += 1;
                    self.draw();
                }
                continue;
            };

            // Approval card swallows most keys while pending.
            if self.ask.pending().is_some() {
                for &ch in &chars {
                    match ch {
                        'y' | 'Y' => self.ask.answer(true, false),
                        'a' => self.ask.answer(true, true),
                        'n' | 'N' => self.ask.answer(false, false),
                        _ => {}
                    }
                }
                self.draw();
                continue;
            }

            // Special keys arrive alone; printable runs carry several chars.
            let first = chars[0];
            if chars.len() == 1 || code != 0 {
                match (first, code) {
                    ('\x04', _) => {
                        if self.last_ctrl_d.map_or(false, |t| t.elapsed() < Duration::from_secs(2)) {
                            self.quit = true;
                        } else {
                            self.last_ctrl_d = Some(std::time::Instant::now());
                            self.items.push(Item::Info("(press ctrl+d again to exit)".into()));
                        }
                    }
                    ('\x03', _) => {
                        if self.running {
                            *self.cancel.lock().unwrap() = true;
                            self.items.push(Item::Info("esc pressed — cancelling after the current step…".into()));
                        } else {
                            self.items.push(Item::Info("(ctrl+d ×2 or /exit to quit)".into()));
                        }
                    }
                    ('\x1b', _) => {
                        if self.running {
                            *self.cancel.lock().unwrap() = true;
                            self.items.push(Item::Info("cancelling after the current step…".into()));
                        } else if self.input.starts_with('/') {
                            // Dismiss slash command popup and clear input
                            self.input.clear();
                            self.cursor = 0;
                            self.slash_selected = 0;
                        }
                    }
                    ('\x0f', _) => {
                        self.expanded = !self.expanded;
                    }
                    ('\x0c', _) => {
                        self.items.clear();
                        self.items.push(Item::Banner);
                    }
                    ('\0', 10) => self.cycle_mode(), // shift+tab
                    ('\t', _) => {
                        let (title, matches) = self.matching_dropdown_items();
                        if !matches.is_empty() {
                            let idx = self.slash_selected.min(matches.len() - 1);
                            let target = &matches[idx].0;
                            if title.contains("Resume") {
                                self.input = format!("/resume {target}");
                            } else if title.contains("Model") {
                                self.input = format!("/model {target}");
                            } else if title.contains("Effort") {
                                self.input = format!("/effort {target}");
                            } else if title.contains("Skill") {
                                self.input = format!("/skill {target}");
                            } else {
                                self.input = format!("{target} ");
                            }
                            self.cursor = self.input.len();
                            self.slash_selected = 0;
                        } else {
                            let sug = self.suggestion();
                            if !sug.is_empty() {
                                let clean: String = sug.chars().take_while(|c| *c != ' ').collect();
                                self.input = format!("{clean} ");
                                self.cursor = self.input.len();
                            }
                        }
                    }
                    ('\r', _) => {
                        let (title, matches) = self.matching_dropdown_items();
                        if !matches.is_empty() {
                            let idx = self.slash_selected.min(matches.len() - 1);
                            let target = &matches[idx].0;
                            if title.contains("Resume") {
                                // Resume immediately!
                                self.input = format!("/resume {target}");
                                self.cursor = self.input.len();
                                self.slash_selected = 0;
                                self.submit();
                            } else if title.contains("Model") {
                                // Switch model immediately!
                                self.input = format!("/model {target}");
                                self.cursor = self.input.len();
                                self.slash_selected = 0;
                                self.submit();
                            } else if title.contains("Effort") {
                                // Set effort immediately!
                                self.input = format!("/effort {target}");
                                self.cursor = self.input.len();
                                self.slash_selected = 0;
                                self.submit();
                            } else if title.contains("Skill") {
                                // Load skill immediately!
                                self.input = format!("/skill {target}");
                                self.cursor = self.input.len();
                                self.slash_selected = 0;
                                self.submit();
                            } else if !self.input.contains(' ') && self.input != *target {
                                let cmd = target;
                                if cmd == "/clear" || cmd == "/tools" || cmd == "/exit" || cmd == "/mode" || cmd == "/help" || cmd == "/usage" || cmd == "/audit" || cmd == "/sessions" || cmd == "/settings" {
                                    self.input = cmd.to_string();
                                    self.cursor = self.input.len();
                                    self.slash_selected = 0;
                                    self.submit();
                                } else {
                                    self.input = format!("{cmd} ");
                                    self.cursor = self.input.len();
                                    self.slash_selected = 0;
                                }
                            } else {
                                self.submit();
                            }
                        } else {
                            self.submit();
                        }
                    }
                    ('\n', 1) => {
                        // alt+enter inserts a newline
                        self.input.insert(self.cursor, '\n');
                        self.cursor += 1;
                    }
                    ('\u{7f}', _) => {
                        if self.cursor > 0 {
                            let prev = self.input[..self.cursor].chars().next_back().map(|c| c.len_utf8()).unwrap_or(0);
                            self.cursor -= prev;
                            self.input.remove(self.cursor);
                            self.slash_selected = 0;
                        }
                    }
                    ('\0', 1004) => {
                        // delete key
                        if self.cursor < self.input.len() {
                            let next = self.input[self.cursor..].chars().next().map(|c| c.len_utf8()).unwrap_or(0);
                            self.input.replace_range(self.cursor..self.cursor + next, "");
                            self.slash_selected = 0;
                        }
                    }
                    ('\0', 1008) => {
                        // Page Up: scroll conversation back
                        self.scroll_offset = self.scroll_offset.saturating_add(5);
                    }
                    ('\0', 1009) => {
                        // Page Down: scroll conversation forward towards live
                        self.scroll_offset = self.scroll_offset.saturating_sub(5);
                    }
                    ('\0', 1000) => {
                        // up arrow: navigate dropdown if active, else scroll conversation
                        let (_, matches) = self.matching_dropdown_items();
                        if !matches.is_empty() {
                            let total = matches.len();
                            self.slash_selected = (self.slash_selected + total - 1) % total;
                        } else if !self.input.contains('\n') && self.input.is_empty() {
                            self.scroll_offset = self.scroll_offset.saturating_add(1);
                        }
                    }
                    ('\0', 1001) => {
                        // down arrow: navigate dropdown if active, else scroll conversation
                        let (_, matches) = self.matching_dropdown_items();
                        if !matches.is_empty() {
                            let total = matches.len();
                            self.slash_selected = (self.slash_selected + 1) % total;
                        } else if !self.input.contains('\n') && self.input.is_empty() {
                            self.scroll_offset = self.scroll_offset.saturating_sub(1);
                        }
                    }
                    ('\0', 1002) => {
                        if self.cursor < self.input.len() {
                            let next = self.input[self.cursor..].chars().next().map(|c| c.len_utf8()).unwrap_or(0);
                            self.cursor += next;
                        }
                    }
                    ('\0', 1003) => {
                        if self.cursor > 0 {
                            let prev = self.input[..self.cursor].chars().next_back().map(|c| c.len_utf8()).unwrap_or(0);
                            self.cursor -= prev;
                        }
                    }
                    ('\0', 1006) => self.cursor = 0,
                    ('\0', 1007) => self.cursor = self.input.len(),
                    (c, 0) if !c.is_control() => {
                        self.scroll_offset = 0; // jump back to bottom when user begins typing
                        self.input.insert(self.cursor, c);
                        self.cursor += c.len_utf8();
                        self.slash_selected = 0;
                        self.last_ctrl_d = None;
                    }
                    _ => {}
                }
            } else {
                self.scroll_offset = 0; // jump back to bottom on paste/multi-character input
                for &c in &chars {
                    self.input.insert(self.cursor, c);
                    self.cursor += c.len_utf8();
                }
                self.slash_selected = 0;
                self.last_ctrl_d = None;
            }
            if self.quit {
                break;
            }
            self.draw();
        }
    }
}

fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for word in text.split_whitespace() {
        if cur.chars().count() + word.chars().count() + 1 > width && !cur.is_empty() {
            out.push(std::mem::take(&mut cur));
        }
        if !cur.is_empty() {
            cur.push(' ');
        }
        cur.push_str(word);
    }
    if !cur.is_empty() || out.is_empty() {
        out.push(cur);
    }
    out
}

pub fn tui_session(
    gw: &mut Gateway<dyn tools::policy::Policy>,
    hm: &mut HttpModel,
    opts: &mut Opts,
) {
    let _raw = match RawMode::enter() {
        Some(r) => r,
        None => {
            eprintln!("onyx: cannot enter raw mode — stdin is not a tty?");
            return;
        }
    };
    let pump = InputPump::start();
    // Switch to alternate screen buffer, clear screen, and hide cursor while painting
    write!(io::stdout(), "\x1b[?1049h\x1b[H\x1b[2J\x1b[?25l").ok();
    io::stdout().flush().ok();
    {
        let mut tui = Tui::new(gw, hm, opts);
        tui.start_worker();
        tui.run(&pump);
        tui.finish();
    }
    pump.shutdown();
    // Restore cursor and switch back to normal screen buffer
    write!(io::stdout(), "\x1b[?25h\x1b[?1049l").ok();
    io::stdout().flush().ok();
    // Final newline so the shell prompt starts clean.
    println!();
}
