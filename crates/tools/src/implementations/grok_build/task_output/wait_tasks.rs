//! `wait_tasks` tool — awaits completion of multiple background tasks.
use super::terminal_command::GLOBAL_PROCESS_MANAGER;
use crate::{Args, Output, Tool, ToolError};
use std::time::Duration;

#[derive(Default)]
pub struct WaitTasksTool;

impl Tool for WaitTasksTool {
    fn name(&self) -> &str {
        "wait_tasks"
    }

    fn description(&self) -> &str {
        "Blocks until one or more background tasks/processes complete or timeout expires. Args: task_ids (array of strings or single string), timeout_ms (optional int, default 30000)."
    }

    fn arg_keys(&self) -> Vec<String> {
        vec!["task_ids".into(), "timeout_ms".into()]
    }

    fn execute(&self, args: &Args) -> Result<Output, ToolError> {
        let task_ids: Vec<String> = if let Some(arr) = args.get("task_ids").and_then(|v| v.as_array()) {
            arr.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect()
        } else if let Some(single) = args.get("task_ids").or_else(|| args.get("task_id")).and_then(|v| v.as_str()) {
            vec![single.to_string()]
        } else {
            return Err(ToolError::BadArgs("missing 'task_ids'".into()));
        };

        if task_ids.is_empty() {
            return Err(ToolError::BadArgs("'task_ids' cannot be empty".into()));
        }

        let timeout_ms = args.get("timeout_ms").and_then(|v| v.as_u64()).unwrap_or(30_000).min(300_000);
        let timeout = Duration::from_millis(timeout_ms);

        let results = GLOBAL_PROCESS_MANAGER.wait_all(&task_ids, timeout);

        let mut out = format!("Waited on {} task(s) (timeout: {}ms):\n", task_ids.len(), timeout_ms);
        let mut completed_count = 0;
        for (id, (running, text)) in results {
            if !running {
                completed_count += 1;
                out.push_str(&format!("\n[Task {id} - COMPLETED]\n{text}\n"));
            } else {
                out.push_str(&format!("\n[Task {id} - STILL RUNNING]\n{text}\n"));
            }
        }
        out.push_str(&format!("\nSummary: {completed_count}/{} tasks finished.", task_ids.len()));

        Ok(Output::ok(out))
    }
}
