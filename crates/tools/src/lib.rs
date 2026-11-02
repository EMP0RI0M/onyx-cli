//! tektile-core tools: the Rust re-architecture of OpenManus's tool set and
//! OpenBot's governance gateway.
//!
//! Design: every tool implements `Tool`. Nothing runs bare — the `Gateway`
//! checks a `Policy` and writes an `AuditLog` row BEFORE/around every call
//! (OpenBot's "no path acts without a record" principle, in Rust types).
//! Tool args are JSON (`serde_json::Value`) to match OpenManus / MCP wire format.

pub mod audit;
pub mod bash;
pub mod control;
pub mod editor;
pub mod implementations;
pub mod planning;
pub mod policy;
pub mod python;
pub mod taxonomy;
pub mod vecmath;
pub mod websearch;

use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;

/// JSON tool-call arguments (MCP/OpenManus wire shape).
pub type Args = Value;

/// A callable capability whose state is shareable across threads. Tools embedded
/// in a `ToolCollection` must be `Send + Sync` so the collection can be cloned
/// into subagent worker threads (`&dyn SharedTool` is `Copy`).
pub trait SharedTool: Tool + Send + Sync {}
impl<T: Tool + Send + Sync> SharedTool for T {}

/// Result returned by a tool.
#[derive(Debug, Clone)]
pub struct Output {
    pub content: String,
    pub is_error: bool,
}

impl Output {
    pub fn ok(content: impl Into<String>) -> Self {
        Self { content: content.into(), is_error: false }
    }
    pub fn err(content: impl Into<String>) -> Self {
        Self { content: content.into(), is_error: true }
    }
}

#[derive(Debug)]
pub enum ToolError {
    BadArgs(String),
    Io(std::io::Error),
    Denied(String),
    Other(String),
}

impl std::fmt::Display for ToolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ToolError::BadArgs(s) => write!(f, "bad args: {s}"),
            ToolError::Io(e) => write!(f, "io: {e}"),
            ToolError::Denied(s) => write!(f, "denied: {s}"),
            ToolError::Other(s) => write!(f, "{s}"),
        }
    }
}

/// A callable capability. Mirrors OpenManus `BaseTool` and the MCP tool shape.
pub trait Tool {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn execute(&self, args: &Args) -> Result<Output, ToolError>;

    /// JSON-schema for this tool's arguments (OpenAI function-calling shape).
    /// Default is a permissive object so existing tools compile unchanged;
    /// tools with known argument names should override it.
    fn schema(&self) -> Value {
        let mut props = serde_json::Map::new();
        for key in self.arg_keys() {
            props.insert(key.clone(), json!({"type": "string"}));
        }
        json!({
            "type": "object",
            "properties": props,
            "additionalProperties": true,
        })
    }

    /// Argument names to surface in the default schema. Empty by default.
    fn arg_keys(&self) -> Vec<String> {
        Vec::new()
    }
}

/// OpenAI-style `tools` array built from the collection: each entry is
/// `{"type":"function","function":{"name","description","parameters"}}`.
pub fn tool_schemas(collection: &ToolCollection) -> Vec<Value> {
    collection
        .names()
        .iter()
        .filter_map(|name| collection.get(name))
        .map(|t| {
            json!({
                "type": "function",
                "function": {
                    "name": t.name(),
                    "description": t.description(),
                    "parameters": t.schema(),
                }
            })
        })
        .collect()
}

/// Name-indexed set of tools (OpenManus `ToolCollection`). Cloning is cheap:
/// entries are `Arc`-shared, so a clone sees the same tool state as the parent.
#[derive(Default, Clone)]
pub struct ToolCollection {
    tools: HashMap<String, Arc<dyn SharedTool>>,
}

impl ToolCollection {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn add(&mut self, tool: impl SharedTool + 'static) -> &mut Self {
        self.tools.insert(tool.name().to_string(), Arc::new(tool));
        self
    }
    pub fn get(&self, name: &str) -> Option<&dyn SharedTool> {
        self.tools.get(name).map(|a| a.as_ref())
    }
    pub fn names(&self) -> Vec<String> {
        let mut v: Vec<String> = self.tools.keys().cloned().collect();
        v.sort();
        v
    }
    /// Keep only the named tools; others are removed. Used to scope subagents.
    pub fn retain(&mut self, keep: &[&str]) -> &mut Self {
        self.tools.retain(|name, _| keep.contains(&name.as_str()));
        self
    }
}

