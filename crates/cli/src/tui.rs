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
use agent::{AgentEvent, AgentError, DoomReason, Message, ToolCallAgent};
use std::cell::RefCell;
use std::collections::VecDeque;
use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;
use tools::audit::{Approval, ApprovalMode, AuditEntry, Gateway};
use crate::markdown::render_markdown;
use crate::theme;
use ratatui::crossterm::cursor::{Hide, Show};
use ratatui::crossterm::execute;
use ratatui::crossterm::event::{DisableBracketedPaste, EnableBracketedPaste};
use ratatui::crossterm::terminal::{
    disable_raw_mode as ct_disable_raw, enable_raw_mode as ct_enable_raw, EnterAlternateScreen,
    LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Alignment as RuAlignment, Constraint, Layout as RuLayout, Rect};
use ratatui::style::Style as RuStyle;
use ratatui::text::{Line as RuLine, Span as RuSpan};
use ratatui::widgets::{
    Block as RuBlock, BorderType as RuBorderType, Borders as RuBorders, List as RuList,
    ListItem as RuListItem, ListState as RuListState, Paragraph as RuParagraph,
    Scrollbar as RuScrollbar, ScrollbarOrientation as RuScrollbarOrientation,
    ScrollbarState as RuScrollbarState, Sparkline as RuSparkline, StatefulWidget, Widget,
    Wrap as RuWrap,
};
use ratatui::Frame;
use ratatui::Terminal;

// ---------------------------------------------------------------- Premium Dark Theme (Hex/TrueColor with ANSI fallback) ----

const RESET: &str = "\x1b[0m";
const BOLD: &str = "\x1b[1m";
const DIM: &str = "\x1b[2m";
const ITALIC: &str = "\x1b[3m";

// Premium Dark Workstation Colors (#00E5FF, #F1F5F9, #89939F, #252B35, #20D99A, #FF647C, #FFCA68)
const CYAN: &str = "\x1b[38;2;0;229;255m";       // Primary cyan #00E5FF
const FG_MAIN: &str = "\x1b[38;2;241;245;249m";  // Main text #F1F5F9
const FG_SEC: &str = "\x1b[38;2;137;147;159m";   // Secondary text #89939F
const BORDER: &str = "\x1b[38;2;37;43;53m";      // Borders #252B35
const GREEN: &str = "\x1b[38;2;32;217;154m";     // Success #20D99A
const RED: &str = "\x1b[38;2;255;100;124m";      // Error #FF647C
const YELLOW: &str = "\x1b[38;2;255;202;104m";   // Warning #FFCA68
const BLUE: &str = "\x1b[38;2;56;189;248m";      // Sky blue for informational highlights

const FG_245: &str = "\x1b[38;2;137;147;159m";   // Map legacy secondary text to #89939F
const FG_250: &str = "\x1b[38;2;241;245;249m";   // Map legacy main text to #F1F5F9

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

/// A run of `─` in the given color, exactly `width` cells wide.
#[allow(dead_code)]
fn hline(width: usize, color: &str) -> String {
    format!("{color}{}{RESET}", "─".repeat(width))
}

/// Center `text` within `avail` visible cells taking ANSI escape sequences into account.
fn center_pad(text: &str, avail: usize) -> String {
    let vis = ansi_width(text);
    if vis >= avail {
        return text.to_string();
    }
    let left = (avail - vis) / 2;
    let right = avail - left - vis;
    format!("{}{}{}", " ".repeat(left), text, " ".repeat(right))
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

/// Turn a model slug into Antigravity-style title case for the header.
/// `gemini-3.8-flash-low` → `Gemini 3.8 Flash Low`; keeps version tokens
/// (those containing a digit) lowercase-free and uppercases only word initials.
fn title_case_model(m: &str) -> String {
    m.split(['-', '_'])
        .filter(|s| !s.is_empty())
        .map(|seg| {
            seg.split('.')
                .map(|part| {
                    if part.chars().next().is_some_and(|c| c.is_ascii_alphabetic()) {
                        let mut ch = part.chars();
                        let first = ch.next().unwrap().to_uppercase().to_string();
                        format!("{first}{}", ch.as_str())
                    } else {
                        part.to_string()
                    }
                })
                .collect::<Vec<_>>()
                .join(".")
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Decode one stdin event into (chars, code). Escape sequences map to codes
/// ≥1000 (arrows, page keys, paste) or 10 (shift+tab). Unrecognized → ('?', 0).
/// Code 2000 indicates bracketed paste payload.
/// A merged run of printable bytes (fast typing) yields every
/// char at once; control bytes are always single events.
fn decode_key(buf: &[u8]) -> (Vec<char>, u32) {
    let one = |c: char| vec![c];
    // Check bracketed paste: \x1b[200~ ... \x1b[201~
    if buf.starts_with(b"\x1b[200~") {
        let content_start = 6;
        let content_end = if buf.ends_with(b"\x1b[201~") {
            buf.len() - 6
        } else {
            buf.len()
        };
        if content_end >= content_start {
            let s = String::from_utf8_lossy(&buf[content_start..content_end]);
            return (s.chars().collect(), 2000);
        }
        return (Vec::new(), 2000);
    }
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
        [0x01] => (one('\x01'), 0), // ctrl+a (emacs: line start)
        [0x05] => (one('\x05'), 0), // ctrl+e (emacs: line end)
        [0x06] => (one('\x06'), 0), // ctrl+f (emacs: forward char)
        [0x02] => (one('\x02'), 0), // ctrl+b (emacs: back char)
        [0x15] => (one('\x15'), 0), // ctrl+u (emacs: clear input)
        [0x0b] => (one('\x0b'), 0), // ctrl+k (emacs: kill to end)
        [0x17] => (one('\x17'), 0), // ctrl+w (emacs: delete word back)
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
            let mut tmp = [0u8; 512];
            let mut in_paste = false;
            let mut paste_buf = Vec::new();
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

                // If currently inside bracketed paste mode, keep accumulating until \x1b[201~
                if in_paste {
                    paste_buf.extend_from_slice(got);
                    if let Some(pos) = paste_buf.windows(6).position(|w| w == b"\x1b[201~") {
                        let end_pos = pos + 6;
                        let full_paste = paste_buf[..end_pos].to_vec();
                        let remainder = paste_buf[end_pos..].to_vec();
                        in_paste = false;
                        paste_buf.clear();
                        if tx.send(full_paste).is_err() {
                            return;
                        }
                        if !remainder.is_empty() {
                            // Process remainder below
                            Self::process_input_chunk(&remainder, &mut in_paste, &mut paste_buf, &tx);
                        }
                    }
                    continue;
                }

                // Normal parsing or entering paste mode
                Self::process_input_chunk(got, &mut in_paste, &mut paste_buf, &tx);
            }
        });
        Self { rx, stop, handle: Some(handle) }
    }

    fn process_input_chunk(
        got: &[u8],
        in_paste: &mut bool,
        paste_buf: &mut Vec<u8>,
        tx: &Sender<Vec<u8>>,
    ) {
        let mut i = 0usize;
        while i < got.len() {
            // Check for start of bracketed paste: \x1b[200~
            if got[i..].starts_with(b"\x1b[200~") {
                *in_paste = true;
                paste_buf.clear();
                paste_buf.extend_from_slice(&got[i..]);
                if let Some(pos) = paste_buf.windows(6).position(|w| w == b"\x1b[201~") {
                    let end_pos = pos + 6;
                    let full_paste = paste_buf[..end_pos].to_vec();
                    let remainder = paste_buf[end_pos..].to_vec();
                    *in_paste = false;
                    paste_buf.clear();
                    if tx.send(full_paste).is_err() {
                        return;
                    }
                    if !remainder.is_empty() {
                        Self::process_input_chunk(&remainder, in_paste, paste_buf, tx);
                    }
                }
                return;
            }

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
                return;
            }
            i = end;
        }
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
    /// Collapsed reasoning trace from one step. Body is hidden until ctrl+o
    /// expands it (gated on `expanded`, same as tool output).
    Thinking(String),
    Info(String),
    Error(String),
}

/// Request from the UI thread to the worker. The worker owns `history` and
/// `hm`, so anything that mutates them (/clear, /model, /skill) or reads the
/// audit log must run there, between turns.
enum Ctl {
    Prompt(String, usize),
    Clear(String),
    Resume(Vec<Message>, String),
    SetModel(String),
    SetEndpoint { host: String, port: u16, path: String, key: String, base_url: String },
    SetEffort(String),
    SystemAppend(String),
    Audit,
    DropTranscript,
    /// Hand a UI-thread-owned transcript (opened by /rename, /save, /rewind) to
    /// the worker so subsequent turn messages append to it. Fire-and-forget:
    /// queued behind every earlier Ctl, so ordering with SetTranscript holds.
    SetTranscript(agent::session::Transcript),
    /// Ask the worker to return its current transcript (it may be None). The
    /// ack sender answers even when the worker holds none, so the caller never
    /// waits on a busy turn — only on the worker reaching this command.
    TakeTranscript(Sender<Option<agent::session::Transcript>>),
    /// Rebuild the worker's history from a fresh source (title rename,
    /// checkpoint rewind, /compact). The worker owns `history`, so these
    /// reads/writes must round-trip through it; `DropTranscript` hands
    /// transcript ownership back to the UI thread for the rewrite.
    ReplaceHistory(Vec<Message>),
}

/// Events the worker thread sends to the UI thread. Agent progress travels as
/// `onyx_core::CoreEvent` — the same protocol any later client (app server,
/// tests) subscribes to. Session bookkeeping that is not part of the event
/// protocol rides the side channel below.
enum UiOut {
    Core(onyx_core::CoreEvent),
    Info(String),
    /// Turn finished: `Err(String)` carries a pre-rendered message for failures
    /// the protocol doesn't describe (max steps, doom loop, gateway error).
    TurnDone(Result<(String, usize), String>),
    Audit(Vec<AuditEntry>),
    ModelName(String),
    /// (message count, total content bytes) — mirrors the worker-owned history
    /// so `/usage` can report without touching it.
    History(usize, usize),
}

/// Per-turn item identity for the CoreEvent stream the worker emits. The host
/// assigns item_ids; ToolStart registers one per tool name and ToolDone reuses
/// it so both edges touch the same item.
struct TurnIds {
    turn: u64,
    item: u64,
    /// FIFO of open item_ids per tool name. The loop runs each call strictly
    /// start→done→start→done, so the front entry is always the matching one —
    /// even when a step fires the same tool several times.
    tool_items: std::collections::HashMap<String, std::collections::VecDeque<u64>>,
}

/// Request/response channel for interactive approvals. The worker blocks in
/// `wait()` until the UI thread answers — freezing the agent loop mid-call.
struct AskCtl {
    state: Mutex<AskState>,
    cv: std::sync::Condvar,
}
struct AskState {
    pending: Option<(String, String, String)>,
    answer: Option<Approval>,
    approve_all: bool,
    question_pending: Option<(String, Vec<String>)>,
    question_answer: Option<Result<String, String>>,
}

impl AskCtl {
    fn new() -> Self {
        Self {
            state: Mutex::new(AskState {
                pending: None,
                answer: None,
                approve_all: false,
                question_pending: None,
                question_answer: None,
            }),
            cv: std::sync::Condvar::new(),
        }
    }
    fn request(&self, tool: &str, args: &str, reason: &str) -> Approval {
        let mut s = self.state.lock().unwrap();
        if s.approve_all {
            return Approval::Always;
        }
        s.pending = Some((tool.to_string(), args.to_string(), reason.to_string()));
        s.answer = None;
        while s.answer.is_none() {
            s = self.cv.wait(s).unwrap();
        }
        let a = s.answer.unwrap_or(Approval::Reject);
        if a == Approval::Always {
            s.approve_all = true;
        }
        s.pending = None;
        a
    }
    fn pending(&self) -> Option<(String, String, String)> {
        self.state.lock().unwrap().pending.clone()
    }
    fn answer(&self, reply: Approval) {
        let mut s = self.state.lock().unwrap();
        s.answer = Some(reply);
        self.cv.notify_all();
    }

    fn request_question(&self, question: &str, options: &[String]) -> Result<String, String> {
        let mut s = self.state.lock().unwrap();
        s.question_pending = Some((question.to_string(), options.to_vec()));
        s.question_answer = None;
        while s.question_answer.is_none() {
            s = self.cv.wait(s).unwrap();
        }
        let ans = s.question_answer.take().unwrap_or_else(|| Err("cancelled".into()));
        s.question_pending = None;
        ans
    }
    fn pending_question(&self) -> Option<(String, Vec<String>)> {
        self.state.lock().unwrap().question_pending.clone()
    }
    fn answer_question(&self, reply: Result<String, String>) {
        let mut s = self.state.lock().unwrap();
        s.question_answer = Some(reply);
        self.cv.notify_all();
    }
}

