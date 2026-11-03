//! Hermes Memory Architecture in Rust.
//!
//! Provides bounded, curated persistent memory for the AI agent:
//! - `MEMORY.md`: Agent's personal notes on environment, conventions, lessons learned (capped ~2,200 chars).
//! - `USER.md`: User profile capturing communication style, explicit preferences (capped ~1,375 chars).
//! - Tool `memory`: allows the model to view, update, append, or clear memory entries with capacity safety.

use crate::{Args, Output, Tool, ToolError};
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

pub const DEFAULT_MEMORY_LIMIT: usize = 2500;
pub const DEFAULT_USER_LIMIT: usize = 1500;

#[derive(Debug, Clone)]
pub struct HermesMemoryStore {
    pub base_dir: PathBuf,
    pub memory_limit: usize,
    pub user_limit: usize,
}

impl HermesMemoryStore {
    pub fn default_paths() -> Self {
        let base_dir = if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
            let hermes_dir = home.join(".hermes/memories");
            if hermes_dir.exists() {
                hermes_dir
            } else {
                home.join(".onyx/memories")
            }
        } else {
            PathBuf::from(".hermes/memories")
        };
        let _ = fs::create_dir_all(&base_dir);
        Self {
            base_dir,
            memory_limit: DEFAULT_MEMORY_LIMIT,
            user_limit: DEFAULT_USER_LIMIT,
        }
    }

    pub fn memory_path(&self) -> PathBuf {
        self.base_dir.join("MEMORY.md")
    }

    pub fn user_path(&self) -> PathBuf {
        self.base_dir.join("USER.md")
    }

    pub fn read_memory(&self) -> String {
        fs::read_to_string(self.memory_path()).unwrap_or_default()
    }

    pub fn read_user(&self) -> String {
        fs::read_to_string(self.user_path()).unwrap_or_default()
    }

    pub fn write_memory(&self, content: &str) -> Result<String, String> {
        let chars = content.chars().count();
        if chars > self.memory_limit {
            return Err(format!(
                "MEMORY.md exceeds character budget ({chars}/{} chars). Please consolidate or prune entries.",
                self.memory_limit
            ));
        }
        let _ = fs::create_dir_all(&self.base_dir);
        fs::write(self.memory_path(), content).map_err(|e| e.to_string())?;
        let pct = (chars * 100) / self.memory_limit;
        Ok(format!("MEMORY.md updated ({chars}/{} chars, {pct}% capacity)", self.memory_limit))
    }

    pub fn write_user(&self, content: &str) -> Result<String, String> {
        let chars = content.chars().count();
        if chars > self.user_limit {
            return Err(format!(
                "USER.md exceeds character budget ({chars}/{} chars). Please consolidate or prune entries.",
                self.user_limit
            ));
        }
        let _ = fs::create_dir_all(&self.base_dir);
        fs::write(self.user_path(), content).map_err(|e| e.to_string())?;
        let pct = (chars * 100) / self.user_limit;
        Ok(format!("USER.md updated ({chars}/{} chars, {pct}% capacity)", self.user_limit))
    }

    pub fn append_memory(&self, note: &str) -> Result<String, String> {
        let mut cur = self.read_memory();
        if !cur.is_empty() && !cur.ends_with('\n') {
            cur.push('\n');
        }
        cur.push_str(note);
        if !cur.ends_with('\n') {
            cur.push('\n');
        }
        self.write_memory(&cur)
    }

    pub fn append_user(&self, note: &str) -> Result<String, String> {
        let mut cur = self.read_user();
        if !cur.is_empty() && !cur.ends_with('\n') {
            cur.push('\n');
        }
        cur.push_str(note);
        if !cur.ends_with('\n') {
            cur.push('\n');
        }
        self.write_user(&cur)
    }

    /// Frozen snapshot for injection into system prompt
    pub fn compile_snapshot(&self) -> String {
        let mut out = String::new();
        let mem = self.read_memory();
        let user = self.read_user();

        if !mem.trim().is_empty() || !user.trim().is_empty() {
            out.push_str("\n\n=== HERMES PERSISTENT MEMORY ===");
            if !mem.trim().is_empty() {
                out.push_str("\n[SYSTEM & ENVIRONMENT MEMORY (MEMORY.md)]:\n");
                out.push_str(mem.trim());
            }
            if !user.trim().is_empty() {
                out.push_str("\n\n[USER PROFILE & PREFERENCES (USER.md)]:\n");
                out.push_str(user.trim());
            }
            out.push_str("\n===============================\n");
        }
        out
    }
}