/// The OpenManus-equivalent default toolset: bash, python, editor, planning,
/// web_search, vecmath (asm-kernel backed), terminate, ask_human.
pub fn default_collection(editor_base: std::path::PathBuf) -> ToolCollection {
    let mut c = ToolCollection::new();
    c.add(bash::Bash::new());
    c.add(python::PythonExecute::default());
    c.add(editor::StrReplaceEditor::new(editor_base.clone()));
    c.add(planning::Planning::default());
    c.add(websearch::WebSearch);
    c.add(vecmath::VecMath);
    c.add(control::Terminate);
    c.add(control::AskHuman);

    // Grok-build tools: read_file, list_dir, grep, search_replace, todo, plan mode, web_fetch, update_goal, task_output, kill_task, ask_user_question
    c.add(implementations::grok_build::ReadFile::new(editor_base.clone()));
    c.add(implementations::grok_build::ListDir::new(editor_base.clone()));
    c.add(implementations::grok_build::Grep::new(editor_base.clone()));
    c.add(implementations::grok_build::SearchReplace::new(editor_base));
    c.add(implementations::grok_build::TodoTool::new());
    let plan_state = Arc::new(std::sync::Mutex::new(implementations::grok_build::PlanState::default()));
    c.add(implementations::grok_build::EnterPlanMode::new(plan_state.clone()));
    c.add(implementations::grok_build::ExitPlanMode::new(plan_state));
    c.add(implementations::grok_build::WebFetch);
    c.add(implementations::grok_build::UpdateGoalTool::default());
    c.add(implementations::grok_build::TaskOutputTool);
    c.add(implementations::grok_build::KillTaskTool);
    c.add(implementations::grok_build::AskUserQuestion);

    c
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    struct Echo;
    impl Tool for Echo {
        fn name(&self) -> &str { "echo" }
        fn description(&self) -> &str { "echo back" }
        fn execute(&self, a: &Args) -> Result<Output, ToolError> {
            Ok(Output::ok(a.get("msg").and_then(|v| v.as_str()).unwrap_or("").to_string()))
        }
    }

    #[test]
    fn collection_dispatch() {
        let mut c = ToolCollection::new();
        c.add(Echo);
        assert_eq!(c.names(), vec!["echo".to_string()]);
        let out = c.get("echo").unwrap().execute(&json!({"msg": "hi"})).unwrap();
        assert_eq!(out.content, "hi");
    }

    #[test]
    fn tool_schemas_wrap_in_function_shape() {
        let c = default_collection(std::env::temp_dir());
        let defs = tool_schemas(&c);
        assert_eq!(defs.len(), c.names().len());
        for d in &defs {
            assert_eq!(d["type"], "function");
            assert!(d["function"]["name"].is_string());
            assert!(d["function"]["description"].is_string());
            assert_eq!(d["function"]["parameters"]["type"], "object");
        }
        // bash declares its arg surface
        let bash = defs.iter().find(|d| d["function"]["name"] == "bash").unwrap();
        assert!(bash["function"]["parameters"]["properties"]["command"].is_object());
    }

    #[test]
    fn retain_scopes_the_collection() {
        let mut c = default_collection(std::env::temp_dir());
        c.retain(&["bash", "terminate"]);
        assert_eq!(c.names(), vec!["bash".to_string(), "terminate".to_string()]);
    }

    #[test]
    fn clone_shares_tool_state_across_threads() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        struct Counter(Arc<AtomicUsize>);
        impl Tool for Counter {
            fn name(&self) -> &str { "counter" }
            fn description(&self) -> &str { "counts" }
            fn execute(&self, _a: &Args) -> Result<Output, ToolError> {
                Ok(Output::ok(self.0.fetch_add(1, Ordering::SeqCst).to_string()))
            }
        }

        let mut c = ToolCollection::new();
        let n = Arc::new(AtomicUsize::new(0));
        c.add(Counter(Arc::clone(&n)));

        // A clone moved to another thread sees (and mutates) the same state.
        let child = c.clone();
        let handle = std::thread::spawn(move || child.get("counter").unwrap().execute(&json!({})).unwrap());
        assert_eq!(handle.join().unwrap().content, "0");
        assert_eq!(n.load(Ordering::SeqCst), 1);
        assert_eq!(c.get("counter").unwrap().execute(&json!({})).unwrap().content, "1");
    }
}
