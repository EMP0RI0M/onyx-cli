//! Durable thread storage: JSONL transcripts with start / resume / fork.
//!
//! The file format is a superset of the engine's existing session records
//! (`{"kind":"meta",…}` header + one record per message), so `agent`'s current
//! transcripts load unchanged while new threads written here carry the extra
//! fields (id, ts per record) that clients need for replay and diffing.

use serde_json::{json, Value};
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

/// A conversational message as persisted and exchanged across the boundary.
#[derive(Debug, Clone, PartialEq)]
pub struct ChatMessage {
    pub role: String, // "system" | "user" | "assistant" | "tool"
    pub content: String,
    pub tool_call: Option<(String, Value)>, // (name, args)
}

impl ChatMessage {
    pub fn text(role: &str, content: impl Into<String>) -> Self {
        Self { role: role.into(), content: content.into(), tool_call: None }
    }
}

/// Header record of one thread file.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionMeta {
    pub id: String,
    pub model: String,
    pub created_ts: u64,
    /// Thread this one was branched from, if a fork.
    pub parent_id: Option<String>,
}

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Fresh thread id: epoch seconds plus a nanosecond component, matching the
/// ids the engine already generates so mixed-vintage directories sort alike.
pub fn new_thread_id() -> String {
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(0);
    format!("{}-{:x}", now_secs(), nanos)
}

/// Root directory holding all thread files, honouring `ONYX_HOME`.
pub fn threads_dir() -> PathBuf {
    let base = match std::env::var("ONYX_HOME") {
        Ok(h) if !h.is_empty() => PathBuf::from(h),
        _ => match std::env::var("HOME").ok().filter(|h| !h.is_empty()) {
            Some(h) => PathBuf::from(h).join(".onyx"),
            None => PathBuf::from(".onyx"),
        },
    };
    base.join("sessions")
}

/// One open thread: metadata plus append-only handle to its file.
pub struct ThreadStore {
    meta: SessionMeta,
    path: PathBuf,
}

impl ThreadStore {
    /// Create a brand-new thread, writing its header. Returns `None` when the
    /// directory or file cannot be created — persistence failures must degrade
    /// to "no durability", never abort a live session.
    pub fn create(model: &str, parent: Option<&str>) -> Option<Self> {
        let dir = threads_dir();
        fs::create_dir_all(&dir).ok()?;
        let id = new_thread_id();
        let path = dir.join(format!("{id}.jsonl"));
        let meta = SessionMeta {
            id,
            model: model.to_string(),
            created_ts: now_secs(),
            parent_id: parent.map(str::to_string),
        };
        let mut f = fs::OpenOptions::new().create_new(true).append(true).open(&path).ok()?;
        let rec = json!({
            "kind": "meta",
            "id": meta.id,
            "model": meta.model,
            "ts": meta.created_ts,
            "parent": meta.parent_id,
        });
        writeln!(f, "{}", rec).ok()?;
        Some(Self { meta, path })
    }

    /// Reopen an existing thread by id, reading back its header.
    pub fn resume(id: &str) -> Option<Self> {
        let path = threads_dir().join(format!("{id}.jsonl"));
        if !path.exists() {
            return None;
        }
        let meta = read_meta(&path)?;
        Some(Self { meta, path })
    }

    /// Fork: create a new thread whose file starts as a copy of this one's
    /// messages under a fresh header carrying `parent = self.id`. The original
    /// stays sealed — exactly the `/resume` semantics the TUI ships today.
    pub fn fork(&self) -> Option<Self> {
        let msgs = self.load_messages();
        let child = ThreadStore::create(&self.meta.model, Some(&self.meta.id))?;
        for m in &msgs {
            child.append(m);
        }
        Some(child)
    }

    /// Append one message record. Best-effort like `create`'s failure policy.
    pub fn append(&self, msg: &ChatMessage) {
        let mut rec = json!({
            "kind": "message",
            "role": msg.role,
            "content": msg.content,
            "ts": now_secs(),
        });
        if let Some((name, args)) = &msg.tool_call {
            rec["tool_call"] = json!({ "name": name, "args": args });
        }
        if let (Ok(mut f), Ok(line)) =
            (fs::OpenOptions::new().append(true).open(&self.path), serde_json::to_string(&rec))
        {
            let _ = writeln!(f, "{line}");
        }
    }

    /// All messages, skipping the header and any torn trailing line.
    pub fn load_messages(&self) -> Vec<ChatMessage> {
        load_messages_at(&self.path)
    }

    pub fn meta(&self) -> &SessionMeta {
        &self.meta
    }

    pub fn path(&self) -> &PathBuf {
        &self.path
    }
}

fn read_meta(path: &PathBuf) -> Option<SessionMeta> {
    let file = fs::File::open(path).ok()?;
    for line in BufReader::new(file).lines() {
        let Ok(line) = line else { break };
        let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
        if v.get("kind").and_then(Value::as_str) == Some("meta") {
            return Some(SessionMeta {
                id: path.file_stem()?.to_string_lossy().into_owned(),
                model: v.get("model").and_then(Value::as_str).unwrap_or("").to_string(),
                created_ts: v.get("ts").and_then(Value::as_u64).unwrap_or(0),
                parent_id: v.get("parent").and_then(Value::as_str).map(str::to_string),
            });
        }
    }
    None
}