/// Hermes Memory Tool for the agent
#[derive(Clone)]
pub struct HermesMemoryTool {
    store: Arc<Mutex<HermesMemoryStore>>,
}

impl HermesMemoryTool {
    pub fn new(store: Arc<Mutex<HermesMemoryStore>>) -> Self {
        Self { store }
    }
}

impl Tool for HermesMemoryTool {
    fn name(&self) -> &str {
        "memory"
    }

    fn description(&self) -> &str {
        "Inspect or update Hermes persistent agent memory (MEMORY.md for environment facts/conventions, USER.md for user preferences). Args: action ('view'|'append'|'replace'|'status'), target ('memory'|'user'), content (optional string)."
    }

    fn arg_keys(&self) -> Vec<String> {
        vec!["action".into(), "target".into(), "content".into()]
    }

    fn execute(&self, args: &Args) -> Result<Output, ToolError> {
        let action = args.get("action").and_then(|v| v.as_str()).unwrap_or("view");
        let target = args.get("target").and_then(|v| v.as_str()).unwrap_or("memory");
        let content = args.get("content").and_then(|v| v.as_str()).unwrap_or("");
        let store = self.store.lock().unwrap();

        match action {
            "status" => {
                let m = store.read_memory();
                let u = store.read_user();
                let m_cnt = m.chars().count();
                let u_cnt = u.chars().count();
                Ok(Output::ok(format!(
                    "Hermes Memory Status:\n  Directory: {}\n  MEMORY.md: {m_cnt}/{} chars ({}%)\n  USER.md:   {u_cnt}/{} chars ({}%)",
                    store.base_dir.display(),
                    store.memory_limit,
                    (m_cnt * 100) / store.memory_limit,
                    store.user_limit,
                    (u_cnt * 100) / store.user_limit,
                )))
            }
            "view" => {
                if target == "user" {
                    let u = store.read_user();
                    if u.trim().is_empty() {
                        Ok(Output::ok("(USER.md is currently empty)"))
                    } else {
                        Ok(Output::ok(format!("USER.md:\n{u}")))
                    }
                } else {
                    let m = store.read_memory();
                    if m.trim().is_empty() {
                        Ok(Output::ok("(MEMORY.md is currently empty)"))
                    } else {
                        Ok(Output::ok(format!("MEMORY.md:\n{m}")))
                    }
                }
            }
            "append" => {
                if content.trim().is_empty() {
                    return Err(ToolError::BadArgs("missing 'content' to append".into()));
                }
                if target == "user" {
                    match store.append_user(content) {
                        Ok(msg) => Ok(Output::ok(msg)),
                        Err(e) => Ok(Output::err(e)),
                    }
                } else {
                    match store.append_memory(content) {
                        Ok(msg) => Ok(Output::ok(msg)),
                        Err(e) => Ok(Output::err(e)),
                    }
                }
            }
            "replace" => {
                if target == "user" {
                    match store.write_user(content) {
                        Ok(msg) => Ok(Output::ok(msg)),
                        Err(e) => Ok(Output::err(e)),
                    }
                } else {
                    match store.write_memory(content) {
                        Ok(msg) => Ok(Output::ok(msg)),
                        Err(e) => Ok(Output::err(e)),
                    }
                }
            }
            other => Err(ToolError::BadArgs(format!("unknown action '{other}'; use view, append, replace, or status"))),
        }
    }
}
