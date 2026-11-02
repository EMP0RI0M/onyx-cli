//! Task and Background Process tools: TaskOutputTool and KillTaskTool.
use crate::{Args, Output, Tool, ToolError};

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
        let task_id = args.get("task_id").and_then(|v| v.as_str()).ok_or_else(|| {
            ToolError::BadArgs("missing 'task_id'".into())
        })?;

        // Checks running processes or task outputs
        let ps = std::process::Command::new("ps")
            .arg("-p")
            .arg(task_id)
            .output();

        match ps {
            Ok(out) if out.status.success() => {
                Ok(Output::ok(format!("Task {task_id} is running.\n{}", String::from_utf8_lossy(&out.stdout))))
            }
            _ => {
                Ok(Output::ok(format!("Task {task_id} completed or not active.")))
            }
        }
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

        let res = std::process::Command::new("kill")
            .arg("-15")
            .arg(task_id)
            .output();

        match res {
            Ok(out) if out.status.success() => {
                Ok(Output::ok(format!("Successfully terminated task {task_id}")))
            }
            Ok(out) => {
                let err = String::from_utf8_lossy(&out.stderr);
                Err(ToolError::Other(format!("Failed to terminate task {task_id}: {err}")))
            }
            Err(e) => Err(ToolError::Io(e)),
        }
    }
}
