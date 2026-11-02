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
    path: PathBuf,
}

impl Transcript {
    /// Create (or reopen) the transcript for `id`, writing a header record the
    /// first time. Returns `None` if the directory could not be created — a
    /// failure to persist must never abort an interactive session.
    pub fn open(id: &str, model: &str) -> Option<Self> {
        let dir = sessions_dir();
        if fs::create_dir_all(&dir).is_err() {
            return None;
        }
        let path = dir.join(format!("{id}.jsonl"));
        let fresh = !path.exists();
        let mut f = fs::OpenOptions::new().create(true).append(true).open(&path).ok()?;
        if fresh {
            let rec = json!({ "kind": "meta", "model": model, "ts": now_secs() });
            if let Ok(line) = serde_json::to_string(&rec) {
                let _ = writeln!(f, "{line}");
            }
        }
        Some(Self { path })
    }

    /// Append one message as its own record. Best-effort: write errors are
    /// swallowed so a full disk degrades to "no persistence", not a crash.
    pub fn push(&self, msg: &Message) {
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
            (fs::OpenOptions::new().append(true).open(&self.path), serde_json::to_string(&rec))
        {
            let _ = writeln!(f, "{line}");
        }
    }

    pub fn path(&self) -> &PathBuf {
        &self.path
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
pub struct SessionInfo {
    pub id: String,
    pub preview: String,
    pub messages: usize,
    #[allow(dead_code)]
    pub modified: u64,
}

/// List stored sessions, most-recently-modified first, capped at `limit`.
pub fn list_sessions(limit: usize) -> Vec<SessionInfo> {
    let dir = sessions_dir();
    let Ok(entries) = fs::read_dir(&dir) else { return Vec::new() };
    let mut infos: Vec<SessionInfo> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }
        let id = path.file_stem().and_then(|s| s.to_str()).unwrap_or("?").to_string();
        let msgs = load_messages(&path);
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
        infos.push(SessionInfo { id, preview, messages: msgs.len(), modified });
    }
    infos.sort_by(|a, b| b.modified.cmp(&a.modified));
    infos.truncate(limit);
    infos
}

/// Resolve a resume target by exact id or unique prefix; returns the message list.
pub fn resolve_session(query: &str) -> Option<(PathBuf, Vec<Message>)> {
    let dir = sessions_dir();
    let exact = dir.join(format!("{query}.jsonl"));
    if exact.exists() {
        return Some((exact.clone(), load_messages(&exact)));
    }
    // Unique prefix match among stored ids.
    let matches: Vec<PathBuf> = list_sessions(usize::MAX)
        .into_iter()
        .filter(|s| s.id.starts_with(query))
        .map(|s| dir.join(format!("{}.jsonl", s.id)))
        .collect();
    if matches.len() == 1 {
        let p = matches.into_iter().next().unwrap();
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
