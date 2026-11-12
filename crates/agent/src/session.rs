//! Durable conversation transcripts.
//!
//! Each interactive run is written as append-only JSON Lines under
//! `$ONYX_HOME/sessions` (default `~/.onyx/sessions`). A transcript is one file
//! per session: a header record followed by one record per message. Keeping it
//! line-oriented means a crash mid-turn only costs the last partial line, and
//! everything before it still loads. No external storage engine — just the
//! standard library plus serde_json, which the workspace already carries.

use crate::{Message, ToolCall};
use serde_json::{json, Value};
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Root directory holding all transcripts, honouring `ONYX_HOME`.
pub fn sessions_dir() -> PathBuf {
    let base = match std::env::var("ONYX_HOME") {
        Ok(h) if !h.is_empty() => PathBuf::from(h),
        _ => home_dir().map(|h| h.join(".onyx")).unwrap_or_else(|| PathBuf::from(".onyx")),
    };
    base.join("sessions")
}

fn home_dir() -> Option<PathBuf> {
    std::env::var("HOME").ok().filter(|h| !h.is_empty()).map(PathBuf::from)
}

/// A stable-ish session id: seconds-since-epoch plus a nanosecond component so
/// two sessions started in the same second still differ.
pub fn new_session_id() -> String {
    let nanos =
        SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(0);
    format!("{}-{:x}", now_secs(), nanos)
}

/// Append-only handle to one session's transcript file.
pub struct Transcript {
    pub path: PathBuf,
}

/// The workspace directory recorded for scoping `/resume` to the current
/// project. Best-effort: `None` when cwd cannot be read.
pub fn detect_cwd() -> Option<String> {
    std::env::current_dir().ok().map(|p| p.to_string_lossy().into_owned())
}

/// Best-effort git branch name for `cwd`, via `git rev-parse --abbrev-ref HEAD`.
/// Returns `None` off a repo / without git; never blocks on a missing binary.
fn detect_branch(cwd: Option<&str>) -> Option<String> {
    let mut cmd = std::process::Command::new("git");
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    let out = cmd
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if s.is_empty() || s == "HEAD" {
        None
    } else {
        Some(s)
    }
}

impl Transcript {
    /// Create (or reopen) the transcript for `id`, writing a header record the
    /// first time. Returns `None` if the directory could not be created — a
    /// failure to persist must never abort an interactive session. Records the
    /// current workspace + git branch so `/resume` can scope by project.
    pub fn open(id: &str, model: &str) -> Option<Self> {
        Self::open_in(id, model, detect_cwd().as_deref())
    }

    /// Like [`Transcript::open`] but records an explicit `cwd` (and its detected
    /// branch) instead of probing the process working directory.
    pub fn open_in(id: &str, model: &str, cwd: Option<&str>) -> Option<Self> {
        let dir = sessions_dir();
        if fs::create_dir_all(&dir).is_err() {
            return None;
        }
        let path = dir.join(format!("{id}.jsonl"));
        let fresh = !path.exists();
        let mut f = fs::OpenOptions::new().create(true).append(true).open(&path).ok()?;
        if fresh {
            let mut rec = json!({ "kind": "meta", "model": model, "ts": now_secs() });
            if let Some(c) = cwd {
                rec["cwd"] = json!(c);
                if let Some(b) = detect_branch(Some(c)) {
                    rec["branch"] = json!(b);
                }
            }
            if let Ok(line) = serde_json::to_string(&rec) {
                let _ = writeln!(f, "{line}");
            }
        }
        Some(Self { path })
    }

    /// Append one message as its own record. Best-effort: write errors are
    /// swallowed so a full disk degrades to "no persistence", not a crash.
    pub fn push(&self, msg: &Message) {
        // If this is the first user prompt and meta header has no title, generate and update title
        if msg.role == "user" && session_title(&self.path).is_none() {
            let title = derive_title(&msg.content);
            let _ = update_session_title(&self.path, &title);
        }

        let mut rec = json!({
            "kind": "message",
            "role": msg.role,
            "content": msg.content,
            "ts": now_secs(),
        });
        if let Some(tc) = &msg.tool_call {
            rec["tool_call"] = json!({ "name": tc.name, "args": tc.args });
        }
        if let (Ok(mut f), Ok(line)) =
            (fs::OpenOptions::new().create(true).append(true).open(&self.path), serde_json::to_string(&rec))
        {
            let _ = writeln!(f, "{line}");
        }
    }