fn load_messages_at(path: &PathBuf) -> Vec<ChatMessage> {
    let mut out = Vec::new();
    let Ok(file) = fs::File::open(path) else { return out };
    for line in BufReader::new(file).lines() {
        let Ok(line) = line else { break };
        let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
        if v.get("kind").and_then(Value::as_str) != Some("message") {
            continue;
        }
        let (Some(role), Some(content)) = (
            v.get("role").and_then(Value::as_str),
            v.get("content").and_then(Value::as_str),
        ) else {
            continue;
        };
        let mut m = ChatMessage::text(role, content);
        if let Some(tc) = v.get("tool_call") {
            let name = tc.get("name").and_then(Value::as_str).unwrap_or("").to_string();
            let args = tc.get("args").cloned().unwrap_or(Value::Null);
            m.tool_call = Some((name, args));
        }
        out.push(m);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    struct DirGuard(PathBuf);
    impl Drop for DirGuard {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
            std::env::remove_var("ONYX_HOME");
        }
    }

    fn temp_home(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("onyx-core-test-{tag}"));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(p.join("sessions")).unwrap();
        p
    }

    // Thread-store tests mutate the process-global ONYX_HOME, so they must run
    // one at a time; otherwise parallel tests race on whose home dir wins.
    fn lock_home() -> std::sync::MutexGuard<'static, ()> {
        use std::sync::{Mutex, OnceLock};
        static HOME_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        HOME_LOCK.get_or_init(|| Mutex::new(())).lock().unwrap_or_else(|e| e.into_inner())
    }

    #[test]
    fn create_append_load_roundtrip() {
        let _serial = lock_home();
        let home = temp_home("roundtrip");
        std::env::set_var("ONYX_HOME", &home);
        let _g = DirGuard(home.clone());
        let t = ThreadStore::create("test-model", None).expect("create");
        assert_eq!(t.meta().model, "test-model");
        assert!(t.meta().parent_id.is_none());
        t.append(&ChatMessage::text("user", "hello"));
        t.append(&ChatMessage {
            role: "assistant".into(),
            content: "".into(),
            tool_call: Some(("bash".into(), json!({"command": "ls"}))),
        });
        let msgs = t.load_messages();
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].content, "hello");
        assert_eq!(msgs[1].tool_call.as_ref().unwrap().0, "bash");
        // Reopen by id sees the same data.
        let reopened = ThreadStore::resume(&t.meta().id).expect("resume");
        assert_eq!(reopened.load_messages(), msgs);
    }

    #[test]
    fn fork_copies_messages_and_links_parent() {
        let _serial = lock_home();
        let home = temp_home("fork");
        std::env::set_var("ONYX_HOME", &home);
        let _g = DirGuard(home.clone());
        let parent = ThreadStore::create("m", None).unwrap();
        parent.append(&ChatMessage::text("user", "one"));
        let child = parent.fork().expect("fork");
        assert_eq!(child.load_messages(), parent.load_messages());
        assert_eq!(child.meta().parent_id.as_deref(), Some(parent.meta().id.as_str()));
        // Parent stays sealed: appending to the child does not touch it.
        child.append(&ChatMessage::text("user", "two"));
        assert_eq!(parent.load_messages().len(), 1);
        assert_eq!(child.load_messages().len(), 2);
    }

    #[test]
    fn legacy_agent_transcripts_load_unchanged() {
        // Exactly the record shape crates/agent/src/session.rs writes today:
        // meta without id/parent, messages without extra fields.
        let _serial = lock_home();
        let home = temp_home("legacy");
        std::env::set_var("ONYX_HOME", &home);
        let _g = DirGuard(home.clone());
        let legacy = home.join("sessions").join("1700000000-abc.jsonl");
        fs::write(
            &legacy,
            "{\"kind\":\"meta\",\"model\":\"old\",\"ts\":1}\n\
             {\"kind\":\"message\",\"role\":\"user\",\"content\":\"hi\",\"ts\":2}\n\
             {\"kind\":\"message\",\"role\":\"assistant\",\"content\":\"yo\",\"ts\":3,\"tool_call\":{\"name\":\"bash\",\"args\":{}}}\n",
        )
        .unwrap();
        let t = ThreadStore::resume("1700000000-abc").expect("resume legacy");
        assert_eq!(t.meta().model, "old");
        let msgs = t.load_messages();
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[1].tool_call.as_ref().unwrap().0, "bash");
    }

    #[test]
    fn torn_trailing_line_is_skipped() {
        let _serial = lock_home();
        let home = temp_home("torn");
        std::env::set_var("ONYX_HOME", &home);
        let _g = DirGuard(home.clone());
        let t = ThreadStore::create("m", None).unwrap();
        t.append(&ChatMessage::text("user", "good"));
        let mut f = fs::OpenOptions::new().append(true).open(t.path()).unwrap();
        write!(f, "{{\"kind\":\"message\",\"role\":\"user\",\"con").unwrap(); // no newline
        drop(f);
        assert_eq!(t.load_messages().len(), 1);
    }
}
