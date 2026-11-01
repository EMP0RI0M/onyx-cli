//! PythonExecute tool — OpenManus `PythonExecute`, ported to Rust.
//! Runs a code snippet through the host interpreter (`python3`) and returns
//! stdout+stderr+exit. The 20s wall-clock cap is delegated to coreutils
//! `timeout` so we never deadlock trying to drain a looping program.

use crate::{Args, Output, Tool, ToolError};
use std::process::Command;
use std::time::{Duration, SystemTime};

pub struct PythonExecute {
    /// Interpreter to invoke; defaults to `python3`.
    pub interpreter: String,
}

impl Default for PythonExecute {
    fn default() -> Self {
        Self { interpreter: "python3".into() }
    }
}

impl Tool for PythonExecute {
    fn name(&self) -> &str {
        "python"
    }
    fn arg_keys(&self) -> Vec<String> {
        vec!["code".into()]
    }
    fn description(&self) -> &str {
        "Execute a Python snippet on the host interpreter. Args: {\"code\": \"<python source>\"}. Returns stdout, stderr and exit code."
    }
    fn execute(&self, args: &Args) -> Result<Output, ToolError> {
        let code = ["code", "script", "snippet", "source"]
            .iter()
            .find_map(|k| args.get(*k).and_then(|v| v.as_str()))
            .ok_or_else(|| ToolError::BadArgs("missing `code`".into()))?;

        // Unique temp file so we never race another snippet; cleaned below.
        let stamp = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir();
        let path = dir.join(format!("tektile_py_{stamp}.py"));
        std::fs::write(&path, code).map_err(ToolError::Io)?;

        // `timeout 20 <interp> <file>` bounds runtime; output() collects pipes
        // without deadlocking (it reads them concurrently with the wait).
        let _ = Duration::from_secs(20); // documented budget, enforced by `timeout`
        let out = Command::new("timeout")
            .arg("20")
            .arg(&self.interpreter)
            .arg(&path)
            .output()
            .map_err(ToolError::Io)?;

        let _ = std::fs::remove_file(&path);

        let stdout = String::from_utf8_lossy(&out.stdout);
        let stderr = String::from_utf8_lossy(&out.stderr);
        let code_num = out.status.code().unwrap_or(-1);

        let mut content = String::new();
        content.push_str(&stdout);
        if !stderr.is_empty() {
            if !content.is_empty() {
                content.push('\n');
            }
            content.push_str(&stderr);
        }
        content.push_str(&format!("\n[exit {code_num}]"));

        Ok(if out.status.success() {
            Output::ok(content)
        } else {
            Output::err(content)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn python_runs_and_prints() {
        // Only meaningful where python3 exists; skip cleanly otherwise.
        let which = Command::new("sh").arg("-c").arg("command -v python3").output();
        if which.map(|o| o.stdout.is_empty()).unwrap_or(true) {
            return;
        }
        let out = PythonExecute::default()
            .execute(&json!({"code": "print(6*7)"}))
            .unwrap();
        assert!(!out.is_error, "{}", out.content);
        assert!(out.content.contains("42"));
    }
}