    /// Append a thinking / reasoning trace to the transcript so it survives TUI restarts.
    pub fn push_thinking(&self, thinking: &str) {
        self.push_step_thinking(0, thinking);
    }

    /// Append a per-step thinking trace with explicit step identifier.
    pub fn push_step_thinking(&self, step: usize, thinking: &str) {
        if thinking.trim().is_empty() {
            return;
        }
        let rec = json!({
            "kind": "thinking",
            "step": step,
            "content": thinking,
            "ts": now_secs(),
        });
        if let (Ok(mut f), Ok(line)) =
            (fs::OpenOptions::new().create(true).append(true).open(&self.path), serde_json::to_string(&rec))
        {
            let _ = writeln!(f, "{line}");
        }
    }

    /// Append a structured operational event (such as StepCompleted or StallDetected).
    pub fn push_event(&self, kind: &str, step: usize, detail: &str) {
        let rec = json!({
            "kind": kind,
            "step": step,
            "content": detail,
            "ts": now_secs(),
        });
        if let (Ok(mut f), Ok(line)) =
            (fs::OpenOptions::new().create(true).append(true).open(&self.path), serde_json::to_string(&rec))
        {
            let _ = writeln!(f, "{line}");
        }
    }

    pub fn path(&self) -> &PathBuf {
        &self.path
    }

    /// Delete transcript file if it contains 0 messages.
    pub fn purge_if_empty(&self) {
        if load_messages(&self.path).is_empty() {
            let _ = fs::remove_file(&self.path);
        }
    }

    /// True when the last message record already carries this exact user turn.
    /// The TUI persists the prompt before the loop runs; the loop re-checks
    /// against the file so a resumed or replayed prompt never double-writes.
    pub fn last_is_user(&self, content: &str) -> bool {
        load_messages(&self.path)
            .last()
            .map_or(false, |m| m.role == "user" && m.content == content)
    }
}

/// One restored transcript entry for UI playback on restart or resume.
#[derive(Debug, Clone)]
pub enum RestoredItem {
    User(String),
    Assistant(String),
    Tool {
        name: String,
        args: String,
        output: String,
    },
    Thinking(String),
}

