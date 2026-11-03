//! Task and Background Process tools: TaskOutputTool, KillTaskTool, and WaitTasksTool.
pub mod terminal_command;
pub mod wait_tasks;

use crate::{Args, Output, Tool, ToolError};
use terminal_command::GLOBAL_PROCESS_MANAGER;
pub use wait_tasks::WaitTasksTool;

pub struct TaskOutputTool;

impl Tool for TaskOutputTool {
    fn name(&self) -> &str {
        "get_task_output"
    }

    fn description(&self) -> &str {
        "Retrieve output and execution status for background tasks or processes. Args: task_id (string), timeout_ms (optional int)."
    }

    fn arg_keys(&self) -> Vec<String> {
        vec!["task_id".into(), "timeout_ms".into()]
    }

    fn execute(&self, args: &Args) -> Result<Output, ToolError> {
        let task_id = args.get("task_id")
            .or_else(|| args.get("task_ids").and_then(|v| v.as_array()).and_then(|a| a.first()))
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::BadArgs("missing 'task_id'".into()))?;

        let timeout_ms = args.get("timeout_ms").and_then(|v| v.as_u64());

        if let Some(ms) = timeout_ms {
            if ms > 0 {
                let timeout = std::time::Duration::from_millis(ms.min(300_000));
                let _ = GLOBAL_PROCESS_MANAGER.wait_all(&[task_id.to_string()], timeout);
            }
        }

        let (is_running, content, truncated) = GLOBAL_PROCESS_MANAGER.get_output(task_id, 32_768);
        let status = if is_running { "running" } else { "completed/stopped" };

        let mut msg = format!("Task {task_id} status: {status}\n\n{content}");
        if truncated {
            msg.push_str("\n\n[Output truncated to 32KB]");
        }
        Ok(Output::ok(msg))
    }
}

pub struct KillTaskTool;

impl Tool for KillTaskTool {
    fn name(&self) -> &str {
        "kill_task"
    }

    fn description(&self) -> &str {
        "Terminates a running background task or process by task_id. Args: task_id (string)."
    }

    fn arg_keys(&self) -> Vec<String> {
        vec!["task_id".into()]
    }

    fn execute(&self, args: &Args) -> Result<Output, ToolError> {
        let task_id = args.get("task_id").and_then(|v| v.as_str()).ok_or_else(|| {
            ToolError::BadArgs("missing 'task_id'".into())
        })?;

        match GLOBAL_PROCESS_MANAGER.kill(task_id) {
            Ok(msg) => Ok(Output::ok(msg)),
            Err(e) => Err(ToolError::Other(e)),
        }
    }
}
