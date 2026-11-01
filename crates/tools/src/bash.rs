//! Bash tool — runs a shell command, returns stdout+stderr+exit code.
//! OpenManus `Bash`, ported to Rust. Governance (what may run) lives in the
//! Policy, not here — this tool just executes.

use crate::{Args, Output, Tool, ToolError};
use std::process::Command;

pub struct Bash {
    /// Working directory the command runs in.
    pub cwd: Option<String>,
}

impl Bash {
    pub fn new() -> Self {
        Self { cwd: None }
    }
}

impl Default for Bash {
    fn default() -> Self {
        Self::new()
    }
}

impl Tool for Bash {
    fn name(&self) -> &str {
        "bash"
    }
    fn arg_keys(&self) -> Vec<String> {
        vec!["command".into()]
    }
    fn description(&self) -> &str {
        "Run a shell command via `sh -c`. Args: {\"command\": \"<the shell command>\"}. Returns combined stdout/stderr and the exit code."
    }
    fn execute(&self, args: &Args) -> Result<Output, ToolError> {
        let cmd = ["command", "cmd", "script", "shell"]
            .iter()
            .find_map(|k| args.get(*k).and_then(|v| v.as_str()))
            .ok_or_else(|| ToolError::BadArgs("missing `command`".into()))?;

        let mut c = Command::new("sh");
        c.arg("-c").arg(cmd);
        if let Some(dir) = &self.cwd {
            c.current_dir(dir);
        }

        let out = c.output().map_err(ToolError::Io)?;
        let stdout = String::from_utf8_lossy(&out.stdout);
        let stderr = String::from_utf8_lossy(&out.stderr);
        let code = out.status.code().unwrap_or(-1);

        let mut content = String::new();
        content.push_str(&stdout);
        if !stderr.is_empty() {
            if !content.is_empty() {
                content.push('\n');
            }
            content.push_str(&stderr);
        }
        content.push_str(&format!("\n[exit {code}]"));
        Ok(if out.status.success() {
            Output::ok(content)
        } else {
            Output::err(content)
        })
    }
}
