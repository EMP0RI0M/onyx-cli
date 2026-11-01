//! onyx TUI — an inline terminal UI modeled on the architecture extracted from
//! the agy CLI (Bubble Tea MVU + Lip Gloss), reimplemented clean-room with raw
//! ANSI so the workspace gains no new dependencies.
//!
//! agy patterns reproduced here:
//!   - inline viewport: finalized lines print into the native scrollback (agy's
//!     cursedRenderer insertAbove); only the live tail (streaming text, spinner,
//!     input card, hints) is redrawn in place;
//!   - collapsible tool groups: `⏺ name(args)` + `(ctrl+o to expand)` and a
//!     `<output +N lines>` truncation hint; ctrl+o toggles globally;
//!   - shift+tab cycles approval modes (normal → accept-edits → auto);
//!   - "Thinking…" spinner while a model step runs, "(esc to cancel)" beside it;
//!   - key-hint footer bar; Ctrl+D twice (or /exit) quits;
//!   - slash autocomplete on ↑/↓/Tab as you type "/…";
//!   - inline confirmation cards ("y run · a always · n/N refuse") wired into
//!     the Gateway approver slot.
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

// ---------------------------------------------------------------- ANSI/TTY ----

const RESET: &str = "\x1b[0m";
const BOLD: &str = "\x1b[1m";
const DIM: &str = "\x1b[2m";
const ITALIC: &str = "\x1b[3m";
const FG_245: &str = "\x1b[38;5;245m";
const FG_250: &str = "\x1b[38;5;250m";
const GREEN: &str = "\x1b[32m";
const RED: &str = "\x1b[31m";
const YELLOW: &str = "\x1b[33m";
const BLUE: &str = "\x1b[34m";
const MAGENTA: &str = "\x1b[35m";
const CYAN: &str = "\x1b[36m";

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
    if unsafe { libc::ioctl(1, libc::TIOCGWINSZ, &mut ws) } == 0 && ws.ws_row > 4 {
        (ws.ws_row - 4, ws.ws_col)
    } else {
        (24, 80)
    }
}