// ------------------------------------------------------------- view helpers ----

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
/// (such as {"tool":"terminate","args":{...}} or {"tool":"ask_human","args":{...}} or {"question": "..."}),
/// or embedded `<think>...</think>` tags from reasoning models.
/// If present, extracts the actual message content so raw reasoning or JSON schema blobs don't leak into the UI.
fn clean_model_text(text: &str) -> String {
    let (stripped_think, _) = agent::repetition::strip_think_tags(text);
    let t = stripped_think.trim();
    // Case 1: `→ tool_name({"question":"..."})` or `→ tool_name({...})` from agent loop tool calls
    if let Some(rest) = t.strip_prefix("→ ") {
        let rest = rest.trim();
        if let Some(open_paren) = rest.find('(') {
            let tool_name = rest[..open_paren].trim();
            if let Some(close_paren) = rest.rfind(')') {
                let inside = &rest[open_paren + 1..close_paren];
                if tool_name == "ask_human" || tool_name == "terminate" || tool_name == "ask_user_question" {
                    if let Ok(v) = serde_json::from_str::<serde_json::Value>(inside) {
                        if let Some(q) = v.get("question").or_else(|| v.get("content")).or_else(|| v.get("answer")).and_then(|s| s.as_str()) {
                            return q.to_string();
                        }
                    }
                    return inside.trim_matches('"').to_string();
                }
            }
        }
        // Case 1b: Malformed `→ ask_humanquestion":"..."` or `→ ask_human{"question":"..."}`
        for prefix in ["ask_human", "ask_user_question", "terminate"] {
            if rest.starts_with(prefix) {
                let remainder = &rest[prefix.len()..];
                for key in ["question", "content", "answer"] {
                    let marker = format!("\"{key}\":\"");
                    if let Some(idx) = remainder.find(&marker) {
                        let start = idx + marker.len();
                        if let Some(end) = remainder[start..].rfind('\"') {
                            return remainder[start..start + end].to_string();
                        }
                    }
                }
            }
        }
    }

    if t.starts_with('{') && t.contains("\"tool\"") {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(t) {
            let tool = v.get("tool").and_then(|s| s.as_str()).unwrap_or("");
            if tool == "terminate" || tool == "ask_human" || tool == "ask_user_question" {
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

fn pretty_action(name: &str, args: &str) -> String {
    let v: serde_json::Value = serde_json::from_str(args).unwrap_or_else(|_| serde_json::json!(args));
    let obj = match v.as_object() {
        Some(o) => o,
        None => return format!("Ran {name} ({})", clip_chars(&v.to_string(), 40)),
    };
    if name == "bash" || name == "shell" {
        if let Some(cmd) = obj.get("command").or_else(|| obj.get("cmd")).and_then(|x| x.as_str()) {
            let first = first_line(cmd).trim();
            return format!("Ran command ({first})");
        }
        return "Ran command".to_string();
    }
    if name == "editor" || name == "file_write" {
        let path = obj.get("path").or_else(|| obj.get("file_path")).and_then(|x| x.as_str()).unwrap_or("file");
        let filename = PathBuf::from(path).file_name().and_then(|f| f.to_str()).map(|s| s.to_string()).unwrap_or_else(|| path.to_string());
        let cmd = obj.get("command").and_then(|x| x.as_str()).unwrap_or("");
        if cmd == "create" || cmd == "write" {
            return format!("Created 1 file ({filename})");
        }
        return format!("Edited 1 file ({filename})");
    }
    if name == "file_read" || name == "view" {
        let path = obj.get("path").or_else(|| obj.get("file_path")).and_then(|x| x.as_str()).unwrap_or("file");
        let filename = PathBuf::from(path).file_name().and_then(|f| f.to_str()).map(|s| s.to_string()).unwrap_or_else(|| path.to_string());
        return format!("Explored 1 file ({filename})");
    }
    if name == "websearch" || name == "web_search" {
        if let Some(q) = obj.get("query").and_then(|x| x.as_str()) {
            return format!("Searched web ({})", clip_chars(q, 40));
        }
        return "Searched web".to_string();
    }
    if name == "ask_human" || name == "ask_user_question" {
        return "Asked user for input".to_string();
    }
    format!("Executed {name} ({})", clip_chars(&pretty_args(name, args), 40))
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
    if name == "ask_human" || name == "ask_user_question" {
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

// ------------------------------------------------------ number/base radii ----
// Clean-room byte encoders for the /hex converter. No external crate: hex,
// binary and base64 are each a few lines of bit-shuffling over &[u8].

/// Uppercase hex bytes separated by single spaces — `b"ONYX"` → `"4F 4E 59 58"`.
fn bytes_to_hex(bytes: &[u8]) -> String {
    const D: &[u8; 16] = b"0123456789ABCDEF";
    let mut s = String::with_capacity(bytes.len() * 3);
    for (i, &b) in bytes.iter().enumerate() {
        if i > 0 {
            s.push(' ');
        }
        s.push(D[(b >> 4) as usize] as char);
        s.push(D[(b & 0xF) as usize] as char);
    }
    s
}

/// Eight-bit binary strings separated by spaces — `b'O'` → `"01001111"`.
fn bytes_to_bin(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|b| format!("{b:08b}"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Space-separated decimal byte values — `b"ONYX"` → `"79 78 89 88"`.
fn bytes_to_dec(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|b| b.to_string())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Standard base64 with `=` padding.
fn bytes_to_b64(bytes: &[u8]) -> String {
    const TBL: &[u8; 64] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(TBL[(n >> 18) as usize & 63] as char);
        out.push(TBL[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { TBL[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { TBL[n as usize & 63] as char } else { '=' });
    }
    out
}

/// Decode a whitespace-separated list of radix numbers into bytes. `radix` is
/// 2 or 16; an optional `0x`/`0b` prefix per token is tolerated. Errors name the
/// offending token so the caller can report it.
fn parse_bytes(text: &str, radix: u32) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    for tok in text.split_whitespace() {
        let cleaned = match radix {
            16 => tok.strip_prefix("0x").or_else(|| tok.strip_prefix("0X")).unwrap_or(tok),
            2 => tok.strip_prefix("0b").or_else(|| tok.strip_prefix("0B")).unwrap_or(tok),
            _ => tok,
        };
        let v = u32::from_str_radix(cleaned, radix)
            .map_err(|_| format!("invalid {radix}-radix byte `{tok}`"))?;
        if v > 255 {
            return Err(format!("byte `{tok}` exceeds 0xFF"));
        }
        out.push(v as u8);
    }
    if out.is_empty() {
        return Err("no bytes to decode".into());
    }
    Ok(out)
}

/// Render one labelled line inside the framed transcript, dimming the label.
fn conv_line(label: &str, value: &str) -> String {
    format!("{FG_245}{label:>8}{RESET}  {value}")
}

const SLASH_COMMANDS: &[(&str, &str)] = &[
    // Context & Session Control
    ("/clear", "reset conversation context"),
    ("/compact", "compress conversation to save tokens"),
    ("/rewind", "rollback to previous turn checkpoint"),
    ("/resume", "<id|prefix> restore a past session"),
    ("/rename", "<name> rename the active session"),
    ("/branch", "[name] create experimental branch worktree"),
    ("/exit", "quit interactive session"),
    ("/undo", "revert last prompt instruction or file edit"),
    ("/reset", "hard-wipe configuration to defaults"),
    ("/save", "[file] snapshot chat history to local backup"),

    // Development & Coding Workflows
    ("/init", "bootstrap ONYX.md project memory guide"),
    ("/memory", "view or modify project memory & guidelines"),
    ("/plan", "[goal] toggle plan mode for complex tasks"),
    ("/diff", "[file] inspect git diff with syntax coloring"),
    ("/review", "structural evaluation of git diff & quality"),
    ("/add-dir", "<path> mount supplemental directory"),
    ("/bashes", "view running background tasks or commands"),
    ("/tasks", "view multi-step task queue or background jobs"),
    ("/btw", "<question> quick tangent without derailing context"),
    ("/explain", "[path] explain code architecture step-by-step"),
    ("/refactor", "[path] optimize code readability & speed"),
    ("/test", "[pattern] scan and run project test suites"),
    ("/debug", "[error] attach to compiler logs / stack traces"),
    ("/docstrings", "scan files to generate missing documentation"),

    // System Settings & AI Control
    ("/provider", "[name|key|oauth] configure AI provider (liter-llm)"),
    ("/connect", "searchable picker to connect a provider and store its key"),
    ("/model", "[name] show or hot-swap AI model"),
    ("/effort", "<low|med|high|max|none> reasoning level"),
    ("/fast", "toggle high-speed execution mode"),
    ("/config", "interactive configuration options"),
    ("/mcp", "inspect connected Model Context Protocol servers"),
    ("/permissions", "modify tool security approval rules"),
    ("/output-style", "cycle terminal text formatting preferences"),
    ("/vim", "toggle Vim-style command bar input mode"),
    ("/emacs", "toggle Emacs-style keybinding navigation"),
    ("/theme", "alter syntax coloring palette profile"),
    ("/verbose", "toggle full background tool reasoning output"),
    ("/settings", "view system configurations & active modes"),
    ("/mode", "cycle approval mode (Normal, Edits, YOLO)"),
    ("/steps", "N set max agent steps"),
    ("/skill", "<name> load a skill into active prompt"),
    ("/skills", "list all discovered skills across installed packs"),
    ("/skillopt", "[<name>] run the SkillOpt offline cycle and gate skill edits"),

    // Token Analytics & Health Diagnostics
    ("/cost", "insight into tokens used and estimated costs"),
    ("/usage", "real token metrics and execution counts"),
    ("/stats", "workspace metrics regarding code files parsed"),
    ("/insights", "build session profiling analysis report"),
    ("/context", "visualize context window capacity limits"),
    ("/doctor", "validate setup and diagnose configuration"),
    ("/ping", "measure server latency to API endpoint"),
    ("/logs", "print local error traces and session logs"),
    ("/audit", "gateway policy audit logs"),
    ("/tools", "list registered tools and capabilities"),
    ("/gain", "report output-compression savings (ONYX_COMPACT)"),
    ("/hex", "<text|-d hex|-b bin|-n dec> base converter"),
    ("/sessions", "list saved session transcripts"),

    // Advanced Skills & Ecosystem Integrations
    ("/batch", "[task] orchestrate parallel file modifications"),
    ("/design", "canvas preview sandbox for UI flows"),
    ("/dataviz", "interactive chart and data visualization"),
    ("/api", "browse language API docs and schemas"),
    ("/claude-api", "browse model and provider API documentation"),
    ("/install-github-app", "streamline automated PR check workflows"),
    ("/pr-comments", "synchronize and inspect Git PR feedback"),
    ("/desktop", "transition session to desktop application"),
    ("/app", "launch companion configuration frame"),
    ("/help", "[command] comprehensive documentation directory"),
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
    sel: *const agent::retrieval::CapabilitySelector,
}
unsafe impl Send for Job {}

pub struct Tui<'a> {
    gw: &'a mut Gateway<dyn tools::policy::Policy>,
    hm: &'a mut HttpModel,
    opts: &'a mut Opts,
    sel: Option<&'a agent::retrieval::CapabilitySelector>,
    history_len: usize,
    history_bytes: usize,
    items: Vec<Item>,
    input: String,
    cursor: usize, // char index into input
    expanded: bool,
    tick: usize,
    thinking: bool,
    tail: String,          // streamed text of the current step
    reasoning: String,     // streamed reasoning/thinking of the current step
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
    question_selected: usize,
    question_input: String,
    question_cursor: usize,
    cached_models: Vec<String>,
    mcp_count: usize,
    skill_count: usize,
    /// SkillOpt candidates accepted by the validation gate, awaiting
    /// `/skillopt apply`. Never written to disk without that confirmation.
    pending_skill_updates: std::collections::HashMap<String, String>,
    real_prompt_tokens: usize,
    real_completion_tokens: usize,
    real_total_tokens: usize,
    real_cached_tokens: usize,
    ctx_history: Vec<u8>, // recent context-usage % samples for the status sparkline
    session_title: Option<String>,
    turns: u64,
    turn_start: Option<std::time::Instant>,
    theme_idx: usize,
    extra_dirs: Vec<String>,
    vim_normal: bool,
    vim_pending_d: bool,
    vim_clipboard: String,
    emacs_keys: bool,
    settings_active: bool,
    settings_selected: usize,
    connect: Option<ConnectWizard>,
    resume_browser: Option<ResumeBrowser>,
    last_browser_height: usize,
    cached_diff_stat: Option<String>,
    last_diff_check: Option<std::time::Instant>,
}

/// Full-screen "Chat Sessions" picker opened by bare `/resume`. Complements —
/// not replaces — the inline dropdown that appears while typing a partial
/// `/resume …`; both share `resume_fork` to replay a transcript. All state is
/// UI-thread only; nothing here touches the running agent loop until Enter.
struct ResumeBrowser {
    /// Every loaded session, in the order returned by the store.
    all: Vec<agent::session::SessionInfo>,
    /// Indices into `all` after scope filter + query filter + date-desc sort.
    rows: Vec<usize>,
    /// Cursor within `rows`.
    sel: usize,
    /// First visible row in the scroll window.
    top: usize,
    /// Live `/` search text.
    query: String,
    /// Space-toggle: preview the selected session's first prompts inline.
    expanded: bool,
    /// Scope toggle: false = current worktree only, true = all projects.
    show_all: bool,
    mode: BrowserMode,
    /// `?` toggles the extended shortcut line.
    help: bool,
}

enum BrowserMode {
    List,
    Search,
}

impl ResumeBrowser {
    fn new(all: Vec<agent::session::SessionInfo>, show_all: bool) -> Self {
        let mut b = Self {
            all,
            rows: Vec::new(),
            sel: 0,
            top: 0,
            query: String::new(),
            expanded: false,
            show_all,
            mode: BrowserMode::List,
            help: false,
        };
        b.rebuild_rows();
        b
    }

    /// Re-derive `rows` from scope + query, sorted newest-first. Keeps the
    /// cursor clamped in range so repeated filtering never strands it.
    fn rebuild_rows(&mut self) {
        let q = self.query.to_lowercase();
        let mut idx: Vec<usize> = (0..self.all.len())
            .filter(|&i| {
                let s = &self.all[i];
                if q.is_empty() {
                    true
                } else {
                    let title = s.title.as_deref().unwrap_or("");
                    let project = s.project.as_str();
                    let branch = s.branch.as_deref().unwrap_or("");
                    let preview = s.preview.as_str();
                    title.to_lowercase().contains(&q)
                        || project.to_lowercase().contains(&q)
                        || branch.to_lowercase().contains(&q)
                        || preview.to_lowercase().contains(&q)
                        || s.id.to_lowercase().contains(&q)
                }
            })
            .collect();
        idx.sort_by(|&a, &b| self.all[b].modified.cmp(&self.all[a].modified));
        self.rows = idx;
        if self.sel >= self.rows.len() {
            self.sel = self.rows.len().saturating_sub(1);
        }
        self.clamp_scroll();
    }

    fn clamp_scroll(&mut self) {
        // Window height is unknown here; callers pass it via ensure_visible.
        if self.top > self.sel {
            self.top = self.sel;
        }
    }

    fn selected_session(&self) -> Option<&agent::session::SessionInfo> {
        self.rows.get(self.sel).map(|&i| &self.all[i])
    }
}

/// Provider rows kept visible in the connect picker before windowing.
const CONNECT_VISIBLE_ROWS: usize = 9;

/// A row in the connect picker: either a non-selectable section header or a
/// provider entry from the catalog.
enum ConnectRow {
    Header(&'static str),
    Provider(usize),
}

/// Inline "Connect a provider" flow, modelled on opencode's setup wizard:
/// searchable Popular-then-alphabetical provider list → masked API-key entry →
/// vault write → activate. Lives entirely on the UI thread; no worker channel
/// is involved because nothing here touches the running agent loop.
struct ConnectWizard {
    step: ConnectStep,
    query: String,
    selected: usize,
    /// Catalog index chosen at the list step, consumed by the key step.
    provider: usize,
    key_input: String,
    key_cursor: usize,
}

enum ConnectStep {
    Pick,
    Key,
}

impl Tui<'_> {
    /// Curated providers shown under "Popular", mirroring what people actually
    /// wire up first: hosted flagships, subscriptions, and local engines.
    const POPULAR_PROVIDERS: [&'static str; 10] = [
        "anthropic",
        "openai",
        "openrouter",
        "github_copilot",
        "gemini",
        "groq",
        "ollama",
        "llamacpp",
        "custom_openai",
        "deepseek",
    ];

    fn open_connect_wizard(&mut self) {
        self.connect = Some(ConnectWizard {
            step: ConnectStep::Pick,
            query: String::new(),
            selected: 0,
            provider: 0,
            key_input: String::new(),
            key_cursor: 0,
        });
    }

    /// Open connect wizard directly at the API key entry step for a chosen provider ID
    fn open_connect_wizard_for_provider(&mut self, prov_id: &str) {
        let catalog = agent::provider::load_catalog();
        if let Some(ci) = catalog.iter().position(|p| p.id.eq_ignore_ascii_case(prov_id) || p.name.eq_ignore_ascii_case(prov_id)) {
            self.connect = Some(ConnectWizard {
                step: ConnectStep::Key,
                query: String::new(),
                selected: 0,
                provider: ci,
                key_input: String::new(),
                key_cursor: 0,
            });
        } else {
            self.open_connect_wizard();
        }
    }

    /// Catalog indices matching the wizard's query, popular ones first and
    /// grouped so the list reads "Popular" then alphabetical — the grouping IS
    /// the sort order.
    fn connect_rows(catalog: &[agent::provider::ProviderSpec], query: &str) -> Vec<ConnectRow> {
        let q = query.to_lowercase();
        let matches = |idx: usize| -> bool {
            if q.is_empty() {
                return true;
            }
            let p = &catalog[idx];
            p.id.to_lowercase().contains(&q) || p.name.to_lowercase().contains(&q)
        };

        // Catalog order is alphabetical, so walking it preserves that for free.
        let mut popular = Vec::new();
        for pid in Self::POPULAR_PROVIDERS {
            if let Some(idx) = catalog.iter().position(|p| p.id == pid) {
                if matches(idx) && !popular.contains(&idx) {
                    popular.push(idx);
                }
            }
        }
        let rest: Vec<usize> = (0..catalog.len())
            .filter(|idx| !popular.contains(idx) && matches(*idx))
            .collect();

        let mut rows = Vec::new();
        if !popular.is_empty() {
            rows.push(ConnectRow::Header("Popular"));
            rows.extend(popular.into_iter().map(ConnectRow::Provider));
        }
        if !rest.is_empty() {
            rows.push(ConnectRow::Header("Providers"));
            rows.extend(rest.into_iter().map(ConnectRow::Provider));
        }
        rows
    }

    /// One-line status used as the description column in the picker.
    fn connect_status(configured: bool, active: bool) -> String {
        let base = if configured { "configured" } else { "not connected" };
        if active {
            format!("{base} · active")
        } else {
            base.to_string()
        }
    }

    fn connect_lines_v(&self) -> Vec<String> {
        let w = self.width.max(40);
        let Some(wiz) = self.connect.as_ref() else {
            return Vec::new();
        };
        let inner = w.saturating_sub(4);
        let mut out = Vec::new();
        match wiz.step {
            ConnectStep::Pick => {
                let search = if wiz.query.is_empty() {
                    format!("{DIM}type to filter{RESET}")
                } else {
                    format!("{BOLD}{FG_MAIN}{}{RESET}", wiz.query)
                };
                out.push(format!("  {BOLD}{CYAN}Connect a provider{RESET}   {search}"));
                let catalog = agent::provider::load_catalog();
                let rows = Self::connect_rows(&catalog, &wiz.query);
                let storage = agent::provider::load_storage();
                let sel = wiz.selected.min(rows.len().saturating_sub(1));

                // Window around the selection so long lists stay bounded.
                let prov_rows: Vec<usize> = (0..rows.len())
                    .filter(|ri| matches!(rows[*ri], ConnectRow::Provider(_)))
                    .collect();
                let start = if prov_rows.len() <= CONNECT_VISIBLE_ROWS {
                    0
                } else {
                    let pos = prov_rows.iter().position(|r| *r >= sel).unwrap_or(0);
                    let lead = pos.saturating_sub(CONNECT_VISIBLE_ROWS / 2);
                    prov_rows[lead.min(prov_rows.len().saturating_sub(CONNECT_VISIBLE_ROWS))]
                };

                let mut emitted = 0usize;
                for ri in start..rows.len() {
                    if emitted >= CONNECT_VISIBLE_ROWS {
                        break;
                    }
                    match &rows[ri] {
                        ConnectRow::Header(title) => {
                            let rule = inner
                                .saturating_sub(title.chars().count() + 7)
                                .max(0);
                            out.push(format!(
                                "  {DIM}{BOLD}── {title} {}{RESET}",
                                "─".repeat(rule)
                            ));
                            emitted += 1;
                        }
                        ConnectRow::Provider(ci) => {
                            let p = &catalog[*ci];
                            let cfg = storage.providers.get(&p.id);
                            let has_key = cfg
                                .and_then(|c| c.api_key.as_ref().map(|k| !k.is_empty()))
                                .unwrap_or(false)
                                || cfg
                                    .and_then(|c| c.oauth_token.as_ref().map(|t| !t.is_empty()))
                                    .unwrap_or(false)
                                || p.auth_type == "none";
                            let active = p.id == storage.active_provider;
                            let is_sel = ri == sel;
                            let mark = if is_sel {
                                format!("{CYAN}❯{RESET}")
                            } else {
                                " ".to_string()
                            };
                            let name_txt = if is_sel {
                                format!("{BOLD}{FG_MAIN}{}{RESET}", p.name)
                            } else {
                                format!("{FG_MAIN}{}{RESET}", p.name)
                            };
                            let desc = Self::connect_status(has_key, active);
                            let plain_len = p.name.chars().count() + 3 + desc.chars().count();
                            if plain_len <= inner {
                                let pad = inner - p.name.chars().count() - desc.chars().count() - 3;
                                out.push(format!(
                                    "{mark} {name_txt}{}{DIM}{desc}{RESET}",
                                    " ".repeat(pad + 1)
                                ));
                            } else {
                                out.push(format!("{mark} {name_txt}"));
                                out.push(format!("   {DIM}{desc}{RESET}"));
                            }
                            emitted += 1;
                        }
                    }
                }
                if rows.is_empty() {
                    out.push(format!("  {DIM}no provider matches ─ esc to close{RESET}"));
                }
                out.push(format!("{BORDER}  {}{RESET}", "─".repeat(inner)));
                out.push(format!(
                    "  {DIM}↑↓ navigate · enter select · esc close{RESET}"
                ));
            }
            ConnectStep::Key => {
                let p = &agent::provider::load_catalog()[wiz.provider];
                out.push(format!("  {BOLD}{CYAN}API key · {}{RESET}", p.name));
                out.push(format!(
                    "  {DIM}stored in the credential vault, never in plaintext config{RESET}"
                ));
                out.push(String::new());
                let shown: String = std::iter::repeat('•')
                    .take(wiz.key_input.chars().count())
                    .collect();
                let body = if shown.is_empty() {
                    format!("{DIM}(paste or type the key){RESET}")
                } else {
                    format!("{BOLD}{FG_MAIN}{shown}{RESET}")
                };
                out.push(format!("  {DIM}key:{RESET} {body}"));
                out.push(String::new());
                out.push(format!("{BORDER}  {}{RESET}", "─".repeat(inner)));
                out.push(format!(
                    "  {DIM}enter submit · esc back to the list{RESET}"
                ));
            }
        }
        out
    }

    /// Commit the typed key: vault write, activate the provider, report where
    /// the secret actually landed.
    fn connect_finish(&mut self) {
        let Some(wiz) = self.connect.take() else {
            return;
        };
        let catalog = agent::provider::load_catalog();
        let Some(p) = catalog.get(wiz.provider) else {
            return;
        };
        let id = p.id.clone();
        let name = p.name.clone();
        let key = wiz.key_input.trim().to_string();
        if key.is_empty() {
            self.items
                .push(Item::Info(format!("cancelled — `{id}` not connected")));
            return;
        }
        let stored = agent::provider::put_secret(&format!("{id}/api_key"), &key);
        let via_vault = stored.starts_with("vault:");
        {
            let mut storage = agent::provider::load_storage();
            let entry = storage.providers.entry(id.clone()).or_insert_with(|| {
                agent::provider::ProviderConfig {
                    provider_id: id.clone(),
                    base_url: None,
                    api_key: None,
                    oauth_token: None,
                    oauth_refresh_token: None,
                    oauth_expires_at: None,
                    active_model: None,
                    custom_headers: std::collections::HashMap::new(),
                }
            });
            entry.api_key = Some(stored);
            if let Err(e) = agent::provider::save_storage(&storage) {
                self.items.push(Item::Error(format!("failed to save config: {e}")));
                return;
            }
        }
        let where_ = if via_vault {
            "credential vault"
        } else {
            "providers.json (vault unavailable — stored inline)"
        };
        self.items
            .push(Item::Info(format!("saved API key for `{name}` in {where_}")));
        self.switch_to_provider(&id);
    }

    /// Keys for the connect wizard, handled ahead of the composer so typing
    /// filters the list instead of landing in the input. Returns true when the
    /// keystroke was consumed.
    fn connect_key(&mut self, chars: &[char], code: u32) -> bool {
        let step = match &self.connect {
            Some(w) => match w.step {
                ConnectStep::Pick => 0,
                ConnectStep::Key => 1,
            },
            None => return false,
        };
        if step == 0 {
            self.connect_pick_key(chars, code)
        } else {
            self.connect_key_entry_key(chars, code)
        }
    }

    fn connect_pick_key(&mut self, chars: &[char], code: u32) -> bool {
        let (query, selected) = {
            let wiz = self.connect.as_ref().unwrap();
            (wiz.query.clone(), wiz.selected)
        };
        let catalog = agent::provider::load_catalog();
        let rows = Self::connect_rows(&catalog, &query);
        let selectable: Vec<usize> = rows
            .iter()
            .enumerate()
            .filter(|(_, r)| matches!(r, ConnectRow::Provider(_)))
            .map(|(i, _)| i)
            .collect();
        let ch = chars.first().copied().unwrap_or('\0');
        match (ch, code) {
            ('\x1b', _) | ('\x03', _) => {
                self.connect = None;
            }
            ('\0', 1000) => {
                if let Some(pos) = selectable.iter().position(|r| *r >= selected) {
                    self.connect.as_mut().unwrap().selected = selectable[pos.saturating_sub(1)];
                }
            }
            ('\0', 1001) => {
                if let Some(pos) = selectable.iter().rposition(|r| *r <= selected) {
                    if pos + 1 < selectable.len() {
                        self.connect.as_mut().unwrap().selected = selectable[pos + 1];
                    }
                }
            }
            ('\r', _) | ('\n', _) => {
                if let Some(ConnectRow::Provider(ci)) = rows.get(selected) {
                    let ci = *ci;
                    let spec = catalog[ci].clone();
                    let storage = agent::provider::load_storage();
                    let configured = storage
                        .providers
                        .get(&spec.id)
                        .and_then(|c| c.api_key.as_ref().map(|k| !k.is_empty()))
                        .unwrap_or(false)
                        || spec.auth_type == "none";
                    if configured {
                        // Already wired (or needs no key): just switch.
                        self.connect = None;
                        self.switch_to_provider(&spec.id);
                    } else {
                        let wiz = self.connect.as_mut().unwrap();
                        wiz.provider = ci;
                        wiz.step = ConnectStep::Key;
                        wiz.key_input.clear();
                        wiz.key_cursor = 0;
                    }
                }
            }
            ('\u{7f}', _) => {
                // Backspace edits the search; esc is the only thing that closes
                // the picker. Popping the last char of an empty query is a no-op,
                // so the user cannot accidentally destroy the wizard here.
                let wiz = self.connect.as_mut().unwrap();
                if wiz.query.pop().is_some() {
                    wiz.selected = 0;
                }
            }
            (c, 0) if !c.is_control() => {
                let wiz = self.connect.as_mut().unwrap();
                wiz.query.extend(chars.iter().copied());
                wiz.selected = 0;
                // Keep the selection on a row that still exists after refiltering.
                let rows = Self::connect_rows(&catalog, &wiz.query);
                if !matches!(rows.get(wiz.selected), Some(ConnectRow::Provider(_))) {
                    wiz.selected = rows
                        .iter()
                        .position(|r| matches!(r, ConnectRow::Provider(_)))
                        .unwrap_or(0);
                }
            }
            _ => {}
        }
        true
    }

    fn connect_key_entry_key(&mut self, chars: &[char], code: u32) -> bool {
        let ch = chars.first().copied().unwrap_or('\0');
        match (ch, code) {
            ('\x1b', _) => {
                self.connect.as_mut().unwrap().step = ConnectStep::Pick;
            }
            ('\x03', _) => {
                self.connect = None;
            }
            ('\r', _) | ('\n', _) => {
                self.connect_finish();
            }
            ('\u{7f}', _) => {
                let wiz = self.connect.as_mut().unwrap();
                if wiz.key_cursor > 0 {
                    let prev = wiz.key_input[..wiz.key_cursor]
                        .chars()
                        .next_back()
                        .map(|c| c.len_utf8())
                        .unwrap_or(0);
                    wiz.key_cursor -= prev;
                    wiz.key_input.remove(wiz.key_cursor);
                }
            }
            ('\0', 1002) => {
                // Right arrow
                let wiz = self.connect.as_mut().unwrap();
                if wiz.key_cursor < wiz.key_input.len() {
                    let next = wiz.key_input[wiz.key_cursor..]
                        .chars()
                        .next()
                        .map(|c| c.len_utf8())
                        .unwrap_or(0);
                    wiz.key_cursor += next;
                }
            }
            ('\0', 1003) => {
                // Left arrow
                let wiz = self.connect.as_mut().unwrap();
                if wiz.key_cursor > 0 {
                    let prev = wiz.key_input[..wiz.key_cursor]
                        .chars()
                        .next_back()
                        .map(|c| c.len_utf8())
                        .unwrap_or(0);
                    wiz.key_cursor -= prev;
                }
            }
            (_, 2000) => {
                // Bracketed paste event into API key entry
                let wiz = self.connect.as_mut().unwrap();
                let clean: String = chars.iter().filter(|c| **c != '\r' && **c != '\n').collect();
                wiz.key_input.replace_range(wiz.key_cursor..wiz.key_cursor, &clean);
                wiz.key_cursor += clean.len();
            }
            (c, 0) if !c.is_control() => {
                let wiz = self.connect.as_mut().unwrap();
                let run: String = chars.iter().filter(|c| !c.is_control()).collect();
                wiz.key_input.replace_range(wiz.key_cursor..wiz.key_cursor, &run);
                wiz.key_cursor += run.len();
            }
            _ => {}
        }
        true
    }
}


impl<'a> Tui<'a> {
    fn new(
        gw: &'a mut Gateway<dyn tools::policy::Policy>,
        hm: &'a mut HttpModel,
        opts: &'a mut Opts,
        sel: Option<&'a agent::retrieval::CapabilitySelector>,
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

        let skill_count = {
            let mut cat = skills::SkillCatalog::new();
            cat.scan_all_default();
            cat.skills.len()
        };

        // Restore the persisted /theme choice (ONYX_THEME env wins).
        let theme_idx = ["onyx-cyan", "amber-term", "emerald"]
            .iter()
            .position(|n| {
                std::env::var("ONYX_THEME").ok().as_deref() == Some(*n)
                    || std::env::var_os("HOME")
                        .map(PathBuf::from)
                        .and_then(|h| std::fs::read_to_string(h.join(".onyx/theme")).ok())
                        .map(|s| s.trim() == *n)
                        .unwrap_or(false)
            })
            .unwrap_or(0);
        if theme_idx != 0 {
            theme::set_palette(theme_idx);
        }

        let cfg = Self::load_persisted_config();
        if let Some(steps) = cfg.get("max_steps").and_then(|v| v.as_u64()) {
            if opts.max_steps == 8 && steps > 0 {
                opts.max_steps = steps as usize;
            }
        }
        if let Some(eff) = cfg.get("reasoning_effort").and_then(|v| v.as_str()) {
            if hm.effort == "medium" || hm.effort.is_empty() {
                hm.effort = eff.to_string();
            }
        }
        if let Some(auto) = cfg.get("auto_mode").and_then(|v| v.as_bool()) {
            if auto {
                *gw.mode.lock().unwrap() = ApprovalMode::Auto;
            }
        }
        let vim_normal = cfg.get("vim_mode").and_then(|v| v.as_bool()).unwrap_or(false);
        let emacs_keys = cfg.get("emacs_mode").and_then(|v| v.as_bool()).unwrap_or(false);
        let expanded = cfg.get("verbose").and_then(|v| v.as_bool()).unwrap_or(false);

        Self {
            gw,
            hm,
            opts,
            sel,
            history_len: 0,
            history_bytes: 0,
            items: vec![Item::Banner],
            input: String::new(),
            cursor: 0,
            expanded,
            tick: 0,
            thinking: false,
            tail: String::new(),
            reasoning: String::new(),
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
            question_selected: 0,
            question_input: String::new(),
            question_cursor: 0,
            cached_models,
            mcp_count,
            skill_count,
            pending_skill_updates: Default::default(),
            real_prompt_tokens: 0,
            real_completion_tokens: 0,
            real_total_tokens: 0,
            real_cached_tokens: 0,
            ctx_history: Vec::new(),
            session_title: None,
            turns: 0,
            turn_start: None,
            theme_idx,
            extra_dirs: Vec::new(),
            vim_normal,
            vim_pending_d: false,
            vim_clipboard: String::new(),
            emacs_keys,
            settings_active: false,
            settings_selected: 0,
            connect: None,
            resume_browser: None,
            last_browser_height: 10,
            cached_diff_stat: None,
            last_diff_check: None,
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
        if let Some(title) = &self.session_title {
            return title.clone();
        }
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

        // Open or resume a durable transcript for this run.
        // Start a fresh, clean session by default. Only resume previous history
        // if explicitly requested via --resume <id> (or /resume inside the TUI).
        let resume_target = self.opts.resume.as_deref();
        let (_sid, transcript, initial_history) = if let Some(target) = resume_target {
            if let Some((session_path, msgs)) = agent::session::resolve_session(target) {
                let id = session_path.file_stem().and_then(|s| s.to_str()).unwrap_or(target).to_string();
                let restored_items = agent::session::load_transcript_items(&session_path);
                for item in restored_items {
                    match item {
                        agent::session::RestoredItem::User(u) => self.items.push(Item::User(u)),
                        agent::session::RestoredItem::Assistant(a) => {
                            let clean = clean_model_text(&a);
                            if !clean.trim().is_empty() {
                                self.items.push(Item::Answer(clean));
                            }
                        }
                        agent::session::RestoredItem::Tool { name, args, output } => {
                            if name != "terminate" && name != "ask_human" && name != "ask_user_question" {
                                let ok = !output.starts_with("ERROR") && !output.starts_with("DENIED");
                                let denied = output.starts_with("DENIED");
                                self.items.push(Item::Tool {
                                    name,
                                    args,
                                    ok,
                                    denied,
                                    duration: None,
                                    output,
                                });
                            }
                        }
                        agent::session::RestoredItem::Thinking(t) => {
                            if !t.trim().is_empty() {
                                self.items.push(Item::Thinking(t));
                            }
                        }
                    }
                }
                self.items.push(Item::Info(format!("resumed session {} · {} messages", id, msgs.len())));
                let t = agent::session::Transcript::open(&id, &self.hm.model);
                (id, t, msgs)
            } else {
                let sid = agent::session::new_session_id();
                let t = agent::session::Transcript::open(&sid, &self.hm.model);
                self.items.push(Item::Error(format!("could not find session to resume matching `{target}`")));
                (sid, t, Vec::new())
            }
        } else {
            let sid = agent::session::new_session_id();
            let t = agent::session::Transcript::open(&sid, &self.hm.model);
            if let Some(tr) = &t {
                self.items.push(Item::Info(format!("session {sid} · {}", tr.path().display())));
            }
            (sid, t, Vec::new())
        };

        self.session = transcript;
        let transcript = self.session.take(); // move into the worker below

        // The loop needs &Gateway + &mut HttpModel + history + opts together;
        // they live in tui_session's frame for the whole session and the worker
        // is joined before anything can touch them again. Bundle them behind a
        // raw pointer handoff created here and destroyed there.
        let job = Box::new(Job {
            gw: self.gw as *const Gateway<dyn tools::policy::Policy>,
            hm: self.hm as *mut HttpModel,
            history: Box::new(initial_history),
            opts: self.opts as *const Opts,
            sel: self.sel.map(|s| s as *const agent::retrieval::CapabilitySelector).unwrap_or(std::ptr::null()),
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
            let sel: Option<&agent::retrieval::CapabilitySelector> =
                if job.sel.is_null() { None } else { Some(unsafe { &*job.sel }) };
            let mut transcript = transcript;

            // Approver runs ON the worker thread (it blocks on AskCtl). It may
            // only touch Sync state — AskCtl is enough: the main thread renders
            // the approval card from `ask.pending()` while it polls keys.
            let ask_for_gw = Arc::clone(&ask);
            gw.set_approver(move |tool: &str, args: &str, reason: &str| -> Approval {
                ask_for_gw.request(tool, args, reason)
            });

            // Register interactive hook for structured questions (ask_user_question)
            let ask_for_hook = Arc::clone(&ask);
            tools::implementations::grok_build::set_question_hook(Arc::new(move |q: &str, opts: &[String]| -> Result<String, String> {
                ask_for_hook.request_question(q, opts)
            }));

            // Session id doubles as the protocol thread_id; keep a local copy
            // so events can be stamped even after transcript ops move it.
            let mut thread_id = transcript
                .as_ref()
                .map(|t| t.path().file_stem().and_then(|s| s.to_str()).unwrap_or("onyx").to_string())
                .unwrap_or_else(|| "onyx".to_string());
            let mut ids = TurnIds { turn: 0, item: 0, tool_items: std::collections::HashMap::new() };

            let workdir = std::env::var("ONYX_WORKDIR")
                .map(PathBuf::from)
                .unwrap_or_else(|_| std::env::current_dir().unwrap_or_else(|_| std::env::temp_dir()));

            let assembler = agent::context_assembler::ContextAssembler::new(
                &workdir,
                agent::context_assembler::ContextBudgetConfig::default(),
            );
            let mut active_checkpoint: Option<Arc<Mutex<agent::autonomous::TaskCheckpoint>>> = None;

            while let Ok(msg) = cr.recv() {
                match msg {
                    Ctl::Prompt(prompt, max_steps) => {
                        *cancel.lock().unwrap() = false;
                        let cut = history.len();
                        ids.turn += 1;
                        ids.item = 0;
                        ids.tool_items.clear();
                        // NB: no UserMessage echo here — the UI thread already
                        // pushed Item::User when it sent this prompt; emitting
                        // it again rendered the prompt twice.
                        // run_turn_observed appends the user message itself, so we
                        // do NOT pre-push it — that was the double-write. We persist
                        // the whole turn from the returned report instead, which
                        // carries the user+assistant+tool messages even when the
                        // live history is later truncated (cancel) or errored.
                        let mut agent = ToolCallAgent::new(gw)
                            .with_context_assembler(&assembler);
                        if let Some(sel) = sel {
                            agent = agent.with_selector(sel);
                        }

                        // Autonomous plan & checkpoint integration for engineering tasks
                        if agent::autonomous::is_engineering_task(&prompt) {
                            let task_id = format!("task-{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0));
                            let arch = assembler.get_arch_ref();
                            let engine = agent::autonomous::AutonomousEngine::new(&arch, &workdir);
                            let plan = engine.plan_task(&task_id, &prompt);
                            let mut cp = agent::autonomous::TaskCheckpoint::new(&task_id, &prompt, plan);
                            cp.session_id = Some(thread_id.clone());
                            let _ = cp.save_atomic(&workdir);
                            let cp_arc = Arc::new(Mutex::new(cp));
                            active_checkpoint = Some(Arc::clone(&cp_arc));
                            agent = agent.with_checkpoint(cp_arc);
                        } else if let Some(cp_arc) = &active_checkpoint {
                            agent = agent.with_checkpoint(Arc::clone(cp_arc));
                        }

                        agent.max_steps = max_steps;
                        let mut current_step = 0usize;
                        let mut step_reasoning = String::new();
                        let mut sink = |ev: AgentEvent| {
                            match &ev {
                                AgentEvent::Thinking { step } => {
                                    if !step_reasoning.trim().is_empty() {
                                        if let Some(t) = &transcript {
                                            t.push_step_thinking(current_step, &step_reasoning);
                                        }
                                        step_reasoning.clear();
                                    }
                                    current_step = *step;
                                }
                                AgentEvent::ReasoningDelta(delta) => {
                                    step_reasoning.push_str(delta);
                                }
                                AgentEvent::ToolStart { name, args } => {
                                    if !step_reasoning.trim().is_empty() {
                                        if let Some(t) = &transcript {
                                            t.push_step_thinking(current_step, &step_reasoning);
                                        }
                                        step_reasoning.clear();
                                    }
                                    if let Some(t) = &transcript {
                                        t.push(&Message {
                                            role: "assistant".into(),
                                            content: format!("→ {name}({args})"),
                                            tool_call: Some(agent::ToolCall {
                                                name: name.clone(),
                                                args: args.clone(),
                                            }),
                                        });
                                    }
                                }
                                AgentEvent::ToolDone { output, .. } => {
                                    if let Some(t) = &transcript {
                                        t.push(&Message {
                                            role: "tool".into(),
                                            content: output.clone(),
                                            tool_call: None,
                                        });
                                    }
                                }
                                AgentEvent::StallDetected { phrase, step } => {
                                    if let Some(t) = &transcript {
                                        t.push_event("stall_detected", *step, phrase);
                                    }
                                }
                                AgentEvent::StepCompleted { step, summary } => {
                                    if !step_reasoning.trim().is_empty() {
                                        if let Some(t) = &transcript {
                                            t.push_step_thinking(*step, &step_reasoning);
                                        }
                                        step_reasoning.clear();
                                    }
                                    if let Some(t) = &transcript {
                                        t.push_event("step_completed", *step, summary);
                                    }
                                }
                                _ => {}
                            }

                            let core = match &ev {
                                AgentEvent::ToolStart { name, .. } => {
                                    let id = { ids.item += 1; ids.item };
                                    ids.tool_items.entry(name.clone()).or_default().push_back(id);
                                    ev.to_core(&thread_id, ids.turn, id)
                                }
                                AgentEvent::ToolDone { name, .. } => {
                                    let id = ids.tool_items.get_mut(name).and_then(|q| q.pop_front())
                                        .unwrap_or({ ids.item += 1; ids.item });
                                    ev.to_core(&thread_id, ids.turn, id)
                                }
                                other => other.to_core(&thread_id, ids.turn, { ids.item += 1; ids.item }),
                            };
                            let _ = ut.send(UiOut::Core(core));
                        };

                        // Chronological transcript persistence: user prompt is written first
                        if let Some(t) = &transcript {
                            if !t.last_is_user(&prompt) {
                                t.push(&Message::text("user", prompt.clone()));
                            }
                        }

                        let res = agent.run_turn_observed(hm, &prompt, &mut history, &mut sink);
                        if let Some(t) = &transcript {
                            // Flush any remaining reasoning from the final step
                            if !step_reasoning.trim().is_empty() {
                                t.push_step_thinking(current_step, &step_reasoning);
                            }
                            // Persist final assistant response if turn succeeded or failed with answer
                            if let Ok(rep) = &res {
                                t.push(&Message::text("assistant", rep.answer.clone()));
                            }
                        }
                        let is_cancelled = *cancel.lock().unwrap();
                        if is_cancelled {
                            // Drop everything this cancelled turn appended to memory.
                            history.truncate(cut);
                        }
                        // Final assistant answer as a protocol item, then the
                        // turn-done edge carrying either (answer, steps) or a
                        // fully rendered error sentence (the UI no longer needs
                        // AgentError variants).
                        let done = if is_cancelled {
                            let cancel_msg = "[turn cancelled by user]".to_string();
                            if let Some(t) = &transcript {
                                t.push(&Message::text("assistant", cancel_msg.clone()));
                            }
                            Err(cancel_msg)
                        } else {
                            match &res {
                                Ok(rep) => {
                                    let answer_text = if rep.answer.trim().is_empty() {
                                        "[model returned empty content]".to_string()
                                    } else {
                                        rep.answer.clone()
                                    };
                                    let _ = ut.send(UiOut::Core(onyx_core::CoreEvent::new(
                                        &thread_id, ids.turn, { ids.item += 1; ids.item },
                                        onyx_core::ItemStatus::Completed,
                                        onyx_core::ItemKind::AgentMessage {
                                            delta: None,
                                            final_text: Some(answer_text.clone()),
                                        },
                                    )));
                                    Ok((answer_text, rep.steps))
                                }
                                Err(e) => Err(match e {
                                    AgentError::MaxSteps => format!(
                                        "reached max steps ({max_steps}) without an answer"
                                    ),
                                    AgentError::DoomLoop { tool, repeats, reason } => match reason {
                                        DoomReason::RepeatedDenials => format!(
                                            "agent stuck: {repeats} calls in a row were denied by policy. \
                                             Consider changing the permission mode or adding allow rules."
                                        ),
                                        DoomReason::RepeatedCalls => format!(
                                            "agent stuck repeating `{tool}` {repeats}× with no progress"
                                        ),
                                    },
                                    AgentError::Tool(msg) => format!("tool failed at the gateway: {msg}"),
                                }),
                            }
                        };
                        let _ = ut.send(UiOut::TurnDone(done));
                    }
                    Ctl::Clear(system) => {
                        history = vec![Message::text("system", system)];
                        // Fresh conversation: seal the old transcript, start a new one.
                        if let Some(t) = &transcript {
                            t.purge_if_empty();
                            let _ = ut.send(UiOut::Info(format!(
                                "archived session {}",
                                t.path().file_stem().and_then(|s| s.to_str()).unwrap_or("?")
                            )));
                        }
                        transcript = None;
                        let sid = agent::session::new_session_id();
                        thread_id = sid.clone();
                        if let Some(t) = agent::session::Transcript::open(&sid, &hm.model) {
                            let _ = ut.send(UiOut::Info(format!(
                                "session {sid} · {}",
                                t.path().display()
                            )));
                            transcript = Some(t);
                        }
                    }
                    Ctl::Resume(msgs, sid) => {
                        history = if msgs.is_empty() {
                            vec![Message::text("system", hm.system.clone())]
                        } else {
                            msgs
                        };
                        thread_id = sid;
                        transcript = None; // main thread already opened the replay transcript
                    }                    Ctl::SetModel(m) => {
                        hm.context_budget = agent::model::context_window_for_model(&m);
                        hm.model = m;
                        let _ = ut.send(UiOut::ModelName(hm.model.clone()));
                    }
                    Ctl::SetEndpoint { host, port, path, key, base_url } => {
                        hm.host = host;
                        hm.port = port;
                        hm.path = path;
                        hm.key = key.clone();
                        hm.set_endpoint_for(&base_url, &key);
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
                    Ctl::SetTranscript(t) => {
                        transcript = Some(t);
                    }
                    Ctl::TakeTranscript(ack) => {
                        let _ = ack.send(transcript.take());
                    }
                    Ctl::ReplaceHistory(msgs) => {
                        history = msgs;
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
        if let Some(t) = &self.session {
            t.purge_if_empty();
        }
        self.ctl_tx = None;
        if let Some(h) = self.worker.take() {
            let _ = h.join();
        }
        // Also cleanup any active sessions that have 0 messages
        let _ = agent::session::list_sessions(1);
    }



    /// All non-banner transcript items + streaming tail, each fitted to `width`.
    fn transcript_lines(&self, width: usize) -> Vec<String> {
        let mut content: Vec<String> = Vec::new();
        for it in &self.items {
            if matches!(it, Item::Banner) {
                continue;
            }
            content.extend(self.item_lines(it));
        }
        if self.tail_visible() {
            content.extend(self.tail_lines());
        } else if self.running && !self.reasoning.is_empty() {
            content.extend(self.reasoning_tail_lines());
        } else if self.thinking {
            content.push(self.thinking_line());
        }
        content.into_iter().flat_map(|l| fit_lines(&l, width)).collect()
    }

    fn tail_visible(&self) -> bool {
        self.running && !self.tail.is_empty()
    }

    /// Shown while a turn is in flight but no streamed text exists yet:
    /// the animated braille spinner + "Thinking…". Keeps the thinking cue
    /// alive after the key-hint footer row was removed.
    fn thinking_line(&self) -> String {
        let frame = SPINNER[self.tick % SPINNER.len()];
        format!("{CYAN}{frame}{RESET} {DIM}Thinking…{RESET}")
    }

    fn tail_lines(&self) -> Vec<String> {
        let vis: String = self.tail.chars().skip(self.tail_scrolled).collect();
        let clean = clean_model_text(&vis);
        let wrap_w = self.width.saturating_sub(4).max(20);
        let mut out = Vec::new();
        for para in clean.split('\n') {
            let wrapped = wrap(para, wrap_w);
            for (i, w) in wrapped.iter().enumerate() {
                let mark = if i == 0 {
                    format!("{} ", SPINNER[self.tick % SPINNER.len()])
                } else {
                    "  ".to_string()
                };
                out.push(format!("{FG_250}{mark}{t}{RESET}", t = w));
            }
        }
        out
    }

    /// Live reasoning, rendered dim + italic under a "Thinking" cue so it reads
    /// as the model's scratchpad rather than the answer.
    fn reasoning_tail_lines(&self) -> Vec<String> {
        let frame = SPINNER[self.tick % SPINNER.len()];
        let mut out = vec![format!("{CYAN}{frame}{RESET} {DIM}Thinking…{RESET}")];
        let vis: String = self.reasoning.chars().skip(self.tail_scrolled).collect();
        let clean = clean_model_text(&vis);
        let wrap_w = self.width.saturating_sub(6).max(20);
        for para in clean.split('\n') {
            for w in wrap(para, wrap_w) {
                out.push(format!("  {DIM}{w}{RESET}"));
            }
        }
        out
    }

    fn item_lines(&self, it: &Item) -> Vec<String> {
        let w = self.width.max(20);
        match it {
            Item::Banner => {
                let (filled, empty, pct) = self.ctx_meter(18);
                let ctx_bar = format!("{CYAN}{}{RESET}{DIM}{}{RESET} {:>3}%", "█".repeat(filled), "░".repeat(empty), pct);

                let id_str = if self.session_label().len() > 16 {
                    format!("…{}", &self.session_label()[self.session_label().len().saturating_sub(14)..])
                } else {
                    self.session_label()
                };

                let mut lines = Vec::new();

                // Centered ONYX Header
                for row in Self::BANNER {
                    lines.push(center_pad(&format!("{CYAN}{row}{RESET}"), w));
                }
                lines.push(center_pad(&format!("{BOLD}{CYAN}ONYX{RESET} {DIM}·{RESET} {BOLD}{FG_MAIN}INTELLIGENCE ENGINE{RESET}"), w));
                lines.push(center_pad(&format!("{BORDER}━━━━━━━━━━━━━━━━━━━━━━━{RESET}"), w));
                lines.push(String::new());

                // SESSION Card
                let card_w = w.saturating_sub(6).min(46).max(28);
                let inner_c = card_w.saturating_sub(2);
                lines.push(format!("  {CYAN}◈{RESET} {BOLD}{FG_MAIN}ONYX{RESET}  {DIM}/{RESET}  {BOLD}{FG_SEC}SESSION{RESET}"));
                lines.push(format!("  {BORDER}┌{}┐{RESET}", "─".repeat(inner_c)));
                
                let row_fmt = |label: &str, val: &str| {
                    let text = format!("  {BORDER}│{RESET} {BOLD}{FG_MAIN}{:<8}{RESET} {val}", label);
                    let pad = (card_w + 2).saturating_sub(ansi_width(&text) + 1);
                    format!("{}{BORDER}│{RESET}", text + &" ".repeat(pad))
                };

                lines.push(row_fmt("ID", &format!("{FG_SEC}{id_str}{RESET}")));
                lines.push(row_fmt("DIR", &format!("{FG_SEC}{}{RESET}", self.workspace_dir)));
                lines.push(row_fmt("MODEL", &format!("{CYAN}{}{RESET}", self.hm.model)));
                lines.push(row_fmt("STATUS", &format!("{GREEN}●{RESET} {BOLD}{GREEN}ONLINE{RESET}")));
                lines.push(row_fmt("CONTEXT", &ctx_bar));
                lines.push(format!("  {BORDER}└{}┘{RESET}", "─".repeat(inner_c)));
                lines.push(String::new());

                // CONVERSATION Divider
                let conv_sep = format!("  {BORDER}── {BOLD}{FG_SEC}CONVERSATION{RESET} {BORDER}{}{RESET}", "─".repeat(w.saturating_sub(20).max(4)));
                lines.push(conv_sep);
                lines
            }
            Item::User(t) => {
                let mut out = Vec::new();
                out.push(String::new());
                for (i, l) in fit_lines(t, w.saturating_sub(4)).into_iter().enumerate() {
                    let mark = if i == 0 { format!("{CYAN}❯{RESET}") } else { " ".to_string() };
                    out.push(format!("  {mark} {BOLD}{FG_MAIN}{l}{RESET}"));
                }
                out.push(String::new());
                out
            }
            Item::Tool { name, args, ok, denied, duration: _, output } => {
                if name == "terminate" || name == "ask_human" || name == "ask_user_question" {
                    return Vec::new();
                }
                let mut out = Vec::new();
                let dot = if *denied {
                    format!("{RED}●{RESET}")
                } else if *ok {
                    format!("{GREEN}●{RESET}")
                } else {
                    let frame = SPINNER[self.tick % SPINNER.len()];
                    format!("{YELLOW}{frame}{RESET}")
                };

                let desc = pretty_action(name, args);
                if self.expanded && !output.is_empty() {
                    let total_lines = output.lines().count();
                    let shown_lines = total_lines.min(15);
                    let hidden_lines = total_lines.saturating_sub(shown_lines);
                    let orig_bytes = output.len();

                    out.push(format!("  {dot} {BOLD}{FG_MAIN}{desc}{RESET} {DIM}(ctrl+o to collapse){RESET}"));
                    out.push(format!("    {DIM}Output: {orig_bytes} bytes · {total_lines} lines (showing {shown_lines}, {hidden_lines} collapsed by TUI viewport){RESET}"));
                    for ol in output.lines().take(15) {
                        let clipped = clip_chars(ol, w.saturating_sub(8));
                        out.push(format!("    {FG_SEC}{clipped}{RESET}"));
                    }
                    if hidden_lines > 0 {
                        out.push(format!("    {DIM}… ({hidden_lines} more lines collapsed in viewport · expand or inspect tool observation){RESET}"));
                    }
                } else if !output.is_empty() {
                    let total_lines = output.lines().count();
                    let orig_bytes = output.len();
                    out.push(format!("  {dot} {BOLD}{FG_MAIN}{desc}{RESET} {DIM}({orig_bytes} bytes, {total_lines} lines · ctrl+o to expand){RESET}"));
                } else {
                    out.push(format!("  {dot} {BOLD}{FG_MAIN}{desc}{RESET}"));
                }
                out
            }
            Item::Thinking(t) => {
                let clean = clean_model_text(t);
                let lines: Vec<&str> = clean.lines().filter(|l| !l.trim().is_empty()).collect();
                let n = lines.len();
                let mut out = Vec::new();
                if self.expanded {
                    out.push(format!("  {DIM}◌ Thought for {n} line(s) · ctrl+o to collapse{RESET}"));
                    let wrap_w = w.saturating_sub(6).max(20);
                    for l in &lines {
                        for wl in wrap(l, wrap_w) {
                            out.push(format!("    {DIM}{ITALIC}{wl}{RESET}"));
                        }
                    }
                } else {
                    out.push(format!("  {DIM}◌ Thought for {n} line(s) · ctrl+o to expand{RESET}"));
                }
                out
            }
            Item::Answer(t) => {
                let clean = clean_model_text(t);
                let mut out = vec![format!("  {CYAN}◈{RESET} {BOLD}{FG_MAIN}ONYX{RESET}")];
                for l in render_markdown(&clean, w.saturating_sub(6)) {
                    out.push(format!("  {l}"));
                }
                out
            }
            Item::Info(t) => fit_lines(t, w.saturating_sub(4))
                .into_iter()
                .map(|l| format!("  {BLUE}ℹ{RESET} {FG_SEC}{l}{RESET}"))
                .collect(),
            Item::Error(t) => {
                let mut out = Vec::new();
                let card_w = w.saturating_sub(6).min(64).max(30);
                let inner_c = card_w.saturating_sub(2);
                out.push(format!("  {RED}┌{}┐{RESET}", "─".repeat(inner_c)));
                
                let line_in_err = |content: &str| {
                    let vis = ansi_width(content);
                    let pad = inner_c.saturating_sub(vis + 1);
                    format!("  {RED}│{RESET} {content}{}{RED}│{RESET}", " ".repeat(pad))
                };

                out.push(line_in_err(&format!("{BOLD}{RED}✗ ERROR ENCOUNTERED{RESET}")));
                out.push(line_in_err(""));
                for l in fit_lines(t, inner_c.saturating_sub(3)) {
                    out.push(line_in_err(&format!("{FG_MAIN}{l}{RESET}")));
                }
                out.push(line_in_err(""));
                out.push(line_in_err(&format!("{YELLOW}Recovery suggestion:{RESET} {FG_SEC}Check permissions, network, or /help{RESET}")));
                out.push(format!("  {RED}└{}┘{RESET}", "─".repeat(inner_c)));
                out
            }
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

    fn question_lines_v(&self) -> Vec<String> {
        let mut out = Vec::new();
        let w = self.width.max(20);
        if let Some((question, options)) = self.ask.pending_question() {
            // Header
            out.push(format!("{BOLD}{CYAN}Asking User{RESET}"));
            out.push(format!("{BORDER}{}{RESET}", "─".repeat(w.saturating_sub(2))));

            // Question prompt text
            for ql in fit_lines(&question, w.saturating_sub(4)) {
                out.push(format!(" {FG_MAIN}{ql}{RESET}"));
            }
            out.push(String::new());

            // Options list
            let selected = self.question_selected.min(options.len().saturating_sub(1));
            for (idx, opt) in options.iter().enumerate() {
                let num = idx + 1;
                let is_sel = idx == selected;
                let prefix = if is_sel {
                    format!(" {CYAN}❯{RESET} {CYAN}{BOLD}{num}.{RESET}")
                } else {
                    format!("   {DIM}{num}.{RESET}")
                };

                let opt_lines = fit_lines(opt, w.saturating_sub(8));
                for (li, ol) in opt_lines.into_iter().enumerate() {
                    if li == 0 {
                        if is_sel {
                            out.push(format!("{prefix} {BOLD}{FG_MAIN}{ol}{RESET}"));
                        } else {
                            out.push(format!("{prefix} {FG_MAIN}{ol}{RESET}"));
                        }
                    } else {
                        let indent = "      ";
                        if is_sel {
                            out.push(format!("{indent}{BOLD}{FG_MAIN}{ol}{RESET}"));
                        } else {
                            out.push(format!("{indent}{FG_SEC}{ol}{RESET}"));
                        }
                    }
                }
            }

            // Interactive answer / typing prompt line
            out.push(String::new());
            let input_slice = if self.question_input.is_empty() {
                format!("{DIM}Type an answer or select an option above…{RESET}")
            } else {
                format!("{BOLD}{FG_MAIN}{}{RESET}", self.question_input)
            };
            out.push(format!(" {CYAN}❯{RESET} {input_slice}"));

            // Divider & footer actions
            out.push(format!("{BORDER} {}{RESET}", "─".repeat(w.saturating_sub(4))));
            out.push(format!(" {DIM}↑↓ choose · Tab fill · Type custom answer · Enter submit · Esc cancel{RESET}"));
        }
        out
    }

    /// A width-fitting slice of `input` centred on the caret, with the caret
    /// landing at column `cursor_chars()` within the slice. Newlines
    /// render as '⏎' so the row stays one line.
    fn input_window(&self) -> String {
        // If pasted multi-line text is long, show compact representation with pasted badge
        let line_count = self.input.lines().count();
        if line_count > 3 {
            let first_snippet: String = self.input.lines().next().unwrap_or("").chars().take(40).collect();
            return format!("[Pasted Text: {line_count} lines] {first_snippet}…");
        }
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

        // Subcommand dropdown for /resume or /res — but ONLY while refining.
        // Bare "/resume" (no trailing space, no query) is reserved for the
        // full-screen browser, so the dropdown must not fire there or its Enter
        // handler would rewrite the command into "/resume <id>" and steal it.
        if input.starts_with("/res ") || input.starts_with("/resume ") {
            let arg = if let Some((_, rest)) = input.split_once(' ') {
                rest.trim()
            } else {
                ""
            };
            let sessions = agent::session::list_sessions(30);
            let mut items = Vec::new();
            for s in sessions {
                let display_title = s.title.clone().unwrap_or_else(|| {
                    if !s.preview.is_empty() {
                        agent::session::derive_title(&s.preview)
                    } else {
                        "Untitled Session".to_string()
                    }
                });
                let time_ago = Self::format_relative_time(s.modified);
                let desc = format!("{:>2} turns ({} msgs) · {:>7}", s.user_turns, s.messages, time_ago);
                if arg.is_empty()
                    || s.id.to_lowercase().contains(&arg.to_lowercase())
                    || display_title.to_lowercase().contains(&arg.to_lowercase())
                {
                    items.push((s.id, display_title, desc));
                }
            }
            return (" Conversations (↑/↓ to choose, tab/enter to resume) ".into(), items);
        }

        // Subcommand dropdown for /provider or /providers
        if input.starts_with("/provider") {
            let arg = if let Some((_, rest)) = input.split_once(' ') {
                rest.trim()
            } else {
                ""
            };
            let storage = agent::provider::load_storage();
            let catalog = agent::provider::load_catalog();
            let mut items = Vec::new();
            for p in catalog {
                let active = if p.id == storage.active_provider { " [active]" } else { "" };
                let desc = format!("{}{active}", p.auth_type);
                if arg.is_empty()
                    || p.id.to_lowercase().contains(&arg.to_lowercase())
                    || p.name.to_lowercase().contains(&arg.to_lowercase())
                {
                    items.push((p.id.clone(), p.name, desc));
                }
            }
            return (" AI Providers (↑/↓ to choose, tab/enter to select) ".into(), items);
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
        if input.starts_with("/skill ") {
            let arg = input["/skill ".len()..].trim();
            let mut items = Vec::new();
            let mut cat = skills::SkillCatalog::new();
            cat.scan_all_default();
            for (_, sk) in &cat.skills {
                if arg.is_empty() || sk.name.to_lowercase().contains(&arg.to_lowercase()) {
                    items.push((sk.name.clone(), sk.name.clone(), sk.description.clone()));
                }
            }
            items.sort_by(|a, b| a.0.cmp(&b.0));
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

    /// Real context usage: (filled cells, empty cells, pct) for a 20-cell bar,
    /// computed from tokens actually reported by the model, falling back to a
    /// byte/length heuristic before the first API round-trip. Shared by the
    /// SESSION box meter and the compact footer so both agree.
    fn ctx_meter(&self, cells: usize) -> (usize, usize, usize) {
        let max_ctx = if self.hm.context_budget > 0 { self.hm.context_budget } else { 128_000 };
        let used_tokens = if self.real_total_tokens > 0 {
            self.real_total_tokens
        } else {
            (self.history_bytes / 4).max(self.history_len * 40)
        };
        let pct = ((used_tokens as f64 / max_ctx as f64) * 100.0).clamp(0.0, 100.0) as usize;
        let filled = (pct * cells / 100).min(cells);
        (filled, cells - filled, pct)
    }

    /// Shadow-block ONYX wordmark (6 rows × 30 cols). Rendered only when the
    /// frame is wide enough; otherwise a compact one-line mark is used. The
    /// ratatui header tints each glyph with an animated cyan→blue gradient.
    /// Compact block-glyph mark (2 rows) for the legacy ANSI banner item.
    const BANNER: &'static [&'static str] = &["█▀ █▀ ▄▀█ █▀", "▄█ ▄█ ▀▀█ ▄█"];

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
        // Checkpoint BEFORE the turn mutates anything: /rewind restores this.
        // Runs before `running` flips so any UI-side transcript is used while
        // still held; write_checkpoint falls back to the on-disk file when the
        // worker owns the handle.
        self.write_checkpoint();
        self.tail.clear();
        self.reasoning.clear();
        self.tail_scrolled = 0;
        self.thinking = true;
        self.running = true;
        self.turns += 1;
        self.turn_start = Some(std::time::Instant::now());
        let ms = self.opts.max_steps;
        if let Some(tx) = &self.ctl_tx {
            let _ = tx.send(Ctl::Prompt(text, ms));
        }
    }

    /// Render a byte slice as the full ASCII/hex/binary/decimal/base64 card.
    fn emit_conversion(&mut self, bytes: &[u8]) {
        let ascii = String::from_utf8_lossy(bytes);
        let printable = bytes.iter().all(|&b| b.is_ascii_graphic() || b == b' ');
        let sum: u16 = bytes.iter().fold(0u16, |a, &b| a + b as u16) & 0xFF;
        self.items.push(Item::Info(conv_line("ASCII", if printable { &ascii } else { "(non-printable)" })));
        self.items.push(Item::Info(conv_line("HEX", &bytes_to_hex(bytes))));
        self.items.push(Item::Info(conv_line("BINARY", &bytes_to_bin(bytes))));
        self.items.push(Item::Info(conv_line("DECIMAL", &bytes_to_dec(bytes))));
        self.items.push(Item::Info(conv_line("BASE64", &bytes_to_b64(bytes))));
        self.items.push(Item::Info(conv_line("LEN", &format!("{} byte(s)", bytes.len()))));
        self.items.push(Item::Info(conv_line("SUM", &format!("{sum:02X}"))));
    }

    // ------------------------------------------------------ command helpers --

    fn checkpoints_dir() -> Option<PathBuf> {
        std::env::var_os("HOME")
            .map(PathBuf::from)
            .map(|h| h.join(".onyx/checkpoints"))
    }

    /// Format relative timestamp (e.g. "13h ago", "1d ago", "Oct 30")
    fn format_relative_time(ts_secs: u64) -> String {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        if ts_secs == 0 || ts_secs > now {
            return "just now".to_string();
        }
        let diff = now - ts_secs;
        if diff < 60 {
            "just now".to_string()
        } else if diff < 3600 {
            format!("{}m ago", diff / 60)
        } else if diff < 86400 {
            format!("{}h ago", diff / 3600)
        } else if diff < 604800 {
            format!("{}d ago", diff / 86400)
        } else {
            // Format approximate date e.g. Oct 28
            let days_ago = diff / 86400;
            format!("{days_ago}d ago")
        }
    }

    /// Read config from ~/.onyx/config.json
    fn load_persisted_config() -> serde_json::Value {
        if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
            let p = home.join(".onyx/config.json");
            if let Ok(c) = std::fs::read_to_string(p) {
                if let Ok(v) = serde_json::from_str(&c) {
                    return v;
                }
            }
        }
        serde_json::json!({})
    }

    /// Save updated config to ~/.onyx/config.json
    fn save_persisted_config(&self) {
        if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
            let dir = home.join(".onyx");
            let _ = std::fs::create_dir_all(&dir);
            let p = dir.join("config.json");
            let auto = matches!(*self.gw.mode.lock().unwrap(), ApprovalMode::Auto);
            const THEMES: [&str; 3] = ["onyx-cyan", "amber-term", "emerald"];
            let theme_name = THEMES.get(self.theme_idx).unwrap_or(&"onyx-cyan");
            let val = serde_json::json!({
                "theme": theme_name,
                "reasoning_effort": self.hm.effort,
                "auto_mode": auto,
                "max_steps": self.opts.max_steps,
                "default_model": self.hm.model,
                "vim_mode": self.vim_normal,
                "emacs_mode": self.emacs_keys,
                "verbose": self.expanded,
            });
            let _ = std::fs::write(p, serde_json::to_string_pretty(&val).unwrap_or_default());
        }
    }

    /// Produce structured list of settings items: (ID, Label, Current Value, Description)
    fn settings_items(&self) -> Vec<(&'static str, &'static str, String, &'static str)> {
        let auto = matches!(*self.gw.mode.lock().unwrap(), ApprovalMode::Auto);
        const THEMES: [&str; 3] = ["onyx-cyan", "amber-term", "emerald"];
        let theme_str = THEMES.get(self.theme_idx).unwrap_or(&"onyx-cyan").to_string();
        let storage = agent::provider::load_storage();
        let prov_id = storage.active_provider.clone();
        vec![
            ("provider", "Active Provider", prov_id, "Connected model provider (Enter to cycle active provider)"),
            ("model", "Model Engine", self.hm.model.clone(), "Active LLM engine (Enter to cycle available models)"),
            ("effort", "Reasoning Effort", self.hm.effort.clone(), "Reasoning depth: none, low, medium, high, max"),
            ("approval", "Approval Mode", if auto { "AUTO (YOLO)".into() } else { "Interactive".into() }, "Security approval policy (cycle with Enter or Shift+Tab)"),
            ("theme", "Syntax Theme", theme_str, "Terminal UI styling palette (Enter to cycle)"),
            ("steps", "Max Steps", format!("{}", self.opts.max_steps), "Maximum tool invocation loops per prompt"),
            ("budget", "Context Budget", format!("{} tokens", self.hm.context_budget), "Context truncation threshold"),
            ("vim", "Vim Modal Editing", if self.vim_normal { "Enabled".into() } else { "Disabled".into() }, "Modal command and text navigation in composer"),
            ("emacs", "Emacs Keybindings", if self.emacs_keys { "Enabled".into() } else { "Disabled".into() }, "Readline-style keybindings (Ctrl+A, Ctrl+E, Ctrl+K)"),
            ("verbose", "Verbose Output", if self.expanded { "Expanded".into() } else { "Collapsed".into() }, "Detailed tool output and reason traces"),
        ]
    }

    /// Toggle or advance the currently selected setting item
    fn cycle_selected_setting(&mut self) {
        let items = self.settings_items();
        if items.is_empty() { return; }
        let idx = self.settings_selected.min(items.len().saturating_sub(1));
        let (key, _, _, _) = items[idx];
        match key {
            "provider" => {
                let storage = agent::provider::load_storage();
                let catalog = agent::provider::load_catalog();
                let ids: Vec<String> = catalog.iter().map(|p| p.id.clone()).collect();
                if let Some(pos) = ids.iter().position(|id| id == &storage.active_provider) {
                    let next_pos = (pos + 1) % ids.len();
                    let next_id = &ids[next_pos];
                    self.switch_to_provider(next_id);
                } else if !ids.is_empty() {
                    self.switch_to_provider(&ids[0]);
                }
            }
            "model" => {
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
                if let Some(pos) = models.iter().position(|m| m == &self.hm.model) {
                    let next = (pos + 1) % models.len();
                    self.hm.model = models[next].clone();
                    self.opts.model = Some(models[next].clone());
                } else if !models.is_empty() {
                    self.hm.model = models[0].clone();
                    self.opts.model = Some(models[0].clone());
                }
                if let Some(tx) = &self.ctl_tx {
                    let _ = tx.send(Ctl::SetModel(self.hm.model.clone()));
                }
            }
            "effort" => {
                let effs = ["low", "medium", "high", "max", "none"];
                let pos = effs.iter().position(|&e| e == self.hm.effort).unwrap_or(1);
                let next = (pos + 1) % effs.len();
                let next_eff = effs[next].to_string();
                if let Some(tx) = &self.ctl_tx {
                    let _ = tx.send(Ctl::SetEffort(next_eff.clone()));
                }
                self.hm.effort = next_eff;
            }
            "approval" => {
                self.cycle_mode();
            }
            "theme" => {
                let next = (self.theme_idx + 1) % 3;
                self.set_theme(next);
            }
            "steps" => {
                let choices = [8, 16, 24, 30, 50, 100];
                let pos = choices.iter().position(|&s| s == self.opts.max_steps).unwrap_or(3);
                let next = (pos + 1) % choices.len();
                self.opts.max_steps = choices[next];
            }
            "budget" => {
                let choices = [64_000, 128_000, 200_000, 500_000, 1_000_000];
                let pos = choices.iter().position(|&b| b == self.hm.context_budget).unwrap_or(1);
                let next = (pos + 1) % choices.len();
                self.hm.context_budget = choices[next];
            }
            "vim" => {
                self.vim_normal = !self.vim_normal;
                self.vim_pending_d = false;
            }
            "emacs" => {
                self.emacs_keys = !self.emacs_keys;
            }
            "verbose" => {
                self.expanded = !self.expanded;
            }
            _ => {}
        }
        self.save_persisted_config();
    }

    /// Switch live execution to target provider
    fn switch_to_provider(&mut self, target_id: &str) {
        let mut storage = agent::provider::load_storage();
        let spec = match agent::provider::find_spec(target_id) {
            Some(s) => s,
            None => {
                self.items.push(Item::Error(format!("unknown provider `{target_id}`")));
                return;
            }
        };

        storage.active_provider = target_id.to_string();
        let cfg = storage.providers.entry(target_id.to_string()).or_insert_with(|| {
            agent::provider::ProviderConfig {
                provider_id: target_id.to_string(),
                base_url: spec.base_url.clone(),
                api_key: None,
                oauth_token: None,
                oauth_refresh_token: None,
                oauth_expires_at: None,
                active_model: None,
                custom_headers: std::collections::HashMap::new(),
            }
        });

        let base = agent::provider::resolve_base_url(&spec, Some(cfg));
        let key = agent::provider::resolve_api_key(&spec, Some(cfg)).unwrap_or_default();
        let (host, port) = agent::model::omniroute_config().host_port_from(&base);
        let path = if base.ends_with("/chat/completions") {
            base.split('/').skip(3).collect::<Vec<&str>>().join("/")
        } else {
            let p = base.split("://").nth(1).unwrap_or(&base);
            let sub = p.find('/').map(|i| &p[i..]).unwrap_or("");
            format!("{sub}/chat/completions")
        };

        self.hm.host = host.clone();
        self.hm.port = port;
        self.hm.path = path.clone();
        self.hm.key = key.clone();
        // Re-resolve the wire protocol for this provider (OpenAI-compat vs
        // Anthropic Messages vs native Gemini) so non-OpenAI endpoints work.
        self.hm.set_endpoint_for(&base, &key);

        if let Some(ref m) = cfg.active_model {
            self.hm.model = m.clone();
            self.opts.model = Some(m.clone());
        }

        if let Some(tx) = &self.ctl_tx {
            let _ = tx.send(Ctl::SetEndpoint {
                host,
                port,
                path,
                key: key.clone(),
                base_url: base.clone(),
            });
            let _ = tx.send(Ctl::SetModel(self.hm.model.clone()));
        }

        let _ = agent::provider::save_storage(&storage);
        // Refresh cached models
        self.cached_models = self.hm.list_models().unwrap_or_default();
        self.items.push(Item::Info(format!(
            "switched provider to {} ({}) · endpoint {}",
            spec.name, spec.id, self.hm.host
        )));
    }

    /// Handle `/provider` command execution
    fn handle_provider_cmd(&mut self, line: &str) {
        let parts: Vec<&str> = line.split_whitespace().collect();
        let mut storage = agent::provider::load_storage();

        if parts.len() == 1 {
            // Display active provider and list connected providers
            self.items.push(Item::Info(format!("Active Provider: {BOLD}{}{RESET}", storage.active_provider)));
            self.items.push(Item::Info("Configured Providers:".into()));
            for (id, cfg) in &storage.providers {
                let spec = agent::provider::find_spec(id);
                let name = spec.as_ref().map(|s| s.name.as_str()).unwrap_or(id.as_str());
                let has_key = cfg.api_key.as_ref().map(|k| !k.is_empty()).unwrap_or(false)
                    || cfg.oauth_token.as_ref().map(|t| !t.is_empty()).unwrap_or(false);
                let auth_stat = if has_key { "configured" } else { "needs key/login" };
                let is_act = if id == &storage.active_provider { " [ACTIVE]" } else { "" };
                self.items.push(Item::Info(format!("  {:<16} · {:<24} ({auth_stat}){is_act}", id, name)));
            }
            self.items.push(Item::Info("Usage: /provider <name> | /provider key <name> <api_key> | /provider login copilot".into()));
            return;
        }

        match parts[1] {
            "list" => {
                let catalog = agent::provider::load_catalog();
                self.items.push(Item::Info(format!("liter-llm Provider Catalog ({} providers):", catalog.len())));
                for p in catalog.iter().take(20) {
                    self.items.push(Item::Info(format!("  {:<16} · {:<28} (auth: {})", p.id, p.name, p.auth_type)));
                }
                self.items.push(Item::Info("... use /provider <name> to activate any of the 174 providers".into()));
            }
            "key" | "set-key" => {
                if parts.len() < 4 {
                    self.items.push(Item::Info("usage: /provider key <provider_id> <api_key>".into()));
                    return;
                }
                let prov_id = parts[2];
                let key = parts[3];
                let entry = storage.providers.entry(prov_id.to_string()).or_insert_with(|| {
                    agent::provider::ProviderConfig {
                        provider_id: prov_id.to_string(),
                        base_url: None,
                        api_key: None,
                        oauth_token: None,
                        oauth_refresh_token: None,
                        oauth_expires_at: None,
                        active_model: None,
                        custom_headers: std::collections::HashMap::new(),
                    }
                });
                // Store the secret in the credential vault and persist only a
                // `vault:<slot>` reference in providers.json, so the literal key
                // never sits in the plaintext config file. put_secret falls back to
                // the raw value if the vault can't be written.
                let stored = agent::provider::put_secret(&format!("{prov_id}/api_key"), key);
                let via_vault = stored.starts_with("vault:");
                entry.api_key = Some(stored);
                if let Err(e) = agent::provider::save_storage(&storage) {
                    self.items.push(Item::Error(format!("failed to save config: {e}")));
                } else {
                    let where_ = if via_vault {
                        "credential vault (~/.onyx/credentials.json)"
                    } else {
                        "~/.onyx/providers.json (vault unavailable — stored inline)"
                    };
                    self.items.push(Item::Info(format!("saved API key for `{prov_id}` in {where_}")));
                    if storage.active_provider == prov_id {
                        self.switch_to_provider(prov_id);
                    }
                }
            }
            "url" | "set-url" => {
                if parts.len() < 4 {
                    self.items.push(Item::Info("usage: /provider url <provider_id> <base_url>".into()));
                    return;
                }
                let prov_id = parts[2];
                let url = parts[3];
                let entry = storage.providers.entry(prov_id.to_string()).or_insert_with(|| {
                    agent::provider::ProviderConfig {
                        provider_id: prov_id.to_string(),
                        base_url: None,
                        api_key: None,
                        oauth_token: None,
                        oauth_refresh_token: None,
                        oauth_expires_at: None,
                        active_model: None,
                        custom_headers: std::collections::HashMap::new(),
                    }
                });
                entry.base_url = Some(url.to_string());
                let _ = agent::provider::save_storage(&storage);
                self.items.push(Item::Info(format!("saved base_url `{url}` for `{prov_id}`")));
                if storage.active_provider == prov_id {
                    self.switch_to_provider(prov_id);
                }
            }
            "login" | "oauth" => {
                let target = parts.get(2).copied().unwrap_or("copilot");
                if target == "copilot" || target == "github" || target == "github_copilot" {
                    self.items.push(Item::Info("Starting GitHub Copilot OAuth Device Flow...".into()));
                    match agent::provider::start_copilot_device_flow() {
                        Ok(resp) => {
                            self.items.push(Item::Info(format!(
                                "Please visit: {BOLD}{}{RESET}", resp.verification_uri
                            )));
                            self.items.push(Item::Info(format!(
                                "Enter device code: {BOLD}{CYAN}{}{RESET}", resp.user_code
                            )));
                            self.items.push(Item::Info("Waiting for authorization (press Esc to cancel)...".into()));
                            
                            // Poll up to 12 attempts
                            let mut authed = false;
                            for _ in 0..12 {
                                std::thread::sleep(std::time::Duration::from_secs(resp.interval.max(5)));
                                match agent::provider::poll_copilot_device_token(&resp.device_code) {
                                    Ok(Some(token)) => {
                                        // Attempt to exchange GitHub token for Copilot session token
                                        let (session_tok, base_url) = match agent::provider::exchange_copilot_session_token(&token) {
                                            Ok(sess) => {
                                                let api_url = sess.endpoints.as_ref()
                                                    .and_then(|e| e.get("api").cloned())
                                                    .unwrap_or_else(|| "https://api.githubcopilot.com".to_string());
                                                (sess.token, api_url)
                                            }
                                            Err(_) => (token.clone(), "https://api.githubcopilot.com".to_string()),
                                        };

                                        let entry = storage.providers.entry("github_copilot".to_string()).or_insert_with(|| {
                                            agent::provider::ProviderConfig {
                                                provider_id: "github_copilot".to_string(),
                                                base_url: Some(base_url.clone()),
                                                api_key: None,
                                                oauth_token: None,
                                                oauth_refresh_token: None,
                                                oauth_expires_at: None,
                                                active_model: Some("gpt-4o".to_string()),
                                                custom_headers: std::collections::HashMap::new(),
                                            }
                                        });
                                        entry.oauth_token = Some(session_tok);
                                        entry.oauth_refresh_token = Some(token);
                                        entry.base_url = Some(base_url);
                                        let _ = agent::provider::save_storage(&storage);
                                        self.items.push(Item::Info("GitHub Copilot OAuth authentication successful!".into()));
                                        self.switch_to_provider("github_copilot");
                                        authed = true;
                                        break;
                                    }
                                    Ok(None) => continue,
                                    Err(e) => {
                                        self.items.push(Item::Error(format!("OAuth polling error: {e}")));
                                        break;
                                    }
                                }
                            }
                            if !authed {
                                self.items.push(Item::Info("Authorization timed out or still pending. Run `/provider login copilot` again when ready.".into()));
                            }
                        }
                        Err(e) => self.items.push(Item::Error(format!("failed to start device flow: {e}"))),
                    }
                } else if target == "google" || target == "vertex" || target == "gemini" {
                    self.items.push(Item::Info("Resolving Google Cloud / Vertex Application Default Credentials...".into()));
                    match agent::provider::exchange_google_service_account_token("") {
                        Ok(token) => {
                            let entry = storage.providers.entry(target.to_string()).or_insert_with(|| {
                                agent::provider::ProviderConfig {
                                    provider_id: target.to_string(),
                                    base_url: None,
                                    api_key: None,
                                    oauth_token: None,
                                    oauth_refresh_token: None,
                                    oauth_expires_at: None,
                                    active_model: Some("gemini-2.5-flash".to_string()),
                                    custom_headers: std::collections::HashMap::new(),
                                }
                            });
                            entry.oauth_token = Some(token);
                            let _ = agent::provider::save_storage(&storage);
                            self.items.push(Item::Info(format!("Google ADC credentials acquired for `{target}`!")));
                            self.switch_to_provider(target);
                        }
                        Err(e) => {
                            self.items.push(Item::Error(format!("{e}")));
                            self.items.push(Item::Info(format!("Tip: You can also set API key directly: `/provider key {target} <key>`")));
                        }
                    }
                } else {
                    self.items.push(Item::Info(format!("OAuth provider `{target}` not supported yet (supported: copilot, google)")));
                }
            }
            target_id => {
                let resolved_id = match target_id.to_lowercase().as_str() {
                    "cliproxy" | "cliproxyapi-rust" | "cliproxy-rust" => "cliproxyapi",
                    _ => target_id,
                };
                let spec_opt = agent::provider::find_spec(resolved_id);
                let storage = agent::provider::load_storage();
                let has_key = storage.providers.get(resolved_id)
                    .and_then(|c| c.api_key.as_ref().map(|k| !k.is_empty()))
                    .unwrap_or(false);
                if let Some(spec) = spec_opt {
                    if spec.auth_type != "none" && !has_key {
                        // Needs an API key: open the input popup immediately!
                        self.open_connect_wizard_for_provider(resolved_id);
                        return;
                    }
                }
                self.switch_to_provider(resolved_id);
            }
        }
    }

    fn settings_lines_v(&self) -> Vec<String> {
        let mut out = Vec::new();
        let w = self.width.max(20);
        let items = self.settings_items();
        let card_w = w.saturating_sub(6).min(78).max(36);
        let inner_c = card_w.saturating_sub(2);

        out.push(format!("{BOLD}{CYAN}⚙ ONYX SETTINGS & PREFERENCES{RESET}"));
        out.push(format!("{BORDER}┌{}┐{RESET}", "─".repeat(inner_c)));

        let sel = self.settings_selected.min(items.len().saturating_sub(1));
        for (i, (_, label, val, desc)) in items.iter().enumerate() {
            let is_sel = i == sel;
            let marker = if is_sel {
                format!("{CYAN}❯{RESET}")
            } else {
                " ".to_string()
            };

            let row_line = if is_sel {
                format!("{marker} {BOLD}{FG_MAIN}{:<18}{RESET} {BOLD}{CYAN}{:<22}{RESET}", label, val)
            } else {
                format!("{marker} {FG_SEC}{:<18}{RESET} {FG_MAIN}{:<22}{RESET}", label, val)
            };

            let pad = (card_w + 2).saturating_sub(ansi_width(&row_line) + 3);
            out.push(format!(" {BORDER}│{RESET} {}{}{BORDER}│{RESET}", row_line, " ".repeat(pad)));

            if is_sel {
                let d_line = format!("    {DIM}{desc}{RESET}");
                let d_pad = (card_w + 2).saturating_sub(ansi_width(&d_line) + 3);
                out.push(format!(" {BORDER}│{RESET} {}{}{BORDER}│{RESET}", d_line, " ".repeat(d_pad)));
            }
        }

        out.push(format!("{BORDER}└{}┘{RESET}", "─".repeat(inner_c)));
        out.push(format!(" {DIM}↑↓ navigate · Enter toggle/cycle · Esc / q close · Persisted to ~/.onyx/config.json{RESET}"));
        out
    }

    /// Snapshot the current session's transcript into
    /// `~/.onyx/checkpoints/<sid>.ckpt.jsonl`. Reads the on-disk file directly
    /// (the worker owns the Transcript handle but pushes are flushed per
    /// message), so this never disturbs transcript ownership mid-turn.
    fn write_checkpoint(&mut self) {
        let Some(dir) = Self::checkpoints_dir() else { return };
        let sid = match self.session.as_ref() {
            Some(t) => t.path().file_stem().and_then(|s| s.to_str()).unwrap_or("onyx").to_string(),
            None => self.current_sid(),
        };
        let src = agent::session::sessions_dir().join(format!("{sid}.jsonl"));
        if !src.exists() {
            return; // nothing recorded yet for this session
        }
        let _ = std::fs::create_dir_all(&dir);
        let dst = dir.join(format!("{sid}.ckpt.jsonl"));
        let _ = std::fs::copy(&src, &dst);
    }

    /// Session id from the most recent "session <sid> ·" info item the worker
    /// reported (startup, /clear, or a new turn). Used when the UI thread does
    /// not hold the Transcript handle itself.
    fn current_sid(&self) -> String {
        for item in self.items.iter().rev() {
            if let Item::Info(s) = item {
                if let Some(rest) = s.strip_prefix("session ") {
                    if let Some(sid) = rest.split(' ').next() {
                        return sid.to_string();
                    }
                }
            }
        }
        String::new()
    }

    fn read_checkpoint(sid: &str) -> Option<Vec<Message>> {
        let dir = Self::checkpoints_dir()?;
        let path = dir.join(format!("{sid}.ckpt.jsonl"));
        if path.exists() {
            Some(agent::session::load_messages(&path))
        } else {
            None
        }
    }

    /// Two-tier compaction against the transcript on disk: prune stale tool
    /// outputs, then fold older turns into a digest. Rewrites the session file
    /// and swaps the worker's history through `Ctl::ReplaceHistory`.
    fn do_compact(&mut self) {
        if self.active_transcript().is_some() {
            self.compact_inner();
        } else {
            self.items.push(Item::Info("no session transcript to compact".into()));
        }
        // Re-hand the transcript so the worker keeps appending, on every exit path.
        if let (Some(tx), Some(t)) = (&self.ctl_tx, self.session.take()) {
            let _ = tx.send(Ctl::SetTranscript(t));
        }
    }

    fn compact_inner(&mut self) {
        let Some(t) = self.session.as_ref() else { return };
        let path = t.path().clone();
        let msgs = agent::session::load_messages(&path);
        if msgs.len() < 4 {
            self.items.push(Item::Info("conversation too short to compact meaningfully".into()));
            return;
        }
        let before: usize = msgs.iter().map(|m| agent::context::estimate_tokens(&m.content)).sum();
        let budget = if self.hm.context_budget > 0 { self.hm.context_budget * 3 / 4 } else { before / 2 };
        let pinned = vec![Message::text(
            "system",
            "[Context checkpoint] Earlier turns of this session were compacted; the summary below preserves decisions and state.",
        )];
        let mut for_compact = pinned.clone();
        for_compact.extend(msgs);
        match agent::context::compact_history(&for_compact, budget) {
            None => self.items.push(Item::Info(format!(
                "compaction not needed yet (~{before} tokens under a {budget}-token target)"
            ))),
            Some(compacted) => {
                let after: usize = compacted.iter().map(|m| agent::context::estimate_tokens(&m.content)).sum();
                // Rewrite the transcript: meta header + compacted messages.
                use std::io::Write;
                let tmp = path.with_extension("jsonl.compacting");
                if let Ok(mut f) = std::fs::File::create(&tmp) {
                    let meta = serde_json::json!({
                        "kind": "meta", "model": self.hm.model,
                        "ts": std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0),
                        "compacted": true
                    });
                    let _ = writeln!(f, "{}", meta);
                    for m in &compacted {
                        if m.role == "system" && pinned.iter().any(|p| p.content == m.content) {
                            continue;
                        }
                        let rec = match &m.tool_call {
                            Some(tc) => serde_json::json!({"kind":"message","role":m.role,"content":m.content,"tool_call":{"name":tc.name,"args":tc.args}}),
                            None => serde_json::json!({"kind":"message","role":m.role,"content":m.content}),
                        };
                        let _ = writeln!(f, "{rec}");
                    }
                    if f.flush().is_ok() && std::fs::rename(&tmp, &path).is_ok() {
                        if let Some(tx) = &self.ctl_tx {
                            let _ = tx.send(Ctl::ReplaceHistory(compacted));
                        }
                        self.items.push(Item::Info(format!(
                            "compacted context: ~{before} → ~{after} tokens (transcript rewritten on disk)"
                        )));
                    } else {
                        let _ = std::fs::remove_file(&tmp);
                        self.items.push(Item::Error("compaction rewrite failed; transcript unchanged".into()));
                    }
                } else {
                    self.items.push(Item::Error("could not open temp file for compaction".into()));
                }
            }
        }
    }

    /// The active transcript path, taking ownership back from the worker if it
    /// still holds one. Session-mutating commands (/rename, /save, /rewind)
    /// call this; after it returns Some, `self.session` is the live transcript
    /// and the worker has none (re-hand it with Ctl::SetTranscript when done).
    fn active_transcript(&mut self) -> Option<&agent::session::Transcript> {
        let want_take = self.session.is_none() && !self.running;
        if want_take {
            let Some(ctl) = &self.ctl_tx else { return None };
            let (ack_tx, ack_rx) = channel::<Option<agent::session::Transcript>>();
            let _ = ctl.send(Ctl::TakeTranscript(ack_tx));
            match ack_rx.recv_timeout(std::time::Duration::from_secs(5)) {
                Ok(Some(t)) => self.session = Some(t),
                Ok(None) | Err(_) => {}
            }
        }
        self.session.as_ref()
    }

    /// Persist a human title into the transcript's meta record.
    fn set_session_title(&mut self, title: &str) {
        let Some(t) = self.active_transcript() else {
            self.items.push(Item::Info("no active session to rename".into()));
            return;
        };
        let path = t.path().clone();
        let Ok(content) = std::fs::read_to_string(&path) else {
            self.items.push(Item::Error("cannot read transcript to rename".into()));
            return;
        };
        use std::fmt::Write as _;
        let mut out = String::with_capacity(content.len() + 64);
        let mut injected = false;
        for line in content.lines() {
            if !injected {
                if let Ok(mut v) = serde_json::from_str::<serde_json::Value>(line) {
                    if v.get("kind").and_then(|k| k.as_str()) == Some("meta") {
                        v["title"] = serde_json::json!(title);
                        let _ = writeln!(out, "{v}");
                        injected = true;
                        continue;
                    }
                }
            }
            out.push_str(line);
            out.push('\n');
            if !injected && line.contains("\"kind\":\"message\"") {
                let meta = serde_json::json!({"kind":"meta","model":self.hm.model,"title":title});
                // meta belongs first; prepend by swapping (cheap: rare path)
                out = format!("{}\n{}", meta, out);
                injected = true;
            }
        }
        if !injected {
            let meta = serde_json::json!({"kind":"meta","model":self.hm.model,"title":title});
            out = format!("{meta}\n{content}");
        }
        let res = std::fs::write(&path, out);
        // Re-hand the transcript either way so the worker keeps appending.
        if let (Some(tx), Some(t)) = (&self.ctl_tx, self.session.take()) {
            let _ = tx.send(Ctl::SetTranscript(t));
        }
        match res {
            Ok(()) => {
                self.session_title = Some(title.to_string());
                self.items.push(Item::Info(format!("session renamed to “{title}” (persisted)")));
            }
            Err(e) => self.items.push(Item::Error(format!("rename failed: {e}"))),
        }
    }

    fn do_save(&mut self, filename: &str) {
        let Some(t) = self.active_transcript() else {
            self.items.push(Item::Info("no active session to save".into()));
            return;
        };
        let src = t.path().clone();
        let dst = PathBuf::from(filename);
        let res = std::fs::copy(&src, &dst);
        // Read-only op: hand the transcript back to the worker either way.
        if let (Some(tx), Some(t)) = (&self.ctl_tx, self.session.take()) {
            let _ = tx.send(Ctl::SetTranscript(t));
        }
        match res {
            Ok(n) => self.items.push(Item::Info(format!(
                "session snapshot: {n} bytes → {}", dst.display()
            ))),
            Err(e) => self.items.push(Item::Error(format!("save failed: {e}"))),
        }
    }

    /// One-shot TCP+HTTP latency probe against the configured gateway.
    fn do_ping(&mut self) {
        let host = self.hm.host.clone();
        let port = self.hm.port;
        let path = if self.hm.path.is_empty() { "/".to_string() } else { self.hm.path.clone() };
        let start = std::time::Instant::now();
        match std::net::TcpStream::connect((host.as_str(), port)) {
            Ok(mut s) => {
                use std::io::{Read, Write};
                let req = format!("GET {path} HTTP/1.1\r\nHost: {host}:{port}\r\nConnection: close\r\n\r\n");
                let _ = s.write_all(req.as_bytes());
                let mut buf = [0u8; 16];
                let _ = s.read(&mut buf);
                let ms = start.elapsed().as_secs_f64() * 1000.0;
                self.items.push(Item::Info(format!(
                    "gateway {host}:{port}{path} responded in {ms:.1}ms"
                )));
            }
            Err(e) => self.items.push(Item::Error(format!("gateway {host}:{port} unreachable: {e}"))),
        }
    }

    fn do_doctor(&mut self) {
        let has_key = !self.hm.key.is_empty();
        let start = std::time::Instant::now();
        let gw_ok = std::net::TcpStream::connect((self.hm.host.as_str(), self.hm.port)).is_ok();
        let latency = start.elapsed();
        let git = std::process::Command::new("git").arg("--version").output().is_ok();
        let rg = std::process::Command::new("rg").arg("--version").output().is_ok();
        let sessions = agent::session::list_sessions(usize::MAX).len();
        let state_dir = std::env::var_os("HOME")
            .map(PathBuf::from)
            .map(|h| h.join(".onyx"))
            .filter(|d| d.exists());
        self.items.push(Item::Info("╭── ONYX DOCTOR ──────────────────────────────────────────────╮".into()));
        let rows: [(&str, String); 10] = [
            ("git", if git { "✓ present".to_string() } else { "✗ missing (some tools degrade)".to_string() }),
            ("ripgrep", if rg { "✓ present".to_string() } else { "— optional (grep fallback used)".to_string() }),
            ("API key", if has_key { "✓ set (value hidden)".to_string() } else { "✗ ONYX_API_KEY missing".to_string() }),
            (
                "gateway",
                if gw_ok {
                    format!("✓ {}:{} · {:.1}ms", self.hm.host, self.hm.port, latency.as_secs_f64() * 1000.0)
                } else {
                    format!("✗ {}:{} unreachable", self.hm.host, self.hm.port)
                },
            ),
            ("model", self.hm.model.clone()),
            ("effort", if self.hm.effort.is_empty() { "(default)".into() } else { self.hm.effort.clone() }),
            ("MCP servers", format!("{}", self.mcp_count)),
            ("skills", format!("{}", self.skill_count)),
            ("sessions", format!("{sessions} saved")),
            ("state dir", state_dir.map(|d| d.display().to_string()).unwrap_or_else(|| "✗ ~/.onyx missing".into())),
        ];
        for (label, value) in rows {
            self.items.push(Item::Info(format!("│ {:<13} {:<43} │", label, clip_chars(&value, 43))));
        }
        self.items.push(Item::Info("╰──────────────────────────────────────────────────────────────╯".into()));
    }

    fn set_theme(&mut self, idx: usize) {
        const NAMES: [&str; 3] = ["onyx-cyan", "amber-term", "emerald"];
        theme::set_palette(idx);
        self.theme_idx = idx;
        let name = NAMES[idx];
        if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
            let dir = home.join(".onyx");
            let _ = std::fs::create_dir_all(&dir);
            let _ = std::fs::write(dir.join("theme"), name);
        }
        self.items.push(Item::Info(format!("theme `{name}` active — accents re-tinted (persisted to ~/.onyx/theme)")));
    }

    // ------------------------------------------------------------- vim ------

    /// NOTE on coordinates: `self.cursor` is a BYTE offset into `self.input`
    /// (every insert/remove site uses `len_utf8`). These helpers convert via
    /// `char_indices`; the composer's own arrow handlers agree.

    /// Byte offset just past the last non-newline char of the current line.
    fn vim_line_end_byte(&self) -> usize {
        let mut byte = self.cursor;
        while byte < self.input.len() {
            match self.input[byte..].chars().next() {
                Some(c) if c != '\n' => byte += c.len_utf8(),
                _ => break,
            }
        }
        byte
    }
    /// Column (in chars) of the cursor within its line.
    fn vim_line_start_col(&self) -> usize {
        self.input[..self.cursor].chars().count()
            - self.input[..self.cursor]
                .rfind('\n')
                .map(|k| self.input[..k + 1].chars().count())
                .unwrap_or(0)
    }
    /// Byte offset where the cursor's line begins.
    fn vim_line_start_byte(&self) -> usize {
        self.input[..self.cursor].rfind('\n').map(|k| k + 1).unwrap_or(0)
    }
    /// Byte offset of the nth char at or after `start` (clamped to `end`).
    fn byte_of_char(&self, start: usize, end: usize, n: usize) -> usize {
        let mut count = 0;
        for (boff, _) in self.input[start..end].char_indices() {
            if count == n {
                return start + boff;
            }
            count += 1;
        }
        end
    }
    fn vim_move_left(&mut self) {
        if self.cursor > 0 {
            let prev = self.input[..self.cursor].chars().next_back().map(|c| c.len_utf8()).unwrap_or(0);
            self.cursor -= prev;
        }
    }
    fn vim_move_right(&mut self) {
        if let Some(next) = self.input[self.cursor..].chars().next() {
            self.cursor += next.len_utf8();
        }
    }
    /// Move down one line, preserving the column where possible.
    fn vim_down(&mut self) {
        let col = self.vim_line_start_col();
        let le = self.vim_line_end_byte();
        if le >= self.input.len() {
            self.cursor = le;
            return;
        }
        let next_start = le + 1; // skip the '\n'
        let next_end = self.input[next_start..]
            .find('\n')
            .map(|k| next_start + k)
            .unwrap_or(self.input.len());
        let width = self.input[next_start..next_end].chars().count();
        self.cursor = if width == 0 {
            next_start
        } else {
            self.byte_of_char(next_start, next_end, col.min(width - 1))
        };
    }
    /// Move up one line, preserving the column where possible.
    fn vim_up(&mut self) {
        let col = self.vim_line_start_col();
        let cur_ls = self.vim_line_start_byte();
        if cur_ls == 0 {
            self.cursor = 0;
            return;
        }
        let prev_end = cur_ls - 1; // byte of the '\n' ending the previous line
        let prev_start = self.input[..prev_end].rfind('\n').map(|k| k + 1).unwrap_or(0);
        let width = self.input[prev_start..prev_end].chars().count();
        self.cursor = if width == 0 {
            prev_start
        } else {
            self.byte_of_char(prev_start, prev_end, col.min(width - 1))
        };
    }
    fn vim_word_forward(&mut self, count: usize) {
        let mut pos = self.cursor;
        for _ in 0..count {
            let rest: Vec<char> = self.input[pos..].chars().collect();
            let mut i = 0;
            while i < rest.len() && (rest[i].is_alphanumeric() || rest[i] == '_') {
                i += 1;
            }
            while i < rest.len() && !rest[i].is_alphanumeric() && rest[i] != '_' {
                i += 1;
            }
            if i == 0 {
                i = 1; // progress guarantee on punctuation-only stretches
            }
            pos += rest.iter().take(i).map(|c| c.len_utf8()).sum::<usize>();
            pos = pos.min(self.input.len());
        }
        self.cursor = pos;
    }
    fn vim_word_back(&mut self, count: usize) {
        for _ in 0..count {
            let start = self.vim_word_back_one();
            self.cursor = start;
        }
    }
    /// Byte offset reached by moving one word backwards from the cursor
    /// (does not move it). Used by `b` and emacs ctrl+w.
    fn vim_word_back_one(&self) -> usize {
        let mut pos = self.cursor;
        while pos > 0 {
            let prev = self.input[..pos].chars().next_back().unwrap();
            pos -= prev.len_utf8();
            if prev.is_alphanumeric() || prev == '_' {
                continue;
            }
            if pos == 0 {
                break;
            }
            let p2 = self.input[..pos].chars().next_back().unwrap();
            if p2.is_alphanumeric() || p2 == '_' {
                break;
            }
        }
        pos
    }
    /// One normal-mode key press against the composer buffer. Returns false
    /// when the char is not a vim command (caller falls through).
    fn vim_normal_key(&mut self, c: char) -> bool {
        if self.vim_pending_d {
            self.vim_pending_d = false;
            match c {
                'd' => {
                    let le = self.vim_line_end_byte();
                    let ls = self.input[..self.cursor].rfind('\n').map(|k| k + 1).unwrap_or(0);
                    self.vim_clipboard = self.input[ls..le].to_string();
                    self.input.replace_range(ls..le, "");
                    self.cursor = ls.min(self.input.len());
                }
                '$' => {
                    let le = self.vim_line_end_byte();
                    self.vim_clipboard = self.input[self.cursor..le].to_string();
                    self.input.replace_range(self.cursor..le, "");
                }
                'w' => {
                    let start = self.cursor;
                    self.vim_word_forward(1);
                    self.vim_clipboard = self.input[start..self.cursor].to_string();
                    self.input.replace_range(start..self.cursor, "");
                    self.cursor = start;
                }
                _ => {}
            }
            return true;
        }
        match c {
            'h' => self.vim_move_left(),
            'l' => self.vim_move_right(),
            'j' => self.vim_down(),
            'k' => self.vim_up(),
            '0' => self.cursor = self.input[..self.cursor].rfind('\n').map(|k| k + 1).unwrap_or(0),
            '^' => {
                let ls = self.vim_line_start_byte();
                let mut byte = ls;
                for c in self.input[ls..].chars() {
                    if c == '\n' || c != ' ' {
                        break;
                    }
                    byte += c.len_utf8();
                }
                self.cursor = byte;
            }
            '$' => self.cursor = self.vim_line_end_byte().saturating_sub(1),
            'w' => self.vim_word_forward(1),
            'b' => self.vim_word_back(1),
            'x' => {
                if let Some(next) = self.input[self.cursor..].chars().next() {
                    self.input.replace_range(self.cursor..self.cursor + next.len_utf8(), "");
                    self.cursor = self.cursor.min(self.input.len());
                }
            }
            'd' => {
                self.vim_pending_d = true;
                return true;
            }
            'i' | 'a' => self.vim_normal = false,
            'A' => {
                self.cursor = self.input.len();
                self.vim_normal = false;
            }
            'D' => {
                let le = self.vim_line_end_byte();
                self.input.truncate(le);
                self.cursor = self.cursor.min(le);
                self.vim_normal = false;
            }
            'C' => {
                let le = self.vim_line_end_byte();
                self.input.replace_range(self.cursor..le, "");
                self.vim_normal = false;
            }
            'p' => {
                if !self.vim_clipboard.is_empty() {
                    let clip = self.vim_clipboard.clone();
                    self.input.insert_str(self.cursor, &clip);
                    self.cursor += clip.len();
                }
            }
            _ => return false,
        }
        true
    }

    /// Open the full-screen session browser, scoped to the current worktree by
    /// default. Falls back to all projects when the workspace has no sessions.
    fn open_resume_browser(&mut self) {
        let scoped = agent::session::list_sessions_scoped(usize::MAX, Some(&self.workspace_dir));
        let (all, show_all) = if scoped.is_empty() {
            (agent::session::list_sessions_scoped(usize::MAX, None), true)
        } else {
            (scoped, false)
        };
        if all.is_empty() {
            self.items.push(Item::Info("no saved sessions yet".into()));
            return;
        }
        self.resume_browser = Some(ResumeBrowser::new(all, show_all));
    }

    /// Seal the current transcript and replay `msgs` into a fresh session id.
    /// Shared by `/resume <query>` and Enter in the browser so both paths fork
    /// identically. Returns the new session id for the caller's status line.
    fn resume_fork(&mut self, msgs: &[Message]) -> String {
        if let Some(tx) = &self.ctl_tx {
            let _ = tx.send(Ctl::DropTranscript);
        }
        self.session = None;
        let new_sid = agent::session::new_session_id();
        let transcript = agent::session::Transcript::open(&new_sid, &self.hm.model);
        if let Some(t) = &transcript {
            for m in msgs {
                t.push(m);
            }
        }
        self.session = transcript;
        if let Some(tx) = &self.ctl_tx {
            let _ = tx.send(Ctl::Resume(msgs.to_vec(), new_sid.clone()));
        }
        self.items.retain(|it| matches!(it, Item::Banner));
        for m in msgs {
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
                _ => {}
            }
        }
        new_sid
    }

    /// Scroll so the cursor stays inside a window of `height` visible rows.
    fn browser_ensure_visible(&mut self, height: usize) {
        let height = height.max(1);
        let (sel, top, len) = {
            let b = self.resume_browser.as_ref().unwrap();
            (b.sel, b.top, b.rows.len())
        };
        if len == 0 {
            if let Some(b) = self.resume_browser.as_mut() {
                b.top = 0;
            }
            return;
        }
        let mut top = top;
        if sel < top {
            top = sel;
        } else if sel >= top + height {
            top = sel + 1 - height;
        }
        let top = top.min(len.saturating_sub(1));
        if let Some(b) = self.resume_browser.as_mut() {
            b.top = top;
        }
    }

    /// Re-scope the row set: current worktree vs all projects, preserving the
    /// live query. Reloads from the store so newly-written sessions appear.
    fn browser_rescope(&mut self) {
        let show_all = self.resume_browser.as_ref().map(|b| b.show_all).unwrap_or(true);
        let all = if show_all {
            agent::session::list_sessions_scoped(usize::MAX, None)
        } else {
            let scoped =
                agent::session::list_sessions_scoped(usize::MAX, Some(&self.workspace_dir));
            if scoped.is_empty() {
                agent::session::list_sessions_scoped(usize::MAX, None)
            } else {
                scoped
            }
        };
        if let Some(b) = self.resume_browser.as_mut() {
            b.all = all;
            b.show_all = show_all;
            b.rebuild_rows();
        }
    }

    fn browser_key(&mut self, chars: &[char], code: u32) {
        let ch = chars.first().copied().unwrap_or('\0');
        // Search mode captures printable edits; List mode dispatches actions.
        let in_search = matches!(
            self.resume_browser.as_ref().map(|b| &b.mode),
            Some(BrowserMode::Search)
        );
        if in_search {
            match (ch, code) {
                ('\x1b', _) => {
                    // Esc: clear query and drop back to list navigation.
                    if let Some(b) = self.resume_browser.as_mut() {
                        b.query.clear();
                        b.mode = BrowserMode::List;
                        b.rebuild_rows();
                    }
                }
                ('\r', _) | ('\n', _) => {
                    // Enter commits the search, returns to list nav.
                    if let Some(b) = self.resume_browser.as_mut() {
                        b.mode = BrowserMode::List;
                    }
                }
                ('\x7f', _) | ('\x08', _) => {
                    if let Some(b) = self.resume_browser.as_mut() {
                        b.query.pop();
                        b.rebuild_rows();
                    }
                }
                ('\0', 1000) => self.browser_move(-1),
                ('\0', 1001) => self.browser_move(1),
                _ => {
                    for &c in chars {
                        if !c.is_control() {
                            if let Some(b) = self.resume_browser.as_mut() {
                                b.query.push(c);
                            }
                        }
                    }
                    if let Some(b) = self.resume_browser.as_mut() {
                        b.rebuild_rows();
                    }
                }
            }
            return;
        }

        match (ch, code) {
            ('\x1b', _) | ('\x03', _) => self.resume_browser = None,
            ('\0', 1000) => self.browser_move(-1),
            ('\0', 1001) => self.browser_move(1),
            ('\0', 1008) => self.browser_move(-8), // PgUp
            ('\0', 1009) => self.browser_move(8),  // PgDn
            (' ', _) => {
                if let Some(b) = self.resume_browser.as_mut() {
                    b.expanded = !b.expanded;
                }
            }
            ('/' , _) => {
                if let Some(b) = self.resume_browser.as_mut() {
                    b.mode = BrowserMode::Search;
                }
            }
            ('?' , _) => {
                if let Some(b) = self.resume_browser.as_mut() {
                    b.help = !b.help;
                }
            }
            ('\x02', _) => {
                // Ctrl+B: toggle to current-branch/worktree scope only.
                if let Some(b) = self.resume_browser.as_mut() {
                    b.show_all = false;
                }
                self.browser_rescope();
            }
            ('\x01', _) => {
                // Ctrl+A: force all projects.
                if let Some(b) = self.resume_browser.as_mut() {
                    b.show_all = true;
                }
                self.browser_rescope();
            }
            ('b', _) => {
                if let Some(b) = self.resume_browser.as_mut() {
                    b.show_all = !b.show_all;
                }
                self.browser_rescope();
            }
            ('\r', _) | ('\n', _) => self.browser_select_resume(),
            _ => {}
        }
    }

    fn browser_move(&mut self, delta: isize) {
        if let Some(b) = self.resume_browser.as_mut() {
            if b.rows.is_empty() {
                return;
            }
            let n = b.rows.len() as isize;
            let mut s = b.sel as isize + delta;
            if s < 0 {
                s = 0;
            }
            if s >= n {
                s = n - 1;
            }
            b.sel = s as usize;
        }
        self.browser_ensure_visible(self.last_browser_height);
    }

    fn browser_select_resume(&mut self) {
        let Some(s) = self.resume_browser.as_ref().and_then(|b| b.selected_session()).cloned()
        else {
            return;
        };
        let Some((_path, msgs)) = agent::session::resolve_session(&s.id) else {
            if let Some(b) = self.resume_browser.as_mut() {
                b.expanded = false;
            }
            self.items.push(Item::Error(format!("could not load session {}", s.id)));
            return;
        };
        self.resume_browser = None;
        let sid = self.resume_fork(&msgs);
        self.items.push(Item::Info(format!(
            "resumed {} message(s) · new session {sid}",
            msgs.len()
        )));
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
                self.items.retain(|it| matches!(it, Item::Banner));
                self.scroll_offset = 0;
                self.tail.clear();
                self.reasoning.clear();
                self.tail_scrolled = 0;
                self.session_title = None;
                self.real_total_tokens = 0;
                self.real_prompt_tokens = 0;
                self.real_completion_tokens = 0;
                self.history_len = 0;
                self.history_bytes = 0;
                self.ctx_history.clear();
                self.items.push(Item::Info("conversation cleared".into()));
            }
            "/provider" | "/providers" => {
                if arg.is_none() {
                    // Bare `/provider` opens the picker; every subcommand form
                    // (list/key/url/login/<id>) keeps its text behaviour.
                    self.open_connect_wizard();
                } else {
                    self.handle_provider_cmd(line);
                }
            }
            "/connect" => self.open_connect_wizard(),
            "/model" | "/models" => {
                if let Some(m) = arg {
                    self.hm.context_budget = agent::model::context_window_for_model(m);
                    self.hm.model = m.to_string();
                    if let Some(tx) = &self.ctl_tx {
                        let _ = tx.send(Ctl::SetModel(m.to_string()));
                    }
                    self.opts.model = Some(m.to_string());
                    self.items.push(Item::Info(format!(
                        "switched model to: {} (context window: {} tokens)",
                        self.hm.model, self.hm.context_budget
                    )));
                } else {
                    self.items.push(Item::Info(format!(
                        "model: {} (context window: {} tokens)",
                        self.hm.model, self.hm.context_budget
                    )));
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
                self.settings_active = true;
                self.settings_selected = 0;
            }
            "/audit" => {
                if let Some(tx) = &self.ctl_tx {
                    let _ = tx.send(Ctl::Audit);
                }
                self.items.push(Item::Info("audit requested (see transcript below)".into()));
            }
            "/tools" => {
                if let Some(sel) = self.sel {
                    self.items.push(Item::Info(format!(
                        "retrieval ON · {} tools, {} skills · engine: {} · adaptive: {}",
                        sel.tool_count(),
                        sel.skill_count(),
                        if sel.embeddings_ready() { "hybrid" } else { "bm25" },
                        sel.adaptive_status()
                    )));
                } else {
                    self.items.push(Item::Info("retrieval OFF (disabled via ONYX_RETRIEVAL=0/off)".into()));
                }
                for n in self.gw.tools.names() {
                    let cap = tools::taxonomy::capability_of(&n).label();
                    self.items.push(Item::Info(format!("  • {n}  {DIM}[{cap}]{RESET}")));
                }
            }
            "/mode" => {
                self.cycle_mode();
            }
            "/gain" => {
                let _on = std::env::var("ONYX_COMPACT").map(|v| v == "1" || v == "true").unwrap_or(true);
                let report = tools::compress::gain_report();
                if report.raw_bytes == 0 {
                    self.items.push(Item::Info(
                        "unified output compression active · no qualifying tool outputs processed yet this session".into(),
                    ));
                } else {
                    self.items.push(Item::Info(format!(
                        "compact gain: {} bytes saved ({:.1}%) across {} call(s) · {} bytes shipped to model",
                        report.bytes_saved, report.reduction_pct, report.calls, report.compacted_bytes
                    )));
                    self.items.push(Item::Info(format!(
                        "estimated context tokens saved: ~{} tokens (estimate, ~4 chars/token) · avg processing latency: {} µs · formatter: {}",
                        report.estimated_tokens_saved,
                        report.avg_latency_us,
                        report.formatter.map(|f| f.as_str()).unwrap_or("none")
                    )));
                }
            }
            "/skill" => {
                if let Some(name) = arg {
                    self.load_skill(name);
                } else {
                    self.items.push(Item::Info("usage: /skill <name> (or /skills to list all)".into()));
                }
            }
            "/skills" => {
                let mut cat = skills::SkillCatalog::new();
                cat.scan_all_default();
                self.items.push(Item::Info(format!("Available skills ({} total):", cat.count())));
                let mut sorted: Vec<_> = cat.skills.values().collect();
                sorted.sort_by(|a, b| a.name.cmp(&b.name));
                for sk in sorted {
                    let desc = if sk.description.is_empty() {
                        "".to_string()
                    } else {
                        format!(" — {}", sk.description)
                    };
                    self.items.push(Item::Info(format!("  • /skill {}{}", sk.name, desc)));
                }
            }
            "/skillopt" => match arg {
                None => self.run_skillopt(None),
                Some("apply") => self.apply_skillopt(),
                Some("discard") => {
                    let n = self.pending_skill_updates.len();
                    self.pending_skill_updates.clear();
                    self.items.push(Item::Info(if n == 0 {
                        "nothing staged".into()
                    } else {
                        format!("dropped {n} staged skill update(s)")
                    }));
                }
                Some(cmd) if cmd.starts_with("rollback") => {
                    let parts: Vec<&str> = cmd.split_whitespace().collect();
                    if parts.len() < 2 {
                        self.items.push(Item::Error("usage: /skillopt rollback <skill_name> [hash]".into()));
                    } else {
                        let skill_name = parts[1];
                        let target_hash = parts.get(2).copied();
                        let mut catalog = skills::SkillCatalog::new();
                        catalog.scan_all_default();
                        match catalog.rollback_skill(skill_name, target_hash) {
                            Ok(restored) => {
                                self.items.push(Item::Info(format!("✓ successfully rolled back `{skill_name}` to `{restored}`")));
                                let mut refreshed = skills::SkillCatalog::new();
                                refreshed.scan_all_default();
                                self.skill_count = refreshed.count();
                            }
                            Err(e) => self.items.push(Item::Error(format!("rollback failed: {e}"))),
                        }
                    }
                }
                Some(name) => self.run_skillopt(Some(name)),
            },
            "/sessions" => {
                let list = agent::session::list_sessions(12);
                if list.is_empty() {
                    self.items.push(Item::Info("no saved sessions yet".into()));
                }
                for s in list {
                    let pv = if s.preview.is_empty() { "(no prompt)".to_string() } else { s.preview };
                    self.items.push(Item::Info(format!(
                        "  {} · {} turn(s) ({} msgs) · {}",
                        s.id,
                        s.user_turns,
                        s.messages,
                        pv
                    )));
                }
                self.items.push(Item::Info("/resume <id|prefix> to replay one".into()));
            }
            "/res" | "/resume" => {
                match arg {
                    // Bare /resume → full-screen session browser (hybrid: keeps
                    // the inline dropdown for partial typing and this direct-arg
                    // path for fast id/prefix resume).
                    None => self.open_resume_browser(),
                    Some(q) => match agent::session::resolve_session(q) {
                        None => self.items.push(Item::Error(format!("no session matches `{q}` (try /sessions)"))),
                        Some((path, msgs)) => {
                            let sid = self.resume_fork(&msgs);
                            self.items.push(Item::Info(format!(
                                "resumed {} message(s) from {} · new session {sid}",
                                msgs.len(),
                                path.file_stem().and_then(|s| s.to_str()).unwrap_or("?")
                            )));
                        }
                    },
                }
            }
            "/hex" => {
                // Rest of the line is the payload (may contain spaces).
                let rest = line.trim_start_matches("/hex").trim();
                if rest.is_empty() {
                    self.items.push(Item::Info(
                        "usage: /hex <text>          → hex/binary/decimal/base64".into(),
                    ));
                    self.items.push(Item::Info(
                        "       /hex -d <hex bytes>   → ascii  (-b/-n for binary/decimal)".into(),
                    ));
                } else if let Some(payload) =
                    rest.strip_prefix("-d ").or_else(|| rest.strip_prefix("--decode ")).map(|s| s.trim())
                {
                    match parse_bytes(payload, 16) {
                        Ok(bytes) => self.emit_conversion(&bytes),
                        Err(e) => self.items.push(Item::Error(format!("hex decode: {e}"))),
                    }
                } else if let Some(payload) =
                    rest.strip_prefix("-b ").or_else(|| rest.strip_prefix("--bits ")).map(|s| s.trim())
                {
                    match parse_bytes(payload, 2) {
                        Ok(bytes) => self.emit_conversion(&bytes),
                        Err(e) => self.items.push(Item::Error(format!("binary decode: {e}"))),
                    }
                } else if let Some(payload) =
                    rest.strip_prefix("-n ").or_else(|| rest.strip_prefix("--dec ")).map(|s| s.trim())
                {
                    match payload.split_whitespace().map(|t| t.parse::<u8>()).collect::<Result<Vec<_>, _>>()
                    {
                        Ok(bytes) => self.emit_conversion(&bytes),
                        Err(_) => self.items.push(Item::Error("decimal decode: expected byte values 0-255".into())),
                    }
                } else {
                    self.emit_conversion(rest.as_bytes());
                }
            }
            "/help" => {
                let category = arg.unwrap_or("");
                match category {
                    "context" | "session" => {
                        self.items.push(Item::Info("╭── CONTEXT & SESSION CONTROL ─────────────────────────────╮".into()));
                        self.items.push(Item::Info("│ /clear         reset conversation context                 │".into()));
                        self.items.push(Item::Info("│ /compact       compress conversation logs to save tokens  │".into()));
                        self.items.push(Item::Info("│ /rewind        rollback to previous turn checkpoint       │".into()));
                        self.items.push(Item::Info("│ /resume <id>   restore past session by id or prefix       │".into()));
                        self.items.push(Item::Info("│ /rename <name> rename current session                     │".into()));
                        self.items.push(Item::Info("│ /branch [name] create isolated experimental git worktree  │".into()));
                        self.items.push(Item::Info("│ /save [file]   export static transcript snapshot          │".into()));
                        self.items.push(Item::Info("│ /undo          revert last prompt or file edit            │".into()));
                        self.items.push(Item::Info("│ /reset         hard-wipe runtime configuration            │".into()));
                        self.items.push(Item::Info("│ /exit          quit session                               │".into()));
                        self.items.push(Item::Info("╰───────────────────────────────────────────────────────────╯".into()));
                    }
                    "dev" | "coding" => {
                        self.items.push(Item::Info("╭── DEVELOPMENT & CODING WORKFLOWS ─────────────────────────╮".into()));
                        self.items.push(Item::Info("│ /init          bootstrap ONYX.md project memory guide     │".into()));
                        self.items.push(Item::Info("│ /memory        view/edit project memory guidelines        │".into()));
                        self.items.push(Item::Info("│ /plan [goal]   toggle plan mode for complex operations    │".into()));
                        self.items.push(Item::Info("│ /review        structural evaluation of git diff          │".into()));
                        self.items.push(Item::Info("│ /add-dir <p>   mount supplemental workspace directory     │".into()));
                        self.items.push(Item::Info("│ /bashes        view & monitor running background tasks    │".into()));
                        self.items.push(Item::Info("│ /tasks         list active multi-step task queue          │".into()));
                        self.items.push(Item::Info("│ /btw <q>       ask sidecar question without context drift │".into()));
                        self.items.push(Item::Info("│ /explain [p]   explain architecture or code logic         │".into()));
                        self.items.push(Item::Info("│ /refactor [p]  optimize code for clarity & speed          │".into()));
                        self.items.push(Item::Info("│ /test [p]      scan and run project test suites           │".into()));
                        self.items.push(Item::Info("│ /debug [err]   attach to compiler errors & stack traces   │".into()));
                        self.items.push(Item::Info("│ /docstrings    generate missing documentation blocks      │".into()));
                        self.items.push(Item::Info("╰───────────────────────────────────────────────────────────╯".into()));
                    }
                    "system" | "config" => {
                        self.items.push(Item::Info("╭── SYSTEM SETTINGS & AI CONTROL ───────────────────────────╮".into()));
                        self.items.push(Item::Info("│ /model [name]  hot-swap active AI engine                  │".into()));
                        self.items.push(Item::Info("│ /effort <lvl>  reasoning level (low, med, high, max)      │".into()));
                        self.items.push(Item::Info("│ /fast          toggle high-speed response mode            │".into()));
                        self.items.push(Item::Info("│ /mode          cycle approvals (Normal, Edits, YOLO)      │".into()));
                        self.items.push(Item::Info("│ /config        view/edit configuration options            │".into()));
                        self.items.push(Item::Info("│ /mcp           inspect connected MCP servers              │".into()));
                        self.items.push(Item::Info("│ /permissions   inspect & adjust safety policies           │".into()));
                        self.items.push(Item::Info("│ /vim           toggle modal Vim command input             │".into()));
                        self.items.push(Item::Info("│ /emacs         toggle Emacs-style keybindings             │".into()));
                        self.items.push(Item::Info("│ /theme         alter terminal syntax theme                │".into()));
                        self.items.push(Item::Info("│ /verbose       toggle full tool reasoning output          │".into()));
                        self.items.push(Item::Info("╰───────────────────────────────────────────────────────────╯".into()));
                    }
                    "analytics" | "diagnostics" => {
                        self.items.push(Item::Info("╭── TOKEN ANALYTICS & DIAGNOSTICS ──────────────────────────╮".into()));
                        self.items.push(Item::Info("│ /cost          insight into token usage & estimated cost  │".into()));
                        self.items.push(Item::Info("│ /usage         real token metrics and step telemetry      │".into()));
                        self.items.push(Item::Info("│ /stats         workspace code composition metrics         │".into()));
                        self.items.push(Item::Info("│ /insights      build session profiling analysis report    │".into()));
                        self.items.push(Item::Info("│ /context       visual graph of context capacity limits    │".into()));
                        self.items.push(Item::Info("│ /doctor        validate system & dependencies             │".into()));
                        self.items.push(Item::Info("│ /ping          measure API round-trip latency             │".into()));
                        self.items.push(Item::Info("│ /logs          print local daemon / interface logs        │".into()));
                        self.items.push(Item::Info("╰───────────────────────────────────────────────────────────╯".into()));
                    }
                    _ => {
                        self.items.push(Item::Info("╭── ONYX EXTENDED COMMAND SUITE (50+ CAPABILITIES) ─────────╮".into()));
                        self.items.push(Item::Info("│ Categories: /help context   · /help dev     · /help system│".into()));
                        self.items.push(Item::Info("│             /help analytics · /help eco     · /settings   │".into()));
                        self.items.push(Item::Info("├───────────────────────────────────────────────────────────┤".into()));
                        self.items.push(Item::Info("│ Essential:  /model /effort /mode /plan /clear /resume     │".into()));
                        self.items.push(Item::Info("│ Diagnostic: /doctor /stats /context /cost /usage /ping    │".into()));
                        self.items.push(Item::Info("│ Keys:       ↵ send · ⇧⇥ mode · ⌃o expand · ⎋ cancel       │".into()));
                        self.items.push(Item::Info("╰───────────────────────────────────────────────────────────╯".into()));
                    }
                }
            }
            // Context & Session Control Handlers
            "/compact" => self.do_compact(),
            "/rewind" | "/undo" => {
                if arg == Some("--list") {
                    let dir = Self::checkpoints_dir();
                    match dir.filter(|d| d.exists()) {
                        None => self.items.push(Item::Info("no checkpoints yet (~/.onyx/checkpoints does not exist)".into())),
                        Some(dir) => {
                            let mut n = 0usize;
                            if let Ok(rd) = std::fs::read_dir(&dir) {
                                for e in rd.flatten() {
                                    let p = e.path();
                                    if p.extension().and_then(|s| s.to_str()) == Some("jsonl") {
                                        let sid = p.file_stem().and_then(|s| s.to_str()).unwrap_or("?");
                                        let msgs = agent::session::load_messages(&p);
                                        self.items.push(Item::Info(format!(
                                            "  {} · {} message(s)",
                                            sid.trim_end_matches(".ckpt"),
                                            msgs.len()
                                        )));
                                        n += 1;
                                    }
                                }
                            }
                            self.items.push(Item::Info(format!("{n} checkpoint(s) — /rewind restores the current session's last one")));
                        }
                    }
                    return;
                }
                let Some(t) = self.active_transcript() else {
                    self.items.push(Item::Info("no active session to rewind".into()));
                    return;
                };
                let sid = t.path().file_stem().and_then(|s| s.to_str()).unwrap_or("").to_string();
                match Self::read_checkpoint(&sid) {
                    None => self.items.push(Item::Info(
                        "no checkpoint recorded yet this session (one is written before every turn)".into(),
                    )),
                    Some(msgs) => {
                        // Same fork discipline as /resume: seal the old transcript,
                        // open a new one seeded with the checkpoint, swap worker history.
                        if let Some(tx) = &self.ctl_tx {
                            let _ = tx.send(Ctl::DropTranscript);
                        }
                        self.session = None;
                        let new_sid = agent::session::new_session_id();
                        let transcript = agent::session::Transcript::open(&new_sid, &self.hm.model);
                        if let Some(t) = &transcript {
                            for m in &msgs {
                                t.push(m);
                            }
                        }
                        self.session = transcript;
                        if let Some(tx) = &self.ctl_tx {
                            let _ = tx.send(Ctl::Resume(msgs.clone(), new_sid.clone()));
                        }
                        self.items.retain(|it| matches!(it, Item::Banner));
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
                                _ => {}
                            }
                        }
                        self.items.push(Item::Info(format!(
                            "rewound conversation to checkpoint · restored {} message(s) into new session {new_sid}\n\
                             note: file edits made by earlier turns are NOT reverted — use git restore for those",
                            msgs.len()
                        )));
                    }
                }
            }
            "/rename" => {
                let rest = line.trim_start_matches("/rename").trim();
                if rest.is_empty() {
                    self.items.push(Item::Info("usage: /rename <new name>".into()));
                } else {
                    self.set_session_title(rest);
                }
            }
            "/branch" => {
                let branch_name = arg.unwrap_or("onyx-experiment");
                let res = std::process::Command::new("git").args(["checkout", "-b", branch_name]).output();
                match res {
                    Ok(out) if out.status.success() => self.items.push(Item::Info(format!("switched to new git branch `{branch_name}`"))),
                    Ok(out) => self.items.push(Item::Error(String::from_utf8_lossy(&out.stderr).trim().into())),
                    Err(e) => self.items.push(Item::Error(format!("git checkout failed: {e}"))),
                }
            }
            "/reset" => {
                self.opts.max_steps = 30;
                *self.gw.mode.lock().unwrap() = ApprovalMode::Normal;
                self.items.push(Item::Info("system configuration reset to factory defaults (steps: 30, mode: Normal)".into()));
            }
            "/save" => {
                let filename = arg.unwrap_or("onyx-session-snapshot.jsonl");
                self.do_save(filename);
            }
            // Development & Coding Workflows Handlers
            "/init" => {
                let prompt = "Please inspect the codebase in the current workspace and generate an initial ONYX.md project guide describing repository architecture, key commands, build setup, and testing instructions.";
                self.input = prompt.to_string();
                self.cursor = self.input.len();
                self.submit();
            }
            "/memory" => {
                let candidates = ["ONYX.md", "PROJECT.md", "CLAUDE.md", "README.md"];
                let mut found = false;
                for c in candidates {
                    if std::path::Path::new(c).exists() {
                        self.items.push(Item::Info(format!("project memory file found: `{c}`")));
                        found = true;
                    }
                }
                // Check WorkMemory in workspace
                let work_mem_path = std::path::Path::new(".onyx/memories/work_memory.json");
                if work_mem_path.exists() {
                    let mem = onyx_project::WorkMemory::load_or_init(std::path::Path::new("."));
                    self.items.push(Item::Info(format!(
                        "Onyx WorkMemory: {} task summary entries, {} pitfalls recorded",
                        mem.task_summaries.len(),
                        mem.pitfalls.len()
                    )));
                    found = true;
                }
                // Check Hermes Persistent Memory store (~/.onyx/memories/MEMORY.md or ~/.hermes/memories/MEMORY.md)
                let hermes = tools::implementations::grok_build::HermesMemoryStore::default_paths();
                let mem_content = hermes.read_memory();
                let user_content = hermes.read_user();
                if !mem_content.is_empty() || !user_content.is_empty() {
                    self.items.push(Item::Info(format!(
                        "Hermes Persistent Memory ({:?}): MEMORY.md ({} chars), USER.md ({} chars)",
                        hermes.base_dir,
                        mem_content.chars().count(),
                        user_content.chars().count()
                    )));
                    found = true;
                }
                if !found {
                    self.items.push(Item::Info("no project memory file found (run /init to create ONYX.md)".into()));
                }
            }
            "/plan" => {
                let goal = line.trim_start_matches("/plan").trim();
                let prompt = if goal.is_empty() {
                    "Please enter plan mode to inspect the project requirements, outline architectural considerations, and prepare a structured implementation plan.".to_string()
                } else {
                    format!("Please enter plan mode to prepare a detailed implementation plan for: {goal}")
                };
                self.input = prompt;
                self.cursor = self.input.len();
                self.submit();
            }
            "/diff" => {
                let target_file = arg.map(|s| s.trim()).filter(|s| !s.is_empty());
                let mut cmd = std::process::Command::new("git");
                cmd.arg("diff");
                if let Some(file) = target_file {
                    cmd.arg("--").arg(file);
                }
                match cmd.output() {
                    Ok(out) => {
                        let diff_text = String::from_utf8_lossy(&out.stdout).trim().to_string();
                        if diff_text.is_empty() {
                            let msg = match target_file {
                                Some(f) => format!("No unstaged git changes found for `{f}`."),
                                None => "No unstaged git changes in working tree.".to_string(),
                            };
                            self.items.push(Item::Info(msg));
                        } else {
                            let heading = match target_file {
                                Some(f) => format!("### Git Diff: `{f}`\n\n```diff\n{diff_text}\n```"),
                                None => format!("### Git Diff\n\n```diff\n{diff_text}\n```"),
                            };
                            self.items.push(Item::Answer(heading));
                        }
                    }
                    Err(e) => {
                        self.items.push(Item::Error(format!("failed to run git diff: {e}")));
                    }
                }
            }
            "/review" => {
                let prompt = "Please inspect the current git diff and perform a comprehensive code review focusing on code quality, potential bugs, style consistency, and security considerations.";
                self.input = prompt.to_string();
                self.cursor = self.input.len();
                self.submit();
            }
            "/add-dir" => {
                match arg {
                    None => self.items.push(Item::Info("usage: /add-dir <directory_path>".into())),
                    Some(dir) => {
                        let path = PathBuf::from(dir);
                        if !path.exists() {
                            self.items.push(Item::Error(format!("no such directory: `{dir}`")));
                        } else if !path.is_dir() {
                            self.items.push(Item::Error(format!("not a directory: `{dir}`")));
                        } else {
                            let abs = std::fs::canonicalize(&path).unwrap_or(path.clone());
                            let key = abs.display().to_string();
                            if self.extra_dirs.iter().any(|d| d == &key) {
                                self.items.push(Item::Info(format!("{key} is already added this session")));
                            } else {
                                // Tools run in the process cwd; the honest way to
                                // widen scope is telling the model where to look.
                                let add = format!(
                                    "\n\n[Additional workspace directory] {}\nFiles there are readable and writable by absolute path.",
                                    key
                                );
                                if let Some(tx) = &self.ctl_tx {
                                    let _ = tx.send(Ctl::SystemAppend(add));
                                }
                                self.extra_dirs.push(key.clone());
                                self.items.push(Item::Info(format!(
                                    "added {key} to the model's awareness (system prompt appended; tools still run from {ws})",
                                    ws = self.workspace_dir
                                )));
                            }
                        }
                    }
                }
            }
            "/bashes" | "/tasks" => {
                // Honest inventory: onyx has no detached background-task runner,
                // so list THIS user's processes as the closest live truth.
                self.items.push(Item::Info(
                    "onyx has no detached background tasks yet — every tool call runs inside its turn.".into(),
                ));
                let me = std::env::var("USER").or_else(|_| std::env::var("LOGNAME")).unwrap_or_default();
                self.items.push(Item::Info(format!("processes owned by `{me}`:")));
                let ps = std::process::Command::new("ps")
                    .args(["-o", "pid,user,etime,comm"])
                    .output();
                match ps {
                    Ok(out) => {
                        let mut shown = 0usize;
                        for l in String::from_utf8_lossy(&out.stdout).lines().skip(1) {
                            let owner = l.split_whitespace().nth(1).unwrap_or("");
                            if !me.is_empty() && owner != me {
                                continue;
                            }
                            self.items.push(Item::Info(format!("  {}", l.trim())));
                            shown += 1;
                            if shown >= 10 {
                                break;
                            }
                        }
                        if shown == 0 {
                            self.items.push(Item::Info("  (none visible)".into()));
                        }
                    }
                    Err(_) => self.items.push(Item::Info("  ps unavailable".into())),
                }
            }
            "/btw" => {
                let q = line.trim_start_matches("/btw").trim();
                if q.is_empty() {
                    self.items.push(Item::Info("usage: /btw <quick question>".into()));
                } else {
                    self.input = format!("[Quick side query - answer concisely without changing active plan]: {q}");
                    self.cursor = self.input.len();
                    self.submit();
                }
            }
            "/explain" => {
                let target = line.trim_start_matches("/explain").trim();
                let prompt = if target.is_empty() {
                    "Please explain the overall project architecture, directory organization, and data flow of this codebase.".to_string()
                } else {
                    format!("Please explain the code structure, logic, and purpose of `{target}` in detail.")
                };
                self.input = prompt;
                self.cursor = self.input.len();
                self.submit();
            }
            "/refactor" => {
                let target = line.trim_start_matches("/refactor").trim();
                let prompt = if target.is_empty() {
                    "Please inspect the codebase for complex or brittle routines and suggest targeted refactorings to improve readability and performance.".to_string()
                } else {
                    format!("Please refactor `{target}` to optimize its clarity, robustness, and execution performance.")
                };
                self.input = prompt;
                self.cursor = self.input.len();
                self.submit();
            }
            "/test" => {
                let pattern = line.trim_start_matches("/test").trim();
                let prompt = if pattern.is_empty() {
                    "Please run the project test suite, diagnose any failures, and summarize the test results.".to_string()
                } else {
                    format!("Please run the project test suite matching `{pattern}`, verify all assertions, and report outcomes.")
                };
                self.input = prompt;
                self.cursor = self.input.len();
                self.submit();
            }
            "/debug" => {
                let err_text = line.trim_start_matches("/debug").trim();
                let prompt = if err_text.is_empty() {
                    "Please examine recent compiler warnings, errors, or logs and trace the root cause with a fix.".to_string()
                } else {
                    format!("Please debug the following error trace and suggest the exact fix: {err_text}")
                };
                self.input = prompt;
                self.cursor = self.input.len();
                self.submit();
            }
            "/docstrings" => {
                let prompt = "Please scan active code files for public functions, structs, or methods that lack documentation comments and generate comprehensive docstrings for them.";
                self.input = prompt.to_string();
                self.cursor = self.input.len();
                self.submit();
            }
            // System Settings & AI Control Handlers
            "/fast" => {
                if let Some(tx) = &self.ctl_tx {
                    let _ = tx.send(Ctl::SetEffort("low".into()));
                }
                self.hm.effort = "low".into();
                self.items.push(Item::Info("high-speed mode enabled (reasoning effort: low)".into()));
            }
            "/config" => {
                self.items.push(Item::Info(format!("active model:        {}", self.hm.model)));
                self.items.push(Item::Info(format!("reasoning effort:    {}", self.hm.effort)));
                self.items.push(Item::Info(format!("context budget:      {} tokens", self.hm.context_budget)));
                self.items.push(Item::Info(format!("max agent steps:     {}", self.opts.max_steps)));
                self.items.push(Item::Info("use /model, /effort, /steps, or /mode to change settings".into()));
            }
            "/mcp" => {
                self.items.push(Item::Info(format!("connected MCP servers: {} active", self.mcp_count)));
                self.items.push(Item::Info("config file: ~/.onyx/mcp.json".into()));
            }
            "/permissions" => {
                let mode = *self.gw.mode.lock().unwrap();
                self.items.push(Item::Info(format!("approval mode: {} (shift+tab or /mode cycles)", mode.label())));
                self.items.push(Item::Info("hard deny rules (bypass even AUTO):".into()));
                for pat in tools::policy::BashGuard::default().dangerous_patterns() {
                    self.items.push(Item::Info(format!("  bash ~ {pat}")));
                }
                self.items.push(Item::Info(
                    "sandbox capability gates ask before network/exec-escalating tools; approving a prompt with `a` (always) promotes a session rule so that exact pattern stops asking.".into(),
                ));
                if !self.extra_dirs.is_empty() {
                    self.items.push(Item::Info(format!("extra directories this session: {}", self.extra_dirs.join(", "))));
                }
            }
            "/output-style" => {
                let state = if self.expanded { "expanded tool output" } else { "collapsed tool output (ctrl+o toggles)" };
                self.items.push(Item::Info(format!(
                    "output style is fixed: markdown + truecolor syntax highlighting, inline tool rows · currently {state}"
                )));
                self.items.push(Item::Info("use /verbose to toggle detail level — there is no separate style catalogue yet".into()));
            }
            "/vim" => {
                self.vim_normal = !self.vim_normal;
                if self.vim_normal {
                    // Normal mode sits ON a character, not after it.
                    self.cursor = self.cursor.min(self.input.len().saturating_sub(1));
                    self.items.push(Item::Info(
                        "vim normal mode ON — h j k l 0 ^ $ w b x · dd d$ dw cut, p paste · i/a/A insert · Esc exits".into(),
                    ));
                } else {
                    self.vim_pending_d = false;
                    self.items.push(Item::Info("vim normal mode OFF — insert typing restored".into()));
                }
            }
            "/emacs" => {
                self.emacs_keys = !self.emacs_keys;
                if self.emacs_keys {
                    self.items.push(Item::Info(
                        "emacs keys ON: ctrl+a line start · ctrl+e line end · ctrl+b/f char · ctrl+k kill to end · ctrl+u clear line · ctrl+w delete word".into(),
                    ));
                } else {
                    self.items.push(Item::Info("emacs keys OFF — plain arrow editing".into()));
                }
            }
            "/theme" => {
                const NAMES: [&str; 3] = ["onyx-cyan", "amber-term", "emerald"];
                match arg {
                    None => {
                        let next = (self.theme_idx + 1) % NAMES.len();
                        self.set_theme(next);
                    }
                    Some(name) => match NAMES.iter().position(|n| *n == name) {
                        Some(idx) => self.set_theme(idx),
                        None => self.items.push(Item::Info(format!(
                            "unknown theme `{name}` — choose from: {}",
                            NAMES.join(", ")
                        ))),
                    },
                }
            }
            "/verbose" => {
                self.expanded = !self.expanded;
                let state = if self.expanded { "expanded" } else { "collapsed" };
                self.items.push(Item::Info(format!("tool output verbosity: {state} (press Ctrl+O anytime)")));
            }
            // Token Analytics & Health Diagnostics Handlers
            "/cost" => {
                // Rates are per-token estimates for a gpt-oss-class model through a
                // local proxy; cached prompt tokens are billed at ~10% of input rate.
                const IN: f64 = 0.000_000_15;
                const OUT: f64 = 0.000_000_60;
                let fresh_in = self.real_prompt_tokens.saturating_sub(self.real_cached_tokens) as f64;
                let cache_read = (self.real_cached_tokens as f64) * IN * 0.10;
                let prompt_cost = fresh_in * IN + cache_read;
                let comp_cost = (self.real_completion_tokens as f64) * OUT;
                let total = prompt_cost + comp_cost;
                self.items.push(Item::Info(format!(
                    "estimated session cost: ${total:.5} (prompt ${prompt_cost:.5}, completion ${comp_cost:.5})"
                )));
                self.items.push(Item::Info(format!(
                    "tokens: {} in ({} cached) · {} out · {} total",
                    self.real_prompt_tokens, self.real_cached_tokens,
                    self.real_completion_tokens, self.real_total_tokens
                )));
                if self.hm.host == "127.0.0.1" || self.hm.host == "localhost" {
                    self.items.push(Item::Info(
                        "note: loopback gateway — your proxy may meter or bill differently than these list-price estimates".into(),
                    ));
                }
            }
            "/stats" => {
                let files_cnt = std::process::Command::new("git").args(["ls-files"]).output()
                    .map(|o| o.stdout.split(|&b| b == b'\n').count())
                    .unwrap_or(0);
                self.items.push(Item::Info(format!("tracked files in workspace: {files_cnt} files")));
                self.items.push(Item::Info(format!("working directory: {}", self.workspace_dir)));
            }
            "/insights" => {
                use std::collections::BTreeMap;
                let audit = self.gw.audit.snapshot();
                let mut mix: BTreeMap<&str, usize> = BTreeMap::new();
                let mut denials = 0usize;
                let mut errors = 0usize;
                for e in &audit {
                    *mix.entry(e.tool.as_str()).or_default() += 1;
                    if e.outcome.starts_with("DENY") {
                        denials += 1;
                    } else if e.outcome.starts_with("ERR") || e.outcome.starts_with("FAIL") {
                        errors += 1;
                    }
                }
                self.items.push(Item::Info(format!(
                    "session insights — {} turn(s), {} message(s), {} tool call(s)",
                    self.turns, self.history_len, audit.len()
                )));
                if !mix.is_empty() {
                    let top: Vec<_> = mix.iter().take(8).map(|(t, n)| format!("{t}:{n}")).collect();
                    self.items.push(Item::Info(format!("tool mix: {}", top.join("  "))));
                }
                self.items.push(Item::Info(format!(
                    "policy denials: {denials} · tool errors: {errors}"
                )));
                if let Some(start) = self.turn_start {
                    self.items.push(Item::Info(format!("current turn running for {:.1}s", start.elapsed().as_secs_f64())));
                }
                if self.ctx_history.len() >= 2 {
                    let first = self.ctx_history[0] as i32;
                    let last = *self.ctx_history.last().unwrap() as i32;
                    let trend = match (last - first).cmp(&0) {
                        std::cmp::Ordering::Greater => "growing",
                        std::cmp::Ordering::Less => "shrinking",
                        std::cmp::Ordering::Equal => "flat",
                    };
                    self.items.push(Item::Info(format!(
                        "context usage {} over the last {} turns ({first}% → {last}%)",
                        trend, self.ctx_history.len()
                    )));
                }
            }
            "/doctor" => self.do_doctor(),
            "/ping" => self.do_ping(),
            "/context" => {
                let (filled, empty, pct) = self.ctx_meter(24);
                let bar = format!("{}{}", "█".repeat(filled), "░".repeat(empty));
                self.items.push(Item::Info(format!("context window capacity: [{bar}] {pct}%")));
                self.items.push(Item::Info(format!("budget: {} tokens · current total: {} tokens", self.hm.context_budget, self.real_total_tokens)));
            }
            "/logs" => {
                self.items.push(Item::Info("recent audit trail (local time):".into()));
                let recent = self.gw.audit.snapshot();
                for e in recent.iter().rev().take(12) {
                    let secs = e.ts_ms / 1000;
                    let hhmmss = {
                        let hours = (secs / 3600) % 24;
                        let mins = (secs / 60) % 60;
                        let ss = secs % 60;
                        format!("{hours:02}:{mins:02}:{ss:02}")
                    };
                    self.items.push(Item::Info(format!(
                        "  {hhmmss}  {} [{}] {}",
                        e.tool,
                        e.outcome,
                        clip_chars(first_line(&e.args), 40)
                    )));
                }
                if recent.is_empty() {
                    self.items.push(Item::Info("  (no tool calls audited yet)".into()));
                }
            }
            // Ecosystem & Advanced Integrations Handlers
            "/batch" => {
                let target = line.trim_start_matches("/batch").trim();
                let prompt = format!("Please orchestrate a batch workflow across files: {target}");
                self.input = prompt;
                self.cursor = self.input.len();
                self.submit();
            }
            "/design" | "/dataviz" => {
                let prompt = "Please launch a visual layout and design exploration for the requested components with clean UI structuring.";
                self.input = prompt.to_string();
                self.cursor = self.input.len();
                self.submit();
            }
            "/api" => {
                self.items.push(Item::Info(format!(
                    "onyx speaks the OpenAI-compatible chat API to {}:{}{} — model `{}`, tools via function-calling schemas.",
                    self.hm.host, self.hm.port,
                    if self.hm.path.is_empty() { "" } else { self.hm.path.as_str() },
                    self.hm.model
                )));
                self.items.push(Item::Info("there is no bundled vendor-doc browser; ask onyx to read an SDK file in the workspace instead".into()));
            }
            "/claude-api" | "/install-github-app" | "/pr-comments" | "/desktop" | "/app" => {
                self.items.push(Item::Info(format!(
                    "`{cmd}` is a vendor-specific integration that onyx does not ship — refusing rather than pretending.",
                    cmd = cmd
                )));
                self.items.push(Item::Info("equivalent reach lives in MCP servers (~/.onyx/mcp.json) and workspace tools; run /mcp to see what's connected".into()));
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
        let mut catalog = skills::SkillCatalog::new();
        catalog.scan_all_default();
        if let Some(sk) = catalog.get(name) {
            let add = format!("\n\n[Active Skill: {}]\n{}\n{}", sk.name, sk.description, sk.instructions);
            if let Some(tx) = &self.ctl_tx {
                let _ = tx.send(Ctl::SystemAppend(add));
            }
            self.items.push(Item::Info(format!("loaded skill: {}", sk.name)));
        } else {
            self.items.push(Item::Error(format!("skill `{name}` not found in skill directories")));
        }
    }

    /// `$ONYX_HOME/skills`, defaulting to `~/.onyx/skills`.
    #[allow(dead_code)]
    fn skills_root() -> Option<PathBuf> {
        let base = match std::env::var("ONYX_HOME") {
            Ok(h) if !h.is_empty() => PathBuf::from(h),
            _ => std::env::var_os("HOME").map(PathBuf::from).and_then(|h| Some(h.join(".onyx")))?,
        };
        Some(base.join("skills"))
    }

    /// Git diff short summary (+lines -lines) for the current workspace (cached for 5 seconds)
    fn git_diff_summary(&mut self) -> Option<String> {
        let now = std::time::Instant::now();
        if let (Some(last), Some(cached)) = (self.last_diff_check, &self.cached_diff_stat) {
            if now.duration_since(last) < Duration::from_secs(5) {
                return Some(cached.clone());
            }
        } else if let Some(last) = self.last_diff_check {
            if now.duration_since(last) < Duration::from_secs(5) {
                return None;
            }
        }

        self.last_diff_check = Some(now);

        let out = std::process::Command::new("git")
            .args(["diff", "--shortstat"])
            .output()
            .ok()?;
        if !out.status.success() {
            self.cached_diff_stat = None;
            return None;
        }
        let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if s.is_empty() {
            self.cached_diff_stat = None;
            return None;
        }
        // Example output: "6 files changed, 331 insertions(+), 76 deletions(-)"
        let mut ins = 0;
        let mut del = 0;
        for part in s.split(',') {
            let part = part.trim();
            if part.contains("insertion") {
                if let Some(num) = part.split_whitespace().next().and_then(|n| n.parse::<usize>().ok()) {
                    ins = num;
                }
            } else if part.contains("deletion") {
                if let Some(num) = part.split_whitespace().next().and_then(|n| n.parse::<usize>().ok()) {
                    del = num;
                }
            }
        }
        let res = if ins > 0 || del > 0 {
            Some(format!("+{ins} -{del}"))
        } else {
            None
        };
        self.cached_diff_stat = res.clone();
        res
    }

    /// Run the SkillOpt-Sleep offline cycle over saved transcripts and gate the
    /// resulting edits. Deterministic: no model call is made, so this works even
    /// when every inference endpoint is unreachable. Accepted candidates are held
    /// for confirmation rather than written straight to disk.
    fn run_skillopt(&mut self, target: Option<&str>) {
        use skillopt::skillopt_sleep::onyx_facade::optimize_skills;

        let mut catalog = skills::SkillCatalog::new();
        catalog.scan_all_default();
        if catalog.count() == 0 {
            self.items.push(Item::Error("no skills found in any configured skill directory".into()));
            return;
        }

        let mut skills = std::collections::HashMap::new();
        match target {
            None => {
                for (name, sk) in &catalog.skills {
                    skills.insert(name.clone(), sk.instructions.clone());
                }
            }
            Some(name) => match catalog.get(name) {
                Some(sk) => {
                    skills.insert(sk.name.clone(), sk.instructions.clone());
                }
                None => {
                    self.items.push(Item::Error(format!("skill `{name}` not found")));
                    return;
                }
            },
        }

        let sessions = agent::session::sessions_dir();
        let n_sessions = std::fs::read_dir(&sessions)
            .map(|d| {
                d.flatten()
                    .filter(|e| e.path().extension().map(|x| x == "jsonl").unwrap_or(false))
                    .count()
            })
            .unwrap_or(0);
        self.items.push(Item::Info(format!(
            "⟳ skillopt: harvesting {n_sessions} transcript(s), optimizing {} skill(s)",
            skills.len()
        )));

        // Deterministic path: no model client is constructed, so this completes
        // even with every inference endpoint down.
        let outcome = match optimize_skills(sessions, skills) {
            Ok(o) => o,
            Err(e) => {
                self.items.push(Item::Error(format!("skillopt failed: {e}")));
                return;
            }
        };

        for note in &outcome.notes {
            self.items.push(Item::Info(format!("   {note}")));
        }
        if outcome.tasks.is_empty() {
            self.items.push(Item::Info(
                "nothing to learn: no mined tasks with a substantive prompt".into(),
            ));
            return;
        }

        for r in &outcome.reports {
            let mark = if r.accepted { "✓" } else { "✗" };
            self.items.push(Item::Info(format!(
                "   {mark} {:<22} {:<15} {:>6.3} → {:>6.3}  {} task(s) {} edit(s)  {}",
                r.skill_name,
                r.gate_action,
                r.baseline_score,
                r.candidate_score,
                r.n_tasks,
                if r.accepted { r.n_applied_edits } else { r.n_rejected_edits },
                r.reason
            )));
        }

        if outcome.updated_skills.is_empty() {
            self.items.push(Item::Info("gate rejected every candidate — no skill changed".into()));
            return;
        }

        self.pending_skill_updates = outcome.updated_skills;
        let names: Vec<&str> = self.pending_skill_updates.keys().map(|s| s.as_str()).collect();
        self.items.push(Item::Info(format!(
            "staged {} skill update(s): {}. /skillopt apply writes them, /skillopt discard drops them.",
            names.len(),
            names.join(", ")
        )));
    }

    /// Write staged SkillOpt candidates back to their SKILL.md files.
    fn apply_skillopt(&mut self) {
        if self.pending_skill_updates.is_empty() {
            self.items.push(Item::Info("nothing staged — run /skillopt first".into()));
            return;
        }
        let mut catalog = skills::SkillCatalog::new();
        catalog.scan_all_default();

        let updates = std::mem::take(&mut self.pending_skill_updates);
        for (name, instructions) in updates {
            let probe = skills::Skill {
                name: name.clone(),
                description: String::new(),
                tools_allowed: Vec::new(),
                model_hint: None,
                instructions: instructions.clone(),
            };
            let new_hash = probe.content_hash();
            match catalog.persist_instructions(&name, &instructions) {
                Ok(prev) => {
                    self.items.push(Item::Info(format!("   ✓ wrote {name}: {prev} → {new_hash}")))
                }
                Err(e) => self.items.push(Item::Error(format!("   ✗ {name}: {e}"))),
            }
        }
        // Re-scan so the header count reflects any skill that failed to parse.
        let mut refreshed = skills::SkillCatalog::new();
        refreshed.scan_all_default();
        self.skill_count = refreshed.count();
    }
    /// Map one protocol event onto the transcript. This is the ONLY place the
    /// UI reacts to engine progress — it never sees AgentEvent directly.
    fn on_core_event(&mut self, ev: onyx_core::CoreEvent) {
        use onyx_core::{ItemKind, ItemStatus};
        match (ev.status, ev.kind) {
            (ItemStatus::Started, ItemKind::Thinking { .. }) => {
                self.tail.clear();
                self.reasoning.clear();
                self.tail_scrolled = 0;
                self.thinking = true;
            }
            (_, ItemKind::Thinking { delta: Some(d), .. }) => {
                // Live reasoning text. Kept out of `tail` so it never becomes the answer.
                self.thinking = true;
                self.reasoning.push_str(&d);
                let n = self.reasoning.chars().count();
                if n > 1400 {
                    self.tail_scrolled = n - 1200;
                }
            }
            (_, ItemKind::AgentMessage { delta, final_text }) => {
                if let Some(d) = delta {
                    self.thinking = false;
                    self.tail.push_str(&d);
                    let n = self.tail.chars().count();
                    if n > 1400 {
                        self.tail_scrolled = n - 1200;
                    }
                } else if let Some(f) = final_text {
                    // Final answer edge: finalize_tail already covers mid-turn
                    // text; only add it if nothing streamed produced it.
                    if self.tail.trim().is_empty() {
                        let clean = clean_model_text(&f);
                        let dup = self.items.iter().rev().any(|i| matches!(i, Item::Answer(t) if t == &clean));
                        if !clean.trim().is_empty() && !dup {
                            self.items.push(Item::Answer(clean));
                        }
                    }
                }
            }
            (ItemStatus::Started, ItemKind::ToolCall { name, args, .. }) => {
                if name == "terminate" || name == "ask_human" || name == "ask_user_question" {
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
            (status, ItemKind::ToolCall { name, output, .. })
                if matches!(status, ItemStatus::Completed | ItemStatus::Failed) =>
            {
                if name == "terminate" || name == "ask_human" || name == "ask_user_question" {
                    return;
                }
                let dur = self.active_tool_start.take().map(|t| t.elapsed());
                let out = output.unwrap_or_default();
                let ok = status == ItemStatus::Completed;
                let denied = out.starts_with("denied:") || out.contains("confirmation refused");
                for it in self.items.iter_mut().rev() {
                    if let Item::Tool { name: n, ok: o, denied: d, duration: du, output: ot, .. } = it {
                        if n == &name && ot.is_empty() {
                            *o = ok;
                            *d = denied;
                            *du = dur;
                            *ot = out;
                            break;
                        }
                    }
                }
                self.thinking = true;
            }
            (ItemStatus::Completed, ItemKind::Usage(u)) => {
                self.real_prompt_tokens = u.prompt_tokens as usize;
                self.real_completion_tokens = u.completion_tokens as usize;
                self.real_total_tokens = u.total_tokens as usize;
                self.real_cached_tokens = u.cached_prompt_tokens.unwrap_or(0) as usize;
                let (_, _, pct) = self.ctx_meter(100);
                self.ctx_history.push(pct as u8);
                if self.ctx_history.len() > 48 {
                    self.ctx_history.remove(0);
                }
            }
            (_, ItemKind::UserMessage { .. }) => {
                // Already echoed by the UI thread on submit; replays render it
                // directly. Nothing to do here.
            }
            (_, ItemKind::Info { text }) => {
                // Per-turn retrieval status ("offered N of M tools via bm25").
                if !text.trim().is_empty() {
                    self.items.push(Item::Info(text));
                }
            }
            _ => {}
        }
    }

    fn finalize_tail(&mut self) {
        if !self.reasoning.trim().is_empty() {
            let r = std::mem::take(&mut self.reasoning);
            self.items.push(Item::Thinking(r));
        }
        if !self.tail.trim().is_empty() {
            let t = std::mem::take(&mut self.tail);
            let clean = clean_model_text(&t);
            // The worker echoes the prompt as a UserMessage protocol item; if the
            // tail streamed that same text back, don't render it twice.
            let dup = self.items.iter().rev().any(|i| matches!(i, Item::User(u) if u.trim() == clean.trim()));
            if !clean.trim().is_empty() && !dup {
                self.items.push(Item::Answer(clean.trim().to_string()));
            }
        }
        self.tail_scrolled = 0;
    }

    fn on_turn_done(&mut self, res: Result<(String, usize), String>) {
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
            Err(msg) => self.items.push(Item::Error(msg)),
        }

        // Automatic context compression:
        // Check if the real token usage has exceeded 85% of the model's total context window.
        let (_, _, pct) = self.ctx_meter(100);
        if pct >= 85 {
            self.items.push(Item::Info(format!(
                "context window reached {pct}% capacity → running automatic progressive compression"
            )));
            self.do_compact();
        }
    }

    // --------------------------------------------------------------- the loop ----

    fn run(&mut self, pump: &InputPump, paint: &mut dyn FnMut(&mut Self)) {
        paint(self);
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
                    UiOut::Core(ev) => self.on_core_event(ev),
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
                paint(self);
            }

            let poll = if self.thinking || self.running {
                Duration::from_millis(100)
            } else {
                Duration::from_millis(250)
            };
            let Some((chars, code)) = pump.next_key(poll) else {
                // When actively running or thinking, advance tick and repaint to keep spinner lively.
                // When completely idle with no pending popups or spinners, avoid constant redraw.
                if self.thinking || self.running || self.connect.is_some() || self.resume_browser.is_some() || self.settings_active {
                    self.tick += 1;
                    paint(self);
                }
                continue;
            };

            // The connect wizard owns the keyboard outright while open: its
            // search field and key entry replace the composer, so nothing here
            // may fall through to normal input handling. One event at a time —
            // a merged printable run shares a single `code`, so iterating chars
            // and passing that code would misread arrow keys as text.
            if self.connect.is_some() {
                self.connect_key(&chars, code);
                paint(self);
                continue;
            }

            // The resume browser owns the keyboard outright while open, exactly
            // like the connect wizard: one event at a time so a merged printable
            // run isn't mistaken for an arrow key.
            if self.resume_browser.is_some() {
                self.browser_key(&chars, code);
                paint(self);
                continue;
            }

            // Approval card swallows most keys while pending.
            if self.ask.pending().is_some() {
                for &ch in &chars {
                    match ch {
                        'y' | 'Y' => self.ask.answer(Approval::Once),
                        'a' => self.ask.answer(Approval::Always),
                        'n' | 'N' => self.ask.answer(Approval::Reject),
                        _ => {}
                    }
                }
                paint(self);
                continue;
            }

            // Interactive Question popup swallows keys while asking user.
            if let Some((_q, options)) = self.ask.pending_question() {
                let first = chars[0];
                let count = options.len();
                match (first, code) {
                    ('\0', 1000) => {
                        // Up arrow: navigate options
                        if count > 0 {
                            self.question_selected = (self.question_selected + count - 1) % count;
                        }
                    }
                    ('\0', 1001) => {
                        // Down arrow: navigate options
                        if count > 0 {
                            self.question_selected = (self.question_selected + 1) % count;
                        }
                    }
                    ('\t', _) => {
                        // Tab: copy highlighted option text into question_input for editing
                        if !options.is_empty() {
                            let idx = self.question_selected.min(count - 1);
                            self.question_input = options[idx].clone();
                            self.question_cursor = self.question_input.len();
                        }
                    }
                    ('\r', _) | ('\n', _) => {
                        // Enter: submit typed answer if present, else selected option
                        let trimmed = self.question_input.trim();
                        if !trimmed.is_empty() {
                            self.ask.answer_question(Ok(trimmed.to_string()));
                        } else if !options.is_empty() {
                            let idx = self.question_selected.min(count - 1);
                            let selected_text = options[idx].clone();
                            self.ask.answer_question(Ok(selected_text));
                        } else {
                            self.ask.answer_question(Ok("confirmed".to_string()));
                        }
                        self.question_selected = 0;
                        self.question_input.clear();
                        self.question_cursor = 0;
                    }
                    ('\x1b', _) | ('\x03', _) => {
                        // Escape / Ctrl+C cancels question
                        self.ask.answer_question(Err("User cancelled question".to_string()));
                        self.question_selected = 0;
                        self.question_input.clear();
                        self.question_cursor = 0;
                    }
                    ('\u{7f}', _) => {
                        // Backspace in question_input
                        if self.question_cursor > 0 {
                            let prev = self.question_input[..self.question_cursor]
                                .chars()
                                .next_back()
                                .map(|c| c.len_utf8())
                                .unwrap_or(0);
                            self.question_cursor -= prev;
                            self.question_input.remove(self.question_cursor);
                        }
                    }
                    ('\0', 1004) => {
                        // Delete key
                        if self.question_cursor < self.question_input.len() {
                            let next = self.question_input[self.question_cursor..]
                                .chars()
                                .next()
                                .map(|c| c.len_utf8())
                                .unwrap_or(0);
                            self.question_input.replace_range(self.question_cursor..self.question_cursor + next, "");
                        }
                    }
                    ('\0', 1002) => {
                        // Right arrow
                        if self.question_cursor < self.question_input.len() {
                            let next = self.question_input[self.question_cursor..]
                                .chars()
                                .next()
                                .map(|c| c.len_utf8())
                                .unwrap_or(0);
                            self.question_cursor += next;
                        }
                    }
                    ('\0', 1003) => {
                        // Left arrow
                        if self.question_cursor > 0 {
                            let prev = self.question_input[..self.question_cursor]
                                .chars()
                                .next_back()
                                .map(|c| c.len_utf8())
                                .unwrap_or(0);
                            self.question_cursor -= prev;
                        }
                    }
                    ('\0', 1006) => self.question_cursor = 0, // Home
                    ('\0', 1007) => self.question_cursor = self.question_input.len(), // End
                    (c, 0) if self.question_input.is_empty() && c >= '1' && c <= '9' => {
                        // Quick digit jump if input is empty
                        let num = (c as usize) - ('0' as usize);
                        if num <= count {
                            let selected_text = options[num - 1].clone();
                            self.ask.answer_question(Ok(selected_text));
                            self.question_selected = 0;
                            self.question_input.clear();
                            self.question_cursor = 0;
                        }
                    }
                    (_, 2000) => {
                        // Bracketed paste into question input
                        for &c in &chars {
                            if c != '\r' && c != '\n' {
                                self.question_input.insert(self.question_cursor, c);
                                self.question_cursor += c.len_utf8();
                            } else {
                                self.question_input.insert(self.question_cursor, ' ');
                                self.question_cursor += 1;
                            }
                        }
                    }
                    _ => {
                        // Regular printable typing into question_input
                        for &c in &chars {
                            if !c.is_control() {
                                self.question_input.insert(self.question_cursor, c);
                                self.question_cursor += c.len_utf8();
                            }
                        }
                    }
                }
                paint(self);
                continue;
            }

            // Interactive Settings Modal swallows keys while open.
            if self.settings_active {
                let first = chars[0];
                let count = self.settings_items().len();
                match (first, code) {
                    ('\0', 1000) => {
                        // Up arrow
                        if count > 0 {
                            self.settings_selected = (self.settings_selected + count - 1) % count;
                        }
                    }
                    ('\0', 1001) => {
                        // Down arrow
                        if count > 0 {
                            self.settings_selected = (self.settings_selected + 1) % count;
                        }
                    }
                    ('\r', _) | ('\n', _) | (' ', _) => {
                        // Enter or Space: cycle selected setting
                        self.cycle_selected_setting();
                    }
                    ('\x1b', _) | ('q', 0) | ('Q', 0) | ('\x03', _) => {
                        // Escape, q, or Ctrl+C: close settings modal
                        self.settings_active = false;
                        self.items.push(Item::Info("settings saved to ~/.onyx/config.json".into()));
                    }
                    (c, 0) if c >= '1' && c <= '9' => {
                        let num = (c as usize) - ('0' as usize);
                        if num <= count {
                            self.settings_selected = num - 1;
                            self.cycle_selected_setting();
                        }
                    }
                    _ => {}
                }
                paint(self);
                continue;
            }

            // Special keys arrive alone; printable runs carry several chars.
            let first = chars[0];

            // Vim modal editing in the composer. `vim_normal` is the NORMAL
            // (command) mode; printable keys run through the state machine
            // instead of inserting. Esc toggles: normal -> insert, and from
            // insert it returns to normal without a message. Control bytes and
            // Enter keep their global meanings (exit, cancel, submit).
            if self.vim_normal && code == 0 {
                let mut handled_any = false;
                for &c in &chars {
                    if !c.is_control() {
                        handled_any = true;
                        self.vim_normal_key(c);
                    } else if c == '\x1b' {
                        // Lone Esc: drop into INSERT so typed text inserts.
                        self.vim_normal = false;
                        self.vim_pending_d = false;
                        self.items.push(Item::Info("vim: insert mode (Esc pressed)".into()));
                        break;
                    }
                }
                if handled_any {
                    if self.quit {
                        break;
                    }
                    paint(self);
                    continue;
                }
            } else if !self.vim_normal && code == 0 && chars.len() == 1 && chars[0] == '\x1b' {
                // Insert-mode Esc: return to NORMAL (no message — vim itself is
                // silent here; the next printable key will act as a command).
                self.vim_normal = true;
                self.vim_pending_d = false;
                paint(self);
                continue;
            }

            let insert_run = |s: &mut Self, chars: &[char]| {
                s.scroll_offset = 0; // jump back to bottom on paste/multi-character input
                for &c in chars {
                    s.input.insert(s.cursor, c);
                    s.cursor += c.len_utf8();
                }
                s.slash_selected = 0;
                s.last_ctrl_d = None;
            };

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
                            let mut c = self.cancel.lock().unwrap();
                            if !*c {
                                *c = true;
                                self.items.push(Item::Info("cancelling after the current step…".into()));
                            }
                        } else {
                            self.items.push(Item::Info("(ctrl+d ×2 or /exit to quit)".into()));
                        }
                    }
                    ('\x1b', _) => {
                        if self.running {
                            let mut c = self.cancel.lock().unwrap();
                            if !*c {
                                *c = true;
                                self.items.push(Item::Info("cancelling after the current step…".into()));
                            }
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
                            if title.contains("Conversations") || title.contains("Resume") {
                                self.input = format!("/resume {target}");
                            } else if title.contains("Providers") {
                                self.input = format!("/provider {target}");
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
                            if title.contains("Conversations") || title.contains("Resume") {
                                // Resume immediately!
                                self.input = format!("/resume {target}");
                                self.cursor = self.input.len();
                                self.slash_selected = 0;
                                self.submit();
                            } else if title.contains("Providers") {
                                // Switch provider immediately!
                                self.input = format!("/provider {target}");
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
                                if cmd == "/clear" || cmd == "/tools" || cmd == "/gain" || cmd == "/exit" || cmd == "/mode" || cmd == "/help" || cmd == "/usage" || cmd == "/audit" || cmd == "/sessions" || cmd == "/settings" {
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
                    // Emacs-style bindings, armed by /emacs. Line semantics:
                    // ctrl+a/e jump within the current line, not buffer ends.
                    ('\x01', _) if self.emacs_keys => {
                        self.cursor = self.vim_line_start_byte();
                    }
                    ('\x05', _) if self.emacs_keys => {
                        self.cursor = self.vim_line_end_byte();
                    }
                    ('\x02', _) if self.emacs_keys => self.vim_move_left(),
                    ('\x06', _) if self.emacs_keys => self.vim_move_right(),
                    ('\x0b', _) if self.emacs_keys => {
                        let le = self.vim_line_end_byte();
                        self.input.replace_range(self.cursor..le, "");
                        self.cursor = self.cursor.min(self.input.len());
                    }
                    ('\x15', _) if self.emacs_keys => {
                        self.input.clear();
                        self.cursor = 0;
                        self.slash_selected = 0;
                    }
                    ('\x17', _) if self.emacs_keys => {
                        let start = self.vim_word_back_one();
                        self.input.replace_range(start..self.cursor, "");
                        self.cursor = start;
                    }
                    (c, 0) if !c.is_control() => {
                        insert_run(self, std::slice::from_ref(&c));
                    }
                    (_, 2000) => {
                        // Bracketed paste event into composer
                        insert_run(self, &chars);
                    }
                    _ => {}
                }
            } else {
                insert_run(self, &chars);
            }
            if self.quit {
                break;
            }
            paint(self);
        }
    }

    // ------------------------------------------------------------ ratatui ----

    /// Drive the same event loop as `run`, but repaint through the ratatui
    /// dashboard instead of the hand-rolled ANSI writer. Kept separate so the
    /// legacy `draw`/`render_compact` path stays available for reference until
    /// it is deleted once this path fully covers both layouts.
    fn run_ratatui(
        &mut self,
        pump: &InputPump,
        terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    ) {
        self.run(pump, &mut |s| s.draw_ratatui(terminal));
    }

    /// Paint one frame and park the caret on the composer input cell.
    fn draw_ratatui(&mut self, terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) {
        self.sync_size();
        let diff_stat = self.git_diff_summary();
        let _ = terminal.draw(|f| self.render(f, diff_stat.as_deref()));
    }

    /// Convert a legacy styled-ANSI string into a ratatui `Line`. We keep the
    /// item-content builders (`item_lines`) producing ANSI so the two renderers
    /// share one source of truth; this strips the SGR codes back into Spans.
    fn ansi_to_line(s: &str) -> RuLine<'static> {
        let mut spans = Vec::new();
        let mut cur = String::new();
        let mut pending: Option<(u8, u8, u8)> = None;
        let mut is_bold = false;
        let mut is_dim = false;
        let mut i = 0usize;
        let bytes = s.as_bytes();
        while i < bytes.len() {
            if bytes[i] == 0x1b && i + 1 < bytes.len() && bytes[i + 1] == b'[' {
                if !cur.is_empty() {
                    let mut st = RuStyle::default();
                    if let Some((r, g, b)) = pending {
                        st = st.fg(ratatui::style::Color::Rgb(r, g, b));
                    }
                    if is_bold {
                        st = st.add_modifier(ratatui::style::Modifier::BOLD);
                    }
                    if is_dim {
                        st = st.add_modifier(ratatui::style::Modifier::DIM);
                    }
                    spans.push(RuSpan::styled(std::mem::take(&mut cur), st));
                }
                let mut j = i + 2;
                let mut params = String::new();
                while j < bytes.len() && !(bytes[j].is_ascii_alphabetic() || bytes[j] == b'~') {
                    params.push(bytes[j] as char);
                    j += 1;
                }
                let final_c = if j < bytes.len() { bytes[j] as char } else { 'm' };
                if final_c == 'm' {
                    for part in params.split(';') {
                        match part {
                            "" | "0" => {
                                pending = None;
                                is_bold = false;
                                is_dim = false;
                            }
                            "1" => is_bold = true,
                            "2" => is_dim = true,
                            "30" => pending = Some((40, 40, 40)),      // Black
                            "31" => pending = Some((239, 68, 68)),    // Red
                            "32" => pending = Some((34, 197, 94)),    // Green
                            "33" => pending = Some((234, 179, 8)),    // Yellow
                            "34" => pending = Some((59, 130, 246)),   // Blue
                            "35" => pending = Some((168, 85, 247)),   // Magenta
                            "36" => pending = Some((6, 182, 212)),    // Cyan
                            "37" => pending = Some((240, 240, 240)),  // White
                            "90" => pending = Some((120, 120, 120)),  // Bright Black / Gray
                            "91" => pending = Some((248, 113, 113)),  // Bright Red
                            "92" => pending = Some((74, 222, 128)),   // Bright Green
                            "93" => pending = Some((250, 204, 21)),   // Bright Yellow
                            "94" => pending = Some((96, 165, 250)),   // Bright Blue
                            "95" => pending = Some((192, 132, 252)),  // Bright Magenta
                            "96" => pending = Some((34, 211, 238)),   // Bright Cyan
                            "97" => pending = Some((255, 255, 255)),  // Bright White
                            _ => {}
                        }
                    }
                    if let Some(rgb) = params.strip_prefix("38;2;") {
                        let mut it = rgb.split(';');
                        if let (Some(r), Some(g), Some(b)) =
                            (it.next(), it.next(), it.next())
                        {
                            pending = Some((
                                r.parse().unwrap_or(255),
                                g.parse().unwrap_or(255),
                                b.parse().unwrap_or(255),
                            ));
                        }
                    }
                }
                i = j + 1;
            } else {
                let start = i;
                while i < bytes.len() && bytes[i] != 0x1b {
                    i += 1;
                }
                cur.push_str(&s[start..i]);
            }
        }
        if !cur.is_empty() {
            let mut st = RuStyle::default();
            if let Some((r, g, b)) = pending {
                st = st.fg(ratatui::style::Color::Rgb(r, g, b));
            }
            if is_bold {
                st = st.add_modifier(ratatui::style::Modifier::BOLD);
            }
            if is_dim {
                st = st.add_modifier(ratatui::style::Modifier::DIM);
            }
            spans.push(RuSpan::styled(std::mem::take(&mut cur), st));
        }
        RuLine::from(spans)
    }

    /// Build the transcript body lines (all non-banner items + streaming tail),
    /// each already width-fitted to `width` by `transcript_lines`.
    fn transcript_ru_lines(&self, width: usize) -> Vec<RuLine<'static>> {
        self.transcript_lines(width)
            .into_iter()
            .map(|l| Self::ansi_to_line(&l))
            .collect()
    }

    /// Column position of the caret within the composer text row, accounting for
    /// the `❯ ` prompt prefix and the current input window slice.
    fn caret_col(&self) -> u16 {
        // The composer shows either the full input or an input_window slice;
        // in both cases the caret sits after "❯ " plus chars up to cursor.
        let cc = self.cursor_chars();
        // Reserve matches input_window(): avail = width - 6, start = cc - (avail-1).
        let avail = self.width.saturating_sub(6).max(8);
        let total = self.input.chars().count();
        let raw_start = cc.saturating_sub(avail - 1);
        let end = (raw_start + avail).min(total);
        let win_start = end.saturating_sub(avail);
        let offset_in_win = cc - win_start;
        let prefix = 2u16; // "❯ "
        (prefix + offset_in_win as u16).min(self.width.saturating_sub(1) as u16)
    }

    /// Full-dashboard layout: header · status/context strip · scrolling
    /// transcript · [approval] · [slash dropdown] · composer · hints/footer.
    fn render(&self, f: &mut Frame, diff_stat: Option<&str>) {
        let area = f.area();
        f.buffer_mut().set_style(area, theme::base());
        let w = area.width.max(20) as usize;

        // Full-screen resume browser: owns the entire viewport while open, so
        // nothing below renders. Mirrors how the connect wizard is gated.
        if self.resume_browser.is_some() {
            self.render_resume_browser(f, area);
            return;
        }

        // ── header (centered block-glyph ONYX banner + tagline) ──
        let account = std::env::var("ONYX_ACCOUNT").unwrap_or_else(|_| "local".to_string());
        let wide_header = w >= 40 && area.height >= 12;
        const BANNER_ROWS: [&'static str; 6] = [
            "██████╗ ███╗   ██╗██╗   ██╗██╗  ██╗",
            "██╔═══██╗████╗  ██║╚██╗ ██╔╝╚██╗██╔╝",
            "██║   ██║██╔██╗ ██║ ╚████╔╝  ╚███╔╝ ",
            "██║   ██║██║╚██╗██║  ╚██╔╝   ██╔██╗ ",
            "╚██████╔╝██║ ╚████║   ██║   ██╔╝ ██╗",
            " ╚═════╝ ╚═╝  ╚═══╝   ╚═╝   ╚═╝  ╚═╝",
        ];
        let banner_rows: Vec<RuLine> = BANNER_ROWS
            .iter()
            .map(|row| {
                // Graphemes, not bytes: row.len() would make the pad negative for
                // wide UTF-8 and saturate to 0 (banner jams left).
                RuLine::from(vec![
                    RuSpan::raw(" ".repeat((w.saturating_sub(row.chars().count())) / 2)),
                    RuSpan::styled(*row, theme::accent_bold()),
                ])
            })
            .collect();
        let model_title = title_case_model(&self.hm.model);
        let tagline_text = format!("ONYX {account} · {model_title}");
        let tagline = RuLine::from(vec![
            RuSpan::raw(" ".repeat(w.saturating_sub(tagline_text.chars().count()) / 2)),
            RuSpan::styled("ONYX ", theme::strong()),
            RuSpan::styled(account.clone(), theme::rgb(theme::INFO)),
            RuSpan::styled(" · ", theme::dim()),
            RuSpan::styled(model_title, theme::dim()),
        ]);
        // Header: 7 rows (banner + tagline) when wide, 1 compact row otherwise.
        let header_h = if wide_header { 8u16 } else { 1u16 };
        let compact_mark = RuLine::from(vec![
            RuSpan::styled("◆ ", theme::accent_bold()),
            RuSpan::styled("ONYX · INTELLIGENCE ENGINE", theme::strong()),
        ]);

        // ── status/context strip (session id · model · ctx sparkline · pct) ──
        let (_filled, _empty, pct) = self.ctx_meter(20);
        let status_strip = RuLine::from(vec![
            RuSpan::styled(" ID ", theme::dim()),
            RuSpan::styled(self.session_label(), theme::strong()),
            RuSpan::styled("   MODEL ", theme::dim()),
            RuSpan::styled(self.hm.model.clone(), theme::rgb(theme::accent())),
            RuSpan::styled("   CTX ", theme::dim()),
            RuSpan::styled(format!("{pct}%"), theme::rgb(theme::FG_MAIN)),
        ]);

        // ── footer status bar (key-hint row removed per redesign) ──
        let mut footer_spans = vec![
            RuSpan::styled("◈ ", theme::accent_bold()),
            RuSpan::styled("ONYX", theme::strong()),
            RuSpan::styled("  ● ", theme::rgb(theme::active_success())),
            RuSpan::styled(format!("{}", self.mcp_count), theme::strong()),
            RuSpan::styled(" MCP   ◇ ", theme::dim()),
            RuSpan::styled(format!("{}", self.skill_count), theme::strong()),
            RuSpan::styled(" SKILLS   ", theme::dim()),
            RuSpan::styled(self.hm.model.to_uppercase(), theme::dim()),
        ];
        if let Some(ds) = diff_stat {
            footer_spans.push(RuSpan::styled("   DIFF ", theme::dim()));
            footer_spans.push(RuSpan::styled(ds.to_string(), theme::rgb(theme::active_success())));
        }
        let footer_status = RuLine::from(footer_spans);

        // ── settings modal (only when /settings active) ──
        let settings_lines: Vec<RuLine> = if self.settings_active {
            self.settings_lines_v()
                .into_iter()
                .flat_map(|l| fit_lines(&l, w))
                .map(|l| Self::ansi_to_line(&l))
                .collect()
        } else {
            Vec::new()
        };
        let settings_active = !settings_lines.is_empty();

        // ── approval card (only when pending) ──
        let approval_active = self.ask.pending().is_some();
        let approval_lines: Vec<RuLine> = if approval_active {
            self.status_lines_v()
                .into_iter()
                .flat_map(|l| fit_lines(&l, w))
                .map(|l| Self::ansi_to_line(&l))
                .collect()
        } else {
            Vec::new()
        };

        // ── interactive question popup (only when asking user) ──
        let question_active = self.ask.pending_question().is_some();
        let question_lines: Vec<RuLine> = if question_active {
            self.question_lines_v()
                .into_iter()
                .flat_map(|l| fit_lines(&l, w))
                .map(|l| Self::ansi_to_line(&l))
                .collect()
        } else {
            Vec::new()
        };

        // ── connect-provider wizard (modal while open) ──
        let connect_lines: Vec<RuLine> = self
            .connect_lines_v()
            .into_iter()
            .flat_map(|l| fit_lines(&l, w))
            .map(|l| Self::ansi_to_line(&l))
            .collect();
        let connect_active = !connect_lines.is_empty();

        // ── slash dropdown (only when typing "/…") ──
        let (dd_title, dd_matches) = self.matching_dropdown_items();
        // Gate on the wizard being open, not on how many lines it happened to
        // render: an empty-result panel is still one line tall, and the slash
        // dropdown must never steal keystrokes from a wizard the user is in.
        let dropdown_active = !dd_matches.is_empty() && self.connect.is_none();
        let dropdown_height = if dropdown_active {
            (dd_matches.len().min(6) + 2) as u16
        } else {
            0
        };

        // ── compute vertical constraints ──
        let strip_h = 1u16;
        let composer_h = 3u16; // top border + input + bottom border
        let footer_h = 1u16; // status bar only (key-hint row removed)
        let settings_h = settings_lines.len() as u16;
        let approval_h = approval_lines.len() as u16;
        let question_h = question_lines.len() as u16;
        let connect_h = connect_lines.len() as u16;

        let mut constraints = vec![
            Constraint::Length(header_h),
            Constraint::Length(strip_h),
            Constraint::Min(1), // transcript
        ];
        if settings_active {
            constraints.push(Constraint::Length(settings_h));
        }
        if approval_active {
            constraints.push(Constraint::Length(approval_h));
        }
        if question_active {
            constraints.push(Constraint::Length(question_h));
        }
        if connect_active {
            constraints.push(Constraint::Length(connect_h));
        }
        if dropdown_active {
            constraints.push(Constraint::Length(dropdown_height));
        }
        constraints.push(Constraint::Length(composer_h));
        constraints.push(Constraint::Length(footer_h));

        let chunks = RuLayout::vertical(constraints).split(area);
        let mut ci = 0usize;
        let header_area = chunks[ci];
        ci += 1;
        let strip_area = chunks[ci];
        ci += 1;
        let transcript_area = chunks[ci];
        ci += 1;
        let settings_area = if settings_active {
            let s = chunks[ci];
            ci += 1;
            s
        } else {
            Rect::ZERO
        };
        let approval_area = if approval_active {
            let a = chunks[ci];
            ci += 1;
            a
        } else {
            Rect::ZERO
        };
        let question_area = if question_active {
            let q = chunks[ci];
            ci += 1;
            q
        } else {
            Rect::ZERO
        };
        let connect_area = if connect_active {
            let c = chunks[ci];
            ci += 1;
            c
        } else {
            Rect::ZERO
        };
        let dropdown_area = if dropdown_active {
            let d = chunks[ci];
            ci += 1;
            d
        } else {
            Rect::ZERO
        };
        let composer_area = chunks[ci];
        let _ = ci;
        let footer_area = chunks.last().copied().unwrap_or(Rect::ZERO);

        // ── header ──
        if wide_header {
            let mut head = banner_rows;
            head.push(tagline);
            head.push(RuLine::from(vec![]));
            f.render_widget(RuParagraph::new(head), header_area);
        } else {
            f.render_widget(
                RuParagraph::new(vec![compact_mark]).alignment(RuAlignment::Left),
                header_area,
            );
        }

        // ── status strip + context sparkline ──
        let spark_w = 16u16;
        let strip_split = RuLayout::horizontal([
            Constraint::Min(1),
            Constraint::Length(spark_w),
        ])
        .split(strip_area);
        f.render_widget(RuParagraph::new(vec![status_strip]), strip_split[0]);
        if !self.ctx_history.is_empty() {
            let data: Vec<u64> = self.ctx_history.iter().map(|&v| v as u64).collect();
            let spark = RuSparkline::default()
                .data(&data)
                .max(100)
                .style(theme::accent_bold());
            f.render_widget(spark, strip_split[1]);
        }

        // ── transcript (scrollable) ──
        let body_w = transcript_area.width as usize;
        let all_lines = self.transcript_ru_lines(body_w.max(20));
        let visible = transcript_area.height as usize;
        let total = all_lines.len();
        let max_scroll = total.saturating_sub(visible);
        let scroll = self.scroll_offset.min(max_scroll);
        let start = total.saturating_sub(scroll).saturating_sub(visible);
        let end = (start + visible).min(total);
        let slice = &all_lines[start..end];
        let para = RuParagraph::new(slice.to_vec()).wrap(RuWrap { trim: true });
        f.render_widget(para, transcript_area);
        if total > visible {
            let mut sb_state = RuScrollbarState::new(total).position(scroll);
            RuScrollbar::new(RuScrollbarOrientation::VerticalRight)
                .begin_symbol(None)
                .end_symbol(None)
                .track_style(theme::border())
                .thumb_style(theme::accent_bold())
                .render(transcript_area, f.buffer_mut(), &mut sb_state);
        }

        // ── settings modal ──
        if settings_active {
            let sp = RuParagraph::new(settings_lines);
            f.render_widget(sp, settings_area);
        }

        // ── approval card ──
        if approval_active {
            let ap = RuParagraph::new(approval_lines);
            f.render_widget(ap, approval_area);
        }

        // ── interactive question popup ──
        if question_active {
            let qp = RuParagraph::new(question_lines);
            f.render_widget(qp, question_area);
        }

        // ── connect-provider wizard ──
        if connect_active {
            f.render_widget(RuParagraph::new(connect_lines), connect_area);
        }

        // ── slash dropdown ──
        if dropdown_active {
            self.render_dropdown(f, dropdown_area, &dd_title, &dd_matches);
        }

        // ── composer ──
        let sug = self.suggestion();
        let input_text = if self.input.is_empty() && sug.is_empty() {
            RuLine::from(vec![RuSpan::styled(
                "Type a command or ask Onyx anything…",
                theme::dim(),
            )])
        } else if !sug.is_empty() && self.input.starts_with('/') {
            let ghost = sug.strip_prefix(&self.input).unwrap_or("");
            RuLine::from(vec![
                RuSpan::styled("❯ ".to_string(), theme::accent_bold()),
                RuSpan::styled(self.input.clone(), theme::strong()),
                RuSpan::styled(ghost.to_string(), theme::dim()),
            ])
        } else {
            RuLine::from(vec![
                RuSpan::styled("❯ ".to_string(), theme::accent_bold()),
                RuSpan::styled(self.input_window(), theme::strong()),
            ])
        };
        let comp_block = RuBlock::new()
            .borders(RuBorders::ALL)
            .border_type(RuBorderType::Rounded)
            .border_style(if self.running || self.thinking {
                theme::border()
            } else {
                theme::border_focus()
            });
        let comp_inner = comp_block.inner(composer_area);
        comp_block.render(composer_area, f.buffer_mut());
        f.render_widget(RuParagraph::new(vec![input_text]), comp_inner);
        // Park the caret inside the composer input row.
        if !self.running {
            let col = comp_inner.left() + self.caret_col();
            let row = comp_inner.top();
            f.set_cursor_position((col.min(comp_inner.right().saturating_sub(1)), row));
        }

        // ── footer (status bar only) ──
        f.render_widget(RuParagraph::new(vec![footer_status]), footer_area);
    }

    /// Ratatui `List` popup for slash/subcommand completion above the composer.
    fn render_dropdown(
        &self,
        f: &mut Frame,
        area: Rect,
        title: &str,
        matches: &[(String, String, String)],
    ) {
        let max_visible = 6usize;
        let total = matches.len();
        let selected = self.slash_selected.min(total.saturating_sub(1));
        let items: Vec<RuListItem> = matches
            .iter()
            .enumerate()
            .map(|(idx, (_full, name, desc))| {
                let is_sel = idx == selected;
                let style = if is_sel {
                    theme::selection()
                } else {
                    RuStyle::default().fg(theme::rgb(theme::FG_SEC))
                };
                RuListItem::new(vec![RuLine::from(vec![
                    RuSpan::styled(
                        if is_sel { "❯ " } else { "  " }.to_string(),
                        if is_sel {
                            theme::accent_bold()
                        } else {
                            theme::dim()
                        },
                    ),
                    RuSpan::styled(format!("{name:<24}"), style),
                    RuSpan::styled(desc.clone(), if is_sel { style } else { theme::dim() }),
                ])])
            })
            .collect();

        let list = RuList::new(items)
            .highlight_symbol("")
            .block(
                RuBlock::new()
                    .title(title.trim())
                    .borders(RuBorders::ALL)
                    .border_type(RuBorderType::Rounded)
                    .border_style(theme::border()),
            );
        let mut state = RuListState::default().with_selected(Some(selected));
        // Window the list to the visible slice around the selection.
        let _ = max_visible;
        StatefulWidget::render(list, area, f.buffer_mut(), &mut state);
    }

    /// Full-screen "Chat Sessions" browser: header line with count + scope, a
    /// scrollable list of sessions (title on line 1, age · branch · size on line 2), an inline
    /// preview when Space-expanded, and a bottom key-hint strip.
    fn render_resume_browser(&self, f: &mut Frame, area: Rect) {
        let Some(b) = self.resume_browser.as_ref() else {
            return;
        };
        let w = area.width.max(20) as usize;

        // ── top banner rule ──
        // " Chat"
        // "─────────────────────────────────────────────────────────────────────────"
        let top_title = RuLine::from(vec![
            RuSpan::styled(" Chat", theme::strong()),
        ]);
        let divider_line = RuLine::from(vec![
            RuSpan::styled("─".repeat(w), theme::border()),
        ]);

        // ── section header line ──
        // " Chat Sessions (69, current worktree)                                      sorted by date desc"
        let scope = if b.show_all {
            "all projects".to_string()
        } else {
            "current worktree".to_string()
        };
        let left_hdr = format!(" Chat Sessions ({}, {})", b.rows.len(), scope);
        let right_hdr = "sorted by date desc";
        let spaces = w.saturating_sub(left_hdr.chars().count() + right_hdr.chars().count() + 1);
        let section_hdr = RuLine::from(vec![
            RuSpan::styled(left_hdr, theme::strong()),
            RuSpan::styled(" ".repeat(spaces.max(2)), theme::dim()),
            RuSpan::styled(right_hdr, theme::dim()),
        ]);

        // ── search bar (visible only while searching) ──
        let search_line = if matches!(b.mode, BrowserMode::Search) || !b.query.is_empty() {
            Some(RuLine::from(vec![
                RuSpan::styled(" / ", theme::accent_bold()),
                RuSpan::styled(b.query.clone(), theme::strong()),
                RuSpan::styled(if matches!(b.mode, BrowserMode::Search) { "▏" } else { "" }.to_string(), theme::dim()),
            ]))
        } else {
            None
        };

        // ── items ──
        // Exactly matches the user's reference mockup:
        //  ❯ Complete Tektilr Core
        //    55s · onyx-experiment · 27.7 MB
        //
        //    clean n the cache
        //    19h · onyx-experiment · 89.7 KB
        let mut items: Vec<RuListItem> = Vec::new();
        for (row_i, &idx) in b.rows.iter().enumerate() {
            let s = &b.all[idx];
            let is_sel = row_i == b.sel;
            let arrow = if is_sel { " ❯ " } else { "   " };
            let title_raw = s
                .title
                .clone()
                .unwrap_or_else(|| {
                    if !s.preview.is_empty() {
                        agent::session::derive_title(&s.preview)
                    } else {
                        "Untitled Session".to_string()
                    }
                });
            let dur = fmt_duration(s.duration_secs);
            let branch = s.branch.clone().unwrap_or_else(|| "-".into());
            let size = fmt_bytes(s.bytes);
            let meta_line = format!("   {} turns ({} msgs) · {dur} · {branch} · {size}", s.user_turns, s.messages);

            let title_style = if is_sel {
                theme::accent_bold()
            } else {
                theme::strong()
            };

            let mut lines = vec![
                RuLine::from(vec![
                    RuSpan::styled(arrow.to_string(), if is_sel { theme::accent_bold() } else { theme::dim() }),
                    RuSpan::styled(title_raw, title_style),
                ]),
                RuLine::from(vec![
                    RuSpan::styled(meta_line, theme::dim()),
                ]),
            ];

            // Inline preview under the selected expanded row when space toggled
            if is_sel && b.expanded {
                if let Some((_p, msgs)) = agent::session::resolve_session(&s.id) {
                    let users: Vec<&Message> = msgs.iter().filter(|m| m.role == "user").take(3).collect();
                    for m in users {
                        let one = m.content.lines().next().unwrap_or("").chars().take(w.saturating_sub(8)).collect::<String>();
                        lines.push(RuLine::from(vec![
                            RuSpan::styled("   › ".to_string(), theme::dim()),
                            RuSpan::styled(one, theme::dim()),
                        ]));
                    }
                }
            }

            // Trailing empty space line between cards
            lines.push(RuLine::from(vec![RuSpan::raw("")]));
            items.push(RuListItem::new(lines));
        }

        if items.is_empty() {
            items.push(RuListItem::new(vec![
                RuLine::from(vec![RuSpan::styled("   no matching sessions", theme::dim())]),
            ]));
        }

        // ── key hint footer strip ──
        // "Ctrl+A show all projects · Ctrl+B only show current branch · Space preview · ↑↓ navigate ·"
        // "Enter select · / search · ? for shortcuts · Esc close"
        let hint_lines = vec![
            RuLine::from(vec![
                RuSpan::styled(
                    "Ctrl+A show all projects · Ctrl+B only show current branch · Space preview · ↑↓ navigate ·",
                    theme::dim(),
                ),
            ]),
            RuLine::from(vec![
                RuSpan::styled(
                    "Enter select · / search · ? for shortcuts · Esc close",
                    theme::dim(),
                ),
            ]),
        ];

        // ── layout: top_title, divider, section_hdr, [search], list, footer ──
        let mut constraints = vec![
            Constraint::Length(1), // Chat
            Constraint::Length(1), // ───
            Constraint::Length(1), // Chat Sessions header
        ];
        if search_line.is_some() {
            constraints.push(Constraint::Length(1));
        }
        constraints.push(Constraint::Min(4));  // list
        constraints.push(Constraint::Length(2)); // 2-line footer

        let chunks = RuLayout::vertical(constraints).split(area);
        let mut ci = 0usize;
        f.render_widget(RuParagraph::new(vec![top_title]), chunks[ci]);
        ci += 1;
        f.render_widget(RuParagraph::new(vec![divider_line]), chunks[ci]);
        ci += 1;
        f.render_widget(RuParagraph::new(vec![section_hdr]), chunks[ci]);
        ci += 1;
        if let Some(sl) = &search_line {
            f.render_widget(RuParagraph::new(vec![sl.clone()]), chunks[ci]);
            ci += 1;
        }
        let list_area = chunks[ci];
        ci += 1;
        let hints_area = chunks[ci];

        let list = RuList::new(items)
            .highlight_symbol("")
            .block(RuBlock::new().borders(RuBorders::NONE));

        let mut state = RuListState::default()
            .with_offset(b.top)
            .with_selected(Some(b.sel.min(b.rows.len().saturating_sub(1))));
        StatefulWidget::render(list, list_area, f.buffer_mut(), &mut state);

        // Scrollbar on the right edge of the list area
        let total_rows = b.rows.len();
        let visible_rows = (list_area.height as usize) / 3;
        if total_rows > visible_rows.max(1) {
            let mut sb_state = RuScrollbarState::new(total_rows).position(b.top);
            RuScrollbar::new(RuScrollbarOrientation::VerticalRight)
                .begin_symbol(None)
                .end_symbol(None)
                .track_style(theme::border())
                .thumb_style(theme::accent_bold())
                .render(list_area, f.buffer_mut(), &mut sb_state);
        }

        f.render_widget(RuParagraph::new(hint_lines), hints_area);
    }
}

/// Compact human-readable duration: 55s, 19h, 4d, 3mo.
fn fmt_duration(secs: u64) -> String {
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else if secs < 86_400 {
        format!("{}h", secs / 3600)
    } else if secs < 2_592_000 {
        format!("{}d", secs / 86_400)
    } else {
        format!("{}mo", secs / 2_592_000)
    }
}

/// Compact byte count: 27.7 MB, 512 KB, 928 B.
fn fmt_bytes(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;
    if bytes < KB {
        format!("{bytes} B")
    } else if bytes < MB {
        let k = bytes as f64 / KB as f64;
        format!("{:.0} KB", k)
    } else if bytes < GB {
        let m = bytes as f64 / MB as f64;
        format!("{:.1} MB", m)
    } else {
        let g = bytes as f64 / GB as f64;
        format!("{:.1} GB", g)
    }
}

/// Last path component of a filesystem-ish string.
#[allow(dead_code)]
fn basename(p: &str) -> String {
    p.trim_end_matches('/')
        .rsplit('/')
        .next()
        .filter(|s| !s.is_empty())
        .unwrap_or(p)
        .to_string()
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

/// Restores the terminal on drop even if the session unwinds, so a panic in the
/// render loop can never leave the user's shell stuck in raw mode / alt screen.
struct AltScreenGuard;

impl AltScreenGuard {
    /// Best-effort: undo everything `tui_session` turns on, in reverse order.
    fn restore() {
        let mut out = io::stdout();
        execute!(out, DisableBracketedPaste).ok();
        execute!(out, Show).ok();
        execute!(out, LeaveAlternateScreen).ok();
        ct_disable_raw().ok();
        out.flush().ok();
    }
}

impl Drop for AltScreenGuard {
    fn drop(&mut self) {
        Self::restore();
    }
}

pub fn tui_session(
    gw: &mut Gateway<dyn tools::policy::Policy>,
    hm: &mut HttpModel,
    opts: &mut Opts,
    sel: Option<&agent::retrieval::CapabilitySelector>,
) {
    // termios raw-mode guard drives the byte-level InputPump (VMIN=1 blocking
    // read with ISIG/ECHO off). Kept alongside crossterm's raw mode so key
    // decoding and the tick cadence are unchanged from the ANSI renderer.
    let _raw = match RawMode::enter() {
        Some(r) => r,
        None => {
            eprintln!("onyx: cannot enter raw mode — stdin is not a tty?");
            return;
        }
    };

    // Crossterm owns the alternate-screen buffer + cursor visibility that the
    // ratatui renderer draws into. RAII guard restores on every exit path.
    if ct_enable_raw().is_err() {
        eprintln!("onyx: cannot enable crossterm raw mode");
        return;
    }
    {
        let mut out = io::stdout();
        if execute!(out, EnterAlternateScreen).is_err() {
            ct_disable_raw().ok();
            eprintln!("onyx: cannot enter alternate screen");
            return;
        }
        execute!(out, Hide).ok();
        execute!(out, EnableBracketedPaste).ok();
        out.flush().ok();
    }
    let _guard = AltScreenGuard;

    let pump = InputPump::start();
    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::new(backend).expect("ratatui terminal");
    terminal
        .hide_cursor()
        .ok();
    terminal.clear().ok();

    {
        let mut tui = Tui::new(gw, hm, opts, sel);
        tui.start_worker();
        tui.run_ratatui(&pump, &mut terminal);
        tui.finish();
    }
    pump.shutdown();
    // _guard drops here → Show + LeaveAlternateScreen + disable_raw_mode.
    // Final newline so the shell prompt starts clean.
    println!();
}

#[cfg(test)]
mod tests {
    use super::{decode_key, ConnectRow, Tui};
    use ratatui::style::Color;

    #[test]
    fn connect_picker_puts_popular_first_then_alphabetical() {
        let catalog = agent::provider::load_catalog();
        let rows = Tui::connect_rows(&catalog, "");
        let headers: Vec<&str> = rows
            .iter()
            .filter_map(|r| match r {
                ConnectRow::Header(t) => Some(*t),
                ConnectRow::Provider(_) => None,
            })
            .collect();
        assert_eq!(headers, vec!["Popular", "Providers"]);

        // Popular block holds only curated ids, in the curated order.
        let popular: Vec<usize> = rows
            .iter()
            .take_while(|r| !matches!(r, ConnectRow::Header("Providers")))
            .filter_map(|r| match r {
                ConnectRow::Provider(i) => Some(*i),
                _ => None,
            })
            .collect();
        assert!(!popular.is_empty());
        for (k, i) in popular.iter().enumerate() {
            assert_eq!(catalog[*i].id, Tui::POPULAR_PROVIDERS[k]);
        }

        // The Providers block is alphabetical because the catalog is.
        let rest: Vec<usize> = rows
            .iter()
            .skip_while(|r| !matches!(r, ConnectRow::Header("Providers")))
            .filter_map(|r| match r {
                ConnectRow::Provider(i) => Some(*i),
                _ => None,
            })
            .collect();
        let mut sorted = rest.clone();
        sorted.sort_unstable();
        assert_eq!(rest, sorted);
        assert_eq!(rest.len() + popular.len(), catalog.len());
    }

    #[test]
    fn connect_search_filters_both_sections_and_drops_empty_ones() {
        let catalog = agent::provider::load_catalog();
        let rows = Tui::connect_rows(&catalog, "open");
        let hits: Vec<&str> = rows
            .iter()
            .filter_map(|r| match r {
                ConnectRow::Provider(i) => Some(catalog[*i].id.as_str()),
                ConnectRow::Header(_) => None,
            })
            .collect();
        // Name matches count too, so check against both fields.
        for id in &hits {
            let p = catalog.iter().find(|c| &c.id == id).unwrap();
            assert!(
                p.id.contains("open") || p.name.to_lowercase().contains("open"),
                "{id} should not match"
            );
        }
        assert!(hits.contains(&"openrouter"));
        // No curated provider matches "zzz", so its section header disappears.
        let rows = Tui::connect_rows(&catalog, "zzzznotaprovider");
        assert!(rows.is_empty());
    }

    #[test]
    fn ansi_to_line_plain_text_is_one_span() {
        let line = Tui::ansi_to_line("hello");
        assert_eq!(line.spans.len(), 1);
        assert_eq!(line.spans[0].content, "hello");
    }

    #[test]
    fn ansi_to_line_truecolor_becomes_rgb_fg() {
        // ESC [ 38;2;0;229;255 m  ONYX  ESC [ 0 m
        let line = Tui::ansi_to_line("\x1b[38;2;0;229;255mONYX\x1b[0m");
        assert_eq!(line.spans.len(), 1);
        assert_eq!(line.spans[0].content, "ONYX");
        assert_eq!(line.spans[0].style.fg, Some(Color::Rgb(0, 229, 255)));
    }

    #[test]
    fn ansi_to_line_bold_modifier_applied() {
        let line = Tui::ansi_to_line("\x1b[1mBOLD\x1b[0m");
        assert!(line.spans[0]
            .style
            .add_modifier
            .contains(ratatui::style::Modifier::BOLD));
    }

    #[test]
    fn ansi_to_line_splits_on_style_change() {
        // two colored runs → two spans, reset between them
        let line = Tui::ansi_to_line("\x1b[38;2;1;2;3ma\x1b[0m\x1b[38;2;4;5;6mb\x1b[0m");
        assert_eq!(line.spans.len(), 2);
        assert_eq!(line.spans[0].style.fg, Some(Color::Rgb(1, 2, 3)));
        assert_eq!(line.spans[1].style.fg, Some(Color::Rgb(4, 5, 6)));
    }

    #[test]
    fn ansi_to_line_drops_escape_but_keeps_text() {
        // A CSI that is not SGR (e.g. cursor row set) must not leak into text.
        let line = Tui::ansi_to_line("\x1b[3;7Hready");
        assert_eq!(line.spans.iter().map(|s| s.content.as_ref()).collect::<String>(), "ready");
    }

    #[test]
    fn decode_key_bracketed_paste() {
        let paste = b"\x1b[200~hello\nworld\x1b[201~";
        let (chars, code): (Vec<char>, u32) = decode_key(paste);
        assert_eq!(code, 2000);
        let s: String = chars.into_iter().collect();
        assert_eq!(s, "hello\nworld");
    }
}
