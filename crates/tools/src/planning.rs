//! Planning tool — OpenManus `PlanningTool`, ported to Rust.
//! Holds the agent's current task plan and renders it as a numbered checklist.
//! `{"todos":[{"content":"...","state":"not_started"|"in_progress"|"done"}]}` sets
//! the plan; `{"need_summary":true}` returns it; `{"delete":true}` clears it.

use crate::{Args, Output, Tool, ToolError};
use std::sync::Mutex;

const STATES: [&str; 3] = ["not_started", "in_progress", "done"];

fn glyph(state: &str) -> &'static str {
    match state {
        "done" => "[x]",
        "in_progress" => "[~]",
        _ => "[ ]",
    }
}

pub struct Planning {
    plan: Mutex<Vec<(String, String)>>,
}

impl Default for Planning {
    fn default() -> Self {
        Self { plan: Mutex::new(Vec::new()) }
    }
}

impl Planning {
    fn render(&self) -> String {
        let p = self.plan.lock().unwrap();
        if p.is_empty() {
            return "No plan set.".to_string();
        }
        let total = p.len();
        let done = p.iter().filter(|(_, s)| s == "done").count();
        let mut out = format!("Plan ({done}/{total} done):\n");
        for (i, (content, state)) in p.iter().enumerate() {
            out.push_str(&format!("{}. {} {} ({state})\n", i + 1, glyph(state), content));
        }
        out
    }
}

impl Tool for Planning {
    fn name(&self) -> &str {
        "planning"
    }
    fn arg_keys(&self) -> Vec<String> {
        vec!["todos".into(), "delete".into()]
    }
    fn description(&self) -> &str {
        "Create, update, or summarize the task plan as a numbered checklist."
    }
    fn execute(&self, args: &Args) -> Result<Output, ToolError> {
        if args.get("delete").and_then(|v| v.as_bool()).unwrap_or(false) {
            self.plan.lock().unwrap().clear();
            return Ok(Output::ok("Plan cleared."));
        }

        if let Some(todos) = args.get("todos").and_then(|v| v.as_array()) {
            let mut p = self.plan.lock().unwrap();
            p.clear();
            for t in todos {
                let content = t.get("content").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let mut state = t
                    .get("state")
                    .and_then(|v| v.as_str())
                    .unwrap_or("not_started")
                    .to_string();
                if !STATES.contains(&state.as_str()) {
                    state = "not_started".into();
                }
                p.push((content, state));
            }
        }

        Ok(Output::ok(self.render()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn planning_sets_and_counts_done() {
        let pl = Planning::default();
        let out = pl
            .execute(&json!({"todos":[
                {"content":"a","state":"done"},
                {"content":"b","state":"in_progress"}
            ]}))
            .unwrap();
        assert!(out.content.contains("(1/2 done)"));
        assert!(out.content.contains("[x]"));
        assert!(out.content.contains("[~]"));
    }
}