/// Load full transcript items (messages, thinking, tool output) for UI replay.
pub fn load_transcript_items(path: &PathBuf) -> Vec<RestoredItem> {
    let mut out = Vec::new();
    let Ok(file) = fs::File::open(path) else { return out };
    let mut pending_tool_call: Option<(String, String)> = None;

    for line in BufReader::new(file).lines() {
        let Ok(line) = line else { break };
        let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
        let kind = v.get("kind").and_then(Value::as_str).unwrap_or("");
        match kind {
            "thinking" => {
                if let Some(c) = v.get("content").and_then(Value::as_str) {
                    out.push(RestoredItem::Thinking(c.to_string()));
                }
            }
            "message" => {
                let role = v.get("role").and_then(Value::as_str).unwrap_or("");
                let content = v.get("content").and_then(Value::as_str).unwrap_or("");
                match role {
                    "user" => {
                        pending_tool_call = None;
                        out.push(RestoredItem::User(content.to_string()));
                    }
                    "assistant" => {
                        if let Some(tc) = v.get("tool_call") {
                            let name = tc.get("name").and_then(Value::as_str).unwrap_or("").to_string();
                            let args = tc.get("args").map(|a| a.to_string()).unwrap_or_else(|| "{}".to_string());
                            pending_tool_call = Some((name, args));
                        } else if !content.is_empty() {
                            pending_tool_call = None;
                            out.push(RestoredItem::Assistant(content.to_string()));
                        }
                    }
                    "tool" => {
                        if let Some((name, args)) = pending_tool_call.take() {
                            out.push(RestoredItem::Tool {
                                name,
                                args,
                                output: content.to_string(),
                            });
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
    out
}

/// Read a stored transcript back into messages, skipping the header and any
/// malformed trailing line (a torn write from an interrupted run).
pub fn load_messages(path: &PathBuf) -> Vec<Message> {
    let mut out = Vec::new();
    let Ok(file) = fs::File::open(path) else { return out };
    for line in BufReader::new(file).lines() {
        let Ok(line) = line else { break };
        let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
        if v.get("kind").and_then(Value::as_str) == Some("message") {
            if let (Some(role), Some(content)) = (
                v.get("role").and_then(Value::as_str),
                v.get("content").and_then(Value::as_str),
            ) {
                let mut m = Message::text(role, content);
                if let Some(tc) = v.get("tool_call") {
                    let name = tc.get("name").and_then(Value::as_str).unwrap_or("").to_string();
                    let args = tc.get("args").cloned().unwrap_or(Value::Null);
                    m.tool_call = Some(ToolCall { name, args });
                }
                out.push(m);
            }
        }
    }
    out
}

/// Summary of one session for `/resume`: id, first user prompt, size, mtime.
#[derive(Clone, Debug)]
pub struct SessionInfo {
    pub id: String,
    pub preview: String,
    pub messages: usize,
    /// Number of actual user prompts (turns), distinct from total message entries.
    pub user_turns: usize,
    /// Analyst-set title from the meta record (`/rename`), if any.
    pub title: Option<String>,
    #[allow(dead_code)]
    pub modified: u64,
    /// Workspace directory recorded at creation (meta `cwd`), if known.
    pub cwd: Option<String>,
    /// Basename of `cwd` — the project label shown in the browser header.
    pub project: String,
    /// Git branch recorded at creation (meta `branch`), if any.
    pub branch: Option<String>,
    /// Transcript file size in bytes.
    pub bytes: u64,
    /// Earliest message timestamp (0 when none recorded).
    pub first_ts: u64,
    /// Latest message timestamp (falls back to `modified`).
    pub last_ts: u64,
    /// Wall-clock span of the conversation (last_ts − first_ts, 0 when unknown).
    pub duration_secs: u64,
}

/// Collect every per-message `ts` seen while loading, alongside the messages.
fn load_with_timestamps(path: &PathBuf) -> (Vec<Message>, Vec<u64>) {
    let mut msgs = Vec::new();
    let mut stamps = Vec::new();
    let Ok(file) = fs::File::open(path) else { return (msgs, stamps) };
    for line in BufReader::new(file).lines() {
        let Ok(line) = line else { break };
        let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
        if v.get("kind").and_then(Value::as_str) == Some("message") {
            if let (Some(role), Some(content)) = (
                v.get("role").and_then(Value::as_str),
                v.get("content").and_then(Value::as_str),
            ) {
                let mut m = Message::text(role, content);
                if let Some(tc) = v.get("tool_call") {
                    let name = tc.get("name").and_then(Value::as_str).unwrap_or("").to_string();
                    let args = tc.get("args").cloned().unwrap_or(Value::Null);
                    m.tool_call = Some(ToolCall { name, args });
                }
                msgs.push(m);
                if let Some(t) = v.get("ts").and_then(Value::as_u64) {
                    stamps.push(t);
                }
            }
        }
    }
    (msgs, stamps)
}

/// Basename of a path, used as the short project label.
fn basename(p: &str) -> String {
    p.trim_end_matches('/').rsplit('/').next().filter(|s| !s.is_empty()).unwrap_or(p).to_string()
}

/// List stored sessions, most-recently-modified first, capped at `limit`.
/// Empty sessions (0 messages) are skipped and cleaned up.
pub fn list_sessions(limit: usize) -> Vec<SessionInfo> {
    list_sessions_scoped(limit, None)
}

/// Like [`list_sessions`] but, when `scope_cwd` is `Some`, keeps only sessions
/// whose recorded workspace matches that directory. Sessions with no recorded
/// cwd are excluded from a scoped view (they predate project tagging).
pub fn list_sessions_scoped(limit: usize, scope_cwd: Option<&str>) -> Vec<SessionInfo> {
    let dir = sessions_dir();
    let Ok(entries) = fs::read_dir(&dir) else { return Vec::new() };
    let mut infos: Vec<SessionInfo> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }
        let id = path.file_stem().and_then(|s| s.to_str()).unwrap_or("?").to_string();
        let (msgs, stamps) = load_with_timestamps(&path);
        // If session has no user messages, remove empty artifact and skip
        if msgs.is_empty() {
            let _ = fs::remove_file(&path);
            continue;
        }
        let preview = msgs
            .iter()
            .find(|m| m.role == "user")
            .map(|m| m.content.chars().take(72).collect::<String>())
            .unwrap_or_default();
        let modified = path
            .metadata()
            .and_then(|m| m.modified())
            .map(|t| t.duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0))
            .unwrap_or(0);
        let bytes = path.metadata().map(|m| m.len()).unwrap_or(0);

        let meta = read_meta(&path);
        let cwd = meta
            .as_ref()
            .and_then(|m| m.get("cwd").and_then(Value::as_str).map(|s| s.to_string()));
        let branch = meta
            .as_ref()
            .and_then(|m| m.get("branch").and_then(Value::as_str).map(|s| s.to_string()));
        let project = cwd.as_deref().map(basename).unwrap_or_else(|| "unknown".to_string());

        // Scoped view: keep only same-workspace sessions.
        if let Some(sc) = scope_cwd {
            match &cwd {
                Some(c) if c == sc => {}
                _ => continue,
            }
        }

        let first_ts = stamps.iter().min().copied().unwrap_or(0);
        let last_ts = stamps.iter().max().copied().unwrap_or(modified);
        // Fold the session-start recorded in the meta header so a single-turn
        // conversation still reports its true span (the first message can share
        // the meta's second-granularity timestamp).
        let meta_ts = meta
            .as_ref()
            .and_then(|m| m.get("ts").and_then(Value::as_u64));
        let first_ts = match meta_ts {
            Some(t) if first_ts == 0 => t,
            Some(t) => first_ts.min(t),
            None => first_ts,
        };
        let last_ts = last_ts.max(meta_ts.unwrap_or(0)).max(modified);
        let duration_secs = last_ts.saturating_sub(first_ts);

        let title = session_title(&path).or_else(|| {
            if !preview.is_empty() {
                Some(derive_title(&preview))
            } else {
                None
            }
        });
        let user_turns = msgs.iter().filter(|m| m.role == "user").count();
        infos.push(SessionInfo {
            id,
            preview,
            messages: msgs.len(),
            user_turns,
            title,
            modified,
            cwd,
            project,
            branch,
            bytes,
            first_ts,
            last_ts,
            duration_secs,
        });
    }
    infos.sort_by(|a, b| b.modified.cmp(&a.modified));
    infos.truncate(limit);
    infos
}

/// Read the first `meta` record of a transcript, if any. Shared by title,
/// project and branch lookups so the header is parsed once per session.
pub fn read_meta(path: &PathBuf) -> Option<Value> {
    let file = fs::File::open(path).ok()?;
    for line in BufReader::new(file).lines().take(4) {
        let Ok(line) = line else { break };
        let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
        if v.get("kind").and_then(Value::as_str) == Some("meta") {
            return Some(v);
        }
    }
    None
}

/// Read the `title` field from a transcript's meta header, if present.
pub fn session_title(path: &PathBuf) -> Option<String> {
    read_meta(path)?.get("title").and_then(Value::as_str).map(|s| s.to_string())
}

/// Update or insert a `title` into a transcript's meta header record.
pub fn update_session_title(path: &PathBuf, title: &str) -> Option<()> {
    let content = fs::read_to_string(path).ok()?;
    let mut out = String::with_capacity(content.len() + 64);
    let mut injected = false;
    for line in content.lines() {
        if !injected {
            if let Ok(mut v) = serde_json::from_str::<Value>(line) {
                if v.get("kind").and_then(Value::as_str) == Some("meta") {
                    v["title"] = json!(title);
                    if let Ok(serialized) = serde_json::to_string(&v) {
                        out.push_str(&serialized);
                        out.push('\n');
                        injected = true;
                        continue;
                    }
                }
            }
        }
        out.push_str(line);
        out.push('\n');
        if !injected && line.contains("\"kind\":\"message\"") {
            let meta = json!({ "kind": "meta", "title": title, "ts": now_secs() });
            out = format!("{}\n{}", meta, out);
            injected = true;
        }
    }
    if !injected {
        let meta = json!({ "kind": "meta", "title": title, "ts": now_secs() });
        out = format!("{meta}\n{content}");
    }
    fs::write(path, out).ok()
}

/// Generate a title from a user prompt.
pub fn derive_title(text: &str) -> String {
    let first_line = text.lines().next().unwrap_or("").trim();
    let words: Vec<&str> = first_line.split_whitespace().collect();
    if words.is_empty() {
        return "Untitled Conversation".to_string();
    }
    let take = words.len().min(6);
    let mut out = Vec::new();
    for &w in &words[..take] {
        let clean: String = w.chars().filter(|c| c.is_alphanumeric() || *c == '-' || *c == '_').collect();
        if !clean.is_empty() {
            let mut chars = clean.chars();
            let first = chars.next().unwrap().to_uppercase().collect::<String>();
            let rest = chars.as_str().to_lowercase();
            out.push(format!("{first}{rest}"));
        }
    }
    if out.is_empty() {
        "Untitled Conversation".to_string()
    } else {
        out.join(" ")
    }
}

/// Resolve a resume target by exact id, prefix, suffix, title, or query; returns the message list.
pub fn resolve_session(query: &str) -> Option<(PathBuf, Vec<Message>)> {
    let q = query.trim().trim_end_matches(".jsonl");
    if q.is_empty() {
        return None;
    }
    let dir = sessions_dir();
    let exact = dir.join(format!("{q}.jsonl"));
    if exact.exists() {
        return Some((exact.clone(), load_messages(&exact)));
    }
    // Prefix or suffix match on ID (e.g. `343c481b` from `1793621685-343c481b`)
    let all = list_sessions(usize::MAX);
    let prefix_matches: Vec<PathBuf> = all
        .iter()
        .filter(|s| s.id.starts_with(q))
        .map(|s| dir.join(format!("{}.jsonl", s.id)))
        .collect();
    if prefix_matches.len() == 1 {
        let p = prefix_matches.into_iter().next().unwrap();
        return Some((p.clone(), load_messages(&p)));
    }

    let suffix_or_substr_matches: Vec<PathBuf> = all
        .iter()
        .filter(|s| s.id.ends_with(q) || s.id.contains(q))
        .map(|s| dir.join(format!("{}.jsonl", s.id)))
        .collect();
    if !suffix_or_substr_matches.is_empty() {
        // Return first/most recent match
        let p = suffix_or_substr_matches.into_iter().next().unwrap();
        return Some((p.clone(), load_messages(&p)));
    }

    // Match by session title or user preview text
    let q_lower = q.to_lowercase();
    let title_matches: Vec<PathBuf> = all
        .iter()
        .filter(|s| {
            s.title.as_deref().map(|t| t.to_lowercase().contains(&q_lower)).unwrap_or(false)
                || s.preview.to_lowercase().contains(&q_lower)
        })
        .map(|s| dir.join(format!("{}.jsonl", s.id)))
        .collect();
    if !title_matches.is_empty() {
        let p = title_matches.into_iter().next().unwrap();
        return Some((p.clone(), load_messages(&p)));
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static TEST_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn roundtrip_and_resume() {
        let _guard = TEST_LOCK.lock().unwrap();
        let dir = std::env::temp_dir().join(format!("onyx-sess-test-{}", new_session_id()));
        std::env::set_var("ONYX_HOME", &dir);
        let id = new_session_id();
        {
            let t = Transcript::open(&id, "test-model").expect("open");
            t.push(&Message::text("user", "hello"));
            t.push(&Message::text("assistant", "hi there"));
            t.push(&Message::text("tool", "result"));
        }
        let path = dir.join("sessions").join(format!("{id}.jsonl"));
        let msgs = load_messages(&path);
        assert_eq!(msgs.len(), 3, "header must not count as a message");
        assert_eq!(msgs[0].role, "user");
        assert_eq!(msgs[1].content, "hi there");

        // Exact-id and unique-prefix resolution both work.
        let (_, via_exact) = resolve_session(&id).expect("exact");
        assert_eq!(via_exact.len(), 3);
        let (_, via_prefix) = resolve_session(&id[..8]).expect("prefix");
        assert_eq!(via_prefix.len(), 3);

        let listed = list_sessions(5);
        assert!(listed.iter().any(|s| s.id == id && s.preview == "hello"));

        // Tool calls survive the round trip.
        let tc_id = new_session_id();
        {
            let t = Transcript::open(&tc_id, "test-model").expect("open");
            let mut m = Message::text("assistant", "");
            m.tool_call = Some(ToolCall { name: "bash".into(), args: json!({"cmd": "ls"}) });
            t.push(&m);
        }
        let restored = load_messages(&dir.join("sessions").join(format!("{tc_id}.jsonl")));
        assert_eq!(restored.len(), 1);
        let tc = restored[0].tool_call.as_ref().expect("tool call restored");
        assert_eq!(tc.name, "bash");
        assert_eq!(tc.args["cmd"], "ls");

        let _ = fs::remove_dir_all(&dir);
        std::env::remove_var("ONYX_HOME");
    }

    #[test]
    fn scoped_listing_and_metadata_roundtrip() {
        let _guard = TEST_LOCK.lock().unwrap();
        let dir = std::env::temp_dir().join(format!("onyx-sess-scope-{}", new_session_id()));
        std::env::set_var("ONYX_HOME", &dir);

        // Two sessions in project A, one in project B.
        let a1 = new_session_id();
        {
            let t = Transcript::open_in(&a1, "m", Some("/work/projA")).expect("open");
            t.push(&Message::text("user", "first prompt"));
            t.push(&Message::text("assistant", "reply"));
        }
        let a2 = new_session_id();
        {
            let t = Transcript::open_in(&a2, "m", Some("/work/projA")).expect("open");
            t.push(&Message::text("user", "another"));
        }
        let b1 = new_session_id();
        {
            let t = Transcript::open_in(&b1, "m", Some("/work/projB")).expect("open");
            t.push(&Message::text("user", "elsewhere"));
        }

        let all = list_sessions(usize::MAX);
        assert_eq!(all.len(), 3, "unscoped sees every session");
        for s in &all {
            assert!(!s.project.is_empty());
            assert!(s.bytes > 0, "size recorded");
        }

        let scoped = list_sessions_scoped(usize::MAX, Some("/work/projA"));
        assert_eq!(scoped.len(), 2, "scoped to projA keeps only its two");
        assert!(scoped.iter().all(|s| s.project == "projA"));
        assert!(scoped.iter().any(|s| s.id == a1 && s.messages == 2));

        // Duration is derivable and non-negative; first_ts recorded from message ts.
        let s1 = scoped.iter().find(|s| s.id == a1).unwrap();
        assert!(s1.first_ts > 0);
        assert!(s1.duration_secs < 60, "same-second span stays tiny");

        let _ = fs::remove_dir_all(&dir);
        std::env::remove_var("ONYX_HOME");
    }

    #[test]
    fn torn_last_line_is_skipped() {
        let _guard = TEST_LOCK.lock().unwrap();
        let dir = std::env::temp_dir().join(format!("onyx-sess-torn-{}", new_session_id()));
        std::env::set_var("ONYX_HOME", &dir);
        let id = new_session_id();
        {
            let t = Transcript::open(&id, "m").expect("open");
            t.push(&Message::text("user", "one"));
        }
        let path = dir.join("sessions").join(format!("{id}.jsonl"));
        // Simulate a crash mid-write: append a truncated JSON line.
        if let Ok(mut f) = fs::OpenOptions::new().append(true).open(&path) {
            let _ = f.write_all(b"{\"kind\":\"message\",\"role\":\"user\",\"con");
        }
        let msgs = load_messages(&path);
        assert_eq!(msgs.len(), 1, "the torn trailing line must be ignored");
        let _ = fs::remove_dir_all(&dir);
        std::env::remove_var("ONYX_HOME");
    }
}