/// Terminal cells a char occupies: CJK/fullwidth blocks are 2, combining marks
/// and control-ish codepoints are 0, everything else 1. Approximate wcwidth —
/// enough for the box-drawing, braille spinner and CJK we can actually print.
fn char_width(c: char) -> usize {
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
            if c.is_ascii_alphabetic() {
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

/// Hard-wrap one plain-text line at exactly `width` CELLS, breaking wide glyphs
/// rather than letting the terminal soft-wrap (which would desync our cursor
/// rewind from the rows actually consumed). Returns at least one line.
fn hard_wrap_line(text: &str, width: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut used = 0usize;
    for c in text.chars() {
        let cw = char_width(c).min(width);
        if used + cw > width {
            out.push(std::mem::take(&mut cur));
            used = 0;
        }
        cur.push(c);
        used += cw;
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
    SetModel(String),
    SystemAppend(String),
    Audit,
}

/// Events the worker thread sends to the UI thread.
enum UiOut {
    Ev(AgentEvent),
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

fn pretty_args(args: &str) -> String {
    let v: serde_json::Value = serde_json::from_str(args).unwrap_or_else(|_| serde_json::json!(args));
    let obj = match v.as_object() {
        Some(o) => o,
        None => return clip_chars(&v.to_string(), 100),
    };
    // bash/python/editor-style single-string args render bare, like agy does.
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
    ("/steps", "N set max steps"),
    ("/clear", "new conversation"),
    ("/usage", "token estimate + audit summary"),
    ("/audit", "gateway audit log"),
    ("/tools", "list connected tools"),
    ("/skill", "<name> load a skill"),
    ("/mode", "cycle approval mode"),
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
}


impl<'a> Tui<'a> {
    fn new(
        gw: &'a mut Gateway<dyn tools::policy::Policy>,
        hm: &'a mut HttpModel,
        opts: &'a mut Opts,
    ) -> Self {
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
            height: term_size().0 as usize,
            width: 80.max(term_size().1 as usize - 1),
        }
    }

    /// Refresh the viewport geometry (the user may have resized the window).
    fn sync_size(&mut self) {
        let (rows, cols) = term_size();
        self.height = rows as usize;
        self.width = 80.max(cols as usize - 1);
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
                        let mut agent = ToolCallAgent::new(gw);
                        agent.max_steps = max_steps;
                        let mut sink = |ev: AgentEvent| {
                            let _ = ut.send(UiOut::Ev(ev));
                        };
                        let res = agent.run_turn_observed(hm, &prompt, &mut history, &mut sink);
                        if *cancel.lock().unwrap() {
                            // Drop everything this cancelled turn appended.
                            history.truncate(cut);
                        }
                        let audit = gw.audit.snapshot();
                        let _ = ut.send(UiOut::TurnDone(match res {
                            Ok(rep) => Ok((rep.answer, rep.steps)),
                            Err(e) => Err(e),
                        }));
                        let _ = ut.send(UiOut::Audit(audit));
                    }
                    Ctl::Clear(system) => {
                        history = vec![Message::text("system", system)];
                    }
                    Ctl::SetModel(m) => {
                        hm.model = m;
                        let _ = ut.send(UiOut::ModelName(hm.model.clone()));
                    }
                    Ctl::SystemAppend(s) => {
                        hm.system.push_str(&s);
                    }
                    Ctl::Audit => {
                        let _ = ut.send(UiOut::Audit(gw.audit.snapshot()));
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
        let n = lines.len().min(self.height.max(2)); // frame never exceeds viewport
        let mut out = String::new();
        out.push_str("\x1b[H\x1b[J"); // home + clear entire visible screen
        for l in &lines[..n] {
            out.push_str(l);
            out.push('\n');
        }
        if n >= 2 {
            // Park on the input line: 2nd from the bottom of the frame.
            let col = (3 + self.cursor_chars()).min(self.width.max(1));
            out.push_str(&format!("\x1b[{};{}H", n - 1, col));
        }
        io::stdout().write_all(out.as_bytes()).ok();
        io::stdout().flush().ok();
    }

    fn render_lines(&self) -> Vec<String> {
        let mut lines: Vec<String> = Vec::new();
        // finalized transcript window (bottom-aligned within the reserved area)
        let mut fin: Vec<String> = Vec::new();
        for it in &self.items {
            fin.extend(self.item_lines(it));
        }
        if self.tail_visible() {
            fin.extend(self.tail_lines());
        }
        // Pre-wrap every entry to one display row each, then size the window.
        let w = self.width.max(20);
        let fin: Vec<String> = fin.iter().flat_map(|l| fit_lines(l, w)).collect();
        let status: Vec<String> = self.status_lines_v().iter().flat_map(|l| fit_lines(l, w)).collect();
        let input: Vec<String> = self.input_lines().iter().flat_map(|l| fit_lines(l, w)).collect();
        let hints: Vec<String> = fit_lines(&self.hints(), w);
        let reserve = status.len() + input.len() + hints.len() + 1; // +1 spinner slot
        let avail = self.height.saturating_sub(reserve).max(1);
        // Keep a whole-line suffix of `fin`: the finalized transcript must end
        // with its LAST line (the just-finished answer/tool row), so trim from
        // the front instead of letting a tail slice drop it.
        let mut keep: &[String] = &fin;
        if keep.len() > avail {
            keep = &keep[keep.len() - avail..];
        }
        let dropped = fin.len() - keep.len();
        // The spinner may add a row; if the frame would exceed the viewport,
        // give it that row by dropping one more transcript line.
        let spin_row = if self.thinking && keep.len() + reserve > self.height {
            if !keep.is_empty() {
                keep = &keep[1..];
            }
            true
        } else {
            self.thinking
        };
        if dropped > 0 {
            lines.push(format!("{FG_245}  … {} earlier line(s){RESET}", fin.len() - keep.len()));
        }
        lines.extend_from_slice(keep);

        if spin_row {
            let spin = format!(
                "{DIM}{spin} Thinking… {RESET}{DIM}(esc to cancel){RESET}",
                spin = SPINNER[self.tick % SPINNER.len()]
            );
            lines.extend(fit_lines(&spin, w));
        }
        lines.extend(status);
        lines.extend(input);
        lines.extend(hints);
        lines
    }

    fn tail_visible(&self) -> bool {
        self.running && !self.tail.is_empty()
    }

    fn tail_lines(&self) -> Vec<String> {
        let vis: String = self.tail.chars().skip(self.tail_scrolled).collect();
        let mut out = Vec::new();
        for para in vis.split('\n') {
            let wrapped = wrap(para, 76);
            for (i, w) in wrapped.iter().enumerate() {
                let mark = if i == 0 { "◐ " } else { "  " };
                out.push(format!("{FG_250}{mark}{t}{RESET}", t = w));
            }
        }
        out
    }

    fn item_lines(&self, it: &Item) -> Vec<String> {
        match it {
            Item::Banner => vec![format!(
                "{BOLD}{BLUE}◆ onyx{RESET} {BOLD}agentic coding CLI{RESET} {DIM}· agy-style TUI{RESET}"
            )],
            Item::User(t) => {
                let mut out = Vec::new();
                for (i, l) in fit_lines(t, self.width.max(20)).into_iter().enumerate() {
                    let mark = if i == 0 { "❯" } else { " " };
                    out.push(format!("{CYAN}{mark} {l}{RESET}"));
                }
                out
            }
            Item::Tool { name, args, ok, denied, output } => {
                let marker = if *denied {
                    format!("{RED}✗{RESET}")
                } else if *ok {
                    format!("{GREEN}⏺{RESET}")
                } else {
                    format!("{YELLOW}⏺{RESET}")
                };
                let mut out = vec![format!(
                    "{marker} {BOLD}{name}{RESET}{DIM}({a}){RESET}",
                    a = pretty_args(args)
                )];
                if self.expanded && !output.is_empty() {
                    let mut olines: Vec<String> = Vec::new();
                    for ol in output.lines().take(24) {
                        olines.extend(fit_lines(ol, self.width.max(20)));
                    }
                    if output.lines().count() > 24 {
                        olines.push("…".into());
                    }
                    for ol in olines {
                        out.push(format!("{FG_245}  │ {ol}{RESET}"));
                    }
                } else if !self.expanded && !output.is_empty() {
                    let extra = output.lines().count().saturating_sub(1);
                    if extra > 0 {
                        out.push(format!("{DIM}<output +{extra} lines> {FG_245}(ctrl+o to expand){RESET}"));
                    } else {
                        out.push(format!("{DIM}│ {} {FG_245}(ctrl+o to expand){RESET}", clip_chars(first_line(output), 70)));
                    }
                }
                out
            }
            Item::Answer(t) => {
                let mut out = vec![format!("{BOLD}◆ onyx{RESET}")];
                for para in t.split('\n') {
                    for w in wrap(para, 76) {
                        out.push(format!("{FG_250}  {w}{RESET}"));
                    }
                }
                out
            }
            Item::Info(t) => fit_lines(t, self.width.max(20))
                .into_iter()
                .map(|l| format!("{FG_245}{l}{RESET}"))
                .collect(),
            Item::Error(t) => fit_lines(t, self.width.max(20))
                .into_iter()
                .map(|l| format!("{RED}{l}{RESET}"))
                .collect(),
        }
    }

    fn status_lines_v(&self) -> Vec<String> {
        let mut out = Vec::new();
        if let Some((tool, args, reason)) = self.ask.pending() {
            out.push(format!(
                "{YELLOW}┌ permission needed ─────────────────────────────{RESET}"
            ));
            out.push(format!(
                "{YELLOW}│{RESET} {BOLD}{tool}{RESET}{DIM}({a}){RESET}",
                a = pretty_args(&args)
            ));
            out.push(format!("{YELLOW}│{RESET} {DIM}{reason}{RESET}"));
            out.push(format!(
                "{YELLOW}└{RESET} {BOLD}y{RESET} run · {BOLD}a{RESET} always · {BOLD}n{RESET}/{BOLD}N{RESET} refuse"
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
            format!("{DIM}ask onyx… (enter to send, alt+enter newline){RESET}")
        } else if self.cursor_chars() + 2 >= self.width || self.input.contains('\n') {
            // Long input scrolls a one-row window around the caret instead of
            // wrapping (a wrapped row would break the fixed "input line is
            // 2nd from bottom" rewind contract).
            let win = self.input_window();
            return vec![format!("{BOLD}{CYAN}❯{RESET} {win}")];
        } else {
            format!("{}", self.input)
        };
        out.push(format!("{BOLD}{CYAN}❯{RESET} {body}"));
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
        let mode = self.gw.mode.lock().unwrap().label();
        let mode_color = match *self.gw.mode.lock().unwrap() {
            ApprovalMode::Normal => FG_245,
            ApprovalMode::AcceptEdits => YELLOW,
            ApprovalMode::Auto => RED,
        };
        format!(
            "{DIM}╰─ {RESET}{mode_color}[{mode}]{RESET}{DIM} shift+tab cycle · ctrl+o {exp} · esc cancel · ctrl+d×2 exit · /help{RESET}",
            exp = if self.expanded { "collapse" } else { "expand" }
        )
    }

    // ------------------------------------------------------------ turn flow ----

    fn submit(&mut self) {
        let text = self.input.clone();
        self.input.clear();
        self.cursor = 0;
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
            "/model" => {
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
                let est = self.history_bytes / 4;
                let calls = self.gw.audit.snapshot().len();
                self.items.push(Item::Info(format!(
                    "~{est} tokens in context · {} message(s) · {calls} audited call(s)",
                    self.history_len
                )));
            }
            "/audit" => {
                if let Some(tx) = &self.ctl_tx {
                    let _ = tx.send(Ctl::Audit);
                }
                self.items.push(Item::Info("audit requested (see transcript below)".into()));
            }
            "/tools" => {
                for n in self.gw.tools.names() {
                    self.items.push(Item::Info(format!("  • {n}")));
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
            "/help" => {
                self.items.push(Item::Info(
                    "commands: /model /steps /clear /usage /audit /tools /skill /mode /exit".into(),
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
                self.finalize_tail();
                self.thinking = false; // a card or the tool row is what's live now
                self.items.push(Item::Tool {
                    name,
                    args: args.to_string(),
                    ok: false,
                    denied: false,
                    output: String::new(),
                });
            }
            AgentEvent::ToolDone { name, ok, output } => {
                let denied = output.starts_with("denied:") || output.contains("confirmation refused");
                for it in self.items.iter_mut().rev() {
                    if let Item::Tool { name: n, ok: o, denied: d, output: out, .. } = it {
                        if n == &name && out.is_empty() {
                            *o = ok;
                            *d = denied;
                            *out = output;
                            break;
                        }
                    }
                }
                self.thinking = true;
            }
        }
    }

    fn finalize_tail(&mut self) {
        if !self.tail.trim().is_empty() {
            let t = std::mem::take(&mut self.tail);
            self.items.push(Item::Answer(t.trim().to_string()));
        }
        self.tail_scrolled = 0;
    }

    fn on_turn_done(&mut self, res: Result<(String, usize), AgentError>) {
        self.finalize_tail();
        self.running = false;
        self.thinking = false;
        match res {
            Ok((answer, steps)) => {
                let dup = self.items.iter().rev().any(|i| matches!(i, Item::Answer(t) if *t == answer));
                if !answer.trim().is_empty() && !dup {
                    self.items.push(Item::Answer(answer));
                }
                self.items.push(Item::Info(format!("steps {steps}")));
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
                        let sug = self.suggestion();
                        if !sug.is_empty() {
                            let clean: String = sug.chars().take_while(|c| *c != ' ').collect();
                            self.input = clean;
                            self.cursor = self.input.len();
                        }
                    }
                    ('\r', _) => self.submit(),
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
                        }
                    }
                    ('\0', 1004) => {
                        // delete key
                        if self.cursor < self.input.len() {
                            let next = self.input[self.cursor..].chars().next().map(|c| c.len_utf8()).unwrap_or(0);
                            self.input.replace_range(self.cursor..self.cursor + next, "");
                        }
                    }
                    ('\0', 1000) => {
                        // up: nothing yet (history recall could go here)
                    }
                    ('\0', 1001) => {}
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
                        self.input.insert(self.cursor, c);
                        self.cursor += c.len_utf8();
                        self.last_ctrl_d = None;
                    }
                    _ => {}
                }
            } else {
                for &c in &chars {
                    self.input.insert(self.cursor, c);
                    self.cursor += c.len_utf8();
                }
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
    write!(io::stdout(), "\x1b[?25l").ok(); // hide cursor while painting
    {
        let mut tui = Tui::new(gw, hm, opts);
        tui.start_worker();
        tui.run(&pump);
        write!(io::stdout(), "\x1b[?25h").ok(); // restore cursor
        tui.finish();
    }
    pump.shutdown();
    // Final newline so the shell prompt starts clean.
    println!();
}
