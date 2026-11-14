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

/// Output compression is enabled by default to preserve LLM token context.
/// It can be explicitly disabled by setting `ONYX_COMPACT=0` or `ONYX_COMPACT=false`.
pub fn compact_on() -> bool {
    use std::sync::OnceLock;
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        match std::env::var("ONYX_COMPACT").as_deref() {
            Ok("0") | Ok("false") | Ok("off") => false,
            _ => true,
        }
    })
}

/// Canonical post-execution formatting and observation pipeline for shell command results.
/// Both `Bash` and `SandboxedBash` route their results through this canonical function.
pub fn format_command_output(cmd: &str, content: &str, code: i32, is_success: bool) -> Output {
    if compact_on() {
        let raw_content = content.to_string();
        let (observation, mut body, savings) =
            crate::compress::process_and_compact(cmd, content, None);
        crate::compress::record_with_formatter(savings, observation.kind);
        body.push_str(&format!("\n[exit {code}]"));
        if is_success {
            Output::ok(body).with_observation(raw_content, observation)
        } else {
            Output::err(body).with_observation(raw_content, observation)
        }
    } else {
        let mut text = content.to_string();
        text.push_str(&format!("\n[exit {code}]"));
        if is_success {
            Output::ok(text)
        } else {
            Output::err(text)
        }
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
        c.stdout(std::process::Stdio::piped());
        c.stderr(std::process::Stdio::piped());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            c.process_group(0);
        }
        if let Some(dir) = &self.cwd {
            c.current_dir(dir);
        }

        let mut child = c.spawn().map_err(ToolError::Io)?;

        // Immediately start background readers triggered by OS kernel when data arrives,
        // draining stdout/stderr pipes so the 64KB pipe buffer is freed up instantly.
        // Stream chunks over mpsc channels with bounded recv_timeout after exit to prevent hanging
        // if a background process (e.g. `sleep 300 &`) inherited the pipe.
        let (tx_out, rx_out) = std::sync::mpsc::channel();
        let mut stdout_pipe = child.stdout.take();
        std::thread::spawn(move || {
            if let Some(mut pipe) = stdout_pipe.take() {
                let mut buf = [0u8; 8192];
                while let Ok(n) = std::io::Read::read(&mut pipe, &mut buf) {
                    if n == 0 { break; }
                    if tx_out.send(buf[..n].to_vec()).is_err() {
                        break;
                    }
                }
            }
        });

        let (tx_err, rx_err) = std::sync::mpsc::channel();
        let mut stderr_pipe = child.stderr.take();
        std::thread::spawn(move || {
            if let Some(mut pipe) = stderr_pipe.take() {
                let mut buf = [0u8; 8192];
                while let Ok(n) = std::io::Read::read(&mut pipe, &mut buf) {
                    if n == 0 { break; }
                    if tx_err.send(buf[..n].to_vec()).is_err() {
                        break;
                    }
                }
            }
        });

        // Default command timeout: 60 seconds (prevents hanging indefinitely on dev servers like `npm run dev`)
        let timeout_secs = std::env::var("ONYX_BASH_TIMEOUT")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(60);

        let start = std::time::Instant::now();
        let mut timed_out = false;
        let status = loop {
            match child.try_wait().map_err(ToolError::Io)? {
                Some(status) => break status,
                None => {
                    if start.elapsed() > std::time::Duration::from_secs(timeout_secs) {
                        timed_out = true;
                        #[cfg(unix)]
                        {
                            let pid = child.id() as libc::pid_t;
                            unsafe {
                                libc::kill(-pid, libc::SIGKILL);
                            }
                        }
                        let _ = child.kill();
                        let term_status = child.wait().ok();
                        break term_status.unwrap_or_default();
                    }
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
            }
        };

        let mut stdout = Vec::new();
        while let Ok(chunk) = rx_out.try_recv() {
            stdout.extend_from_slice(&chunk);
        }
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(150);
        while let Ok(chunk) = rx_out.recv_timeout(deadline.saturating_duration_since(std::time::Instant::now())) {
            stdout.extend_from_slice(&chunk);
        }

        let mut stderr = Vec::new();
        while let Ok(chunk) = rx_err.try_recv() {
            stderr.extend_from_slice(&chunk);
        }
        let deadline_err = std::time::Instant::now() + std::time::Duration::from_millis(150);
        while let Ok(chunk) = rx_err.recv_timeout(deadline_err.saturating_duration_since(std::time::Instant::now())) {
            stderr.extend_from_slice(&chunk);
        }

        let stdout_str = String::from_utf8_lossy(&stdout);
        let stderr_str = String::from_utf8_lossy(&stderr);
        let code = if timed_out {
            124
        } else {
            status.code().unwrap_or(-1)
        };

        let mut content = String::new();
        content.push_str(&stdout_str);
        if !stderr_str.is_empty() {
            if !content.is_empty() {
                content.push('\n');
            }
            content.push_str(&stderr_str);
        }
        if timed_out {
            if !content.is_empty() {
                content.push('\n');
            }
            content.push_str(&format!("[command timed out after {timeout_secs}s and was terminated. If this is a background dev server (e.g. `npm run dev`), run it in the background or with a daemon runner.]"));
        }

        let is_success = status.success() && !timed_out;

        Ok(format_command_output(cmd, &content, code, is_success))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn bash_large_output_does_not_deadlock() {
        let tool = Bash::new();
        // 200,000 bytes > 64KB OS pipe buffer
        let args = json!({"command": "python3 -c \"print('A' * 200000)\""});
        let start = std::time::Instant::now();
        let res = tool.execute(&args).expect("must succeed without deadlocking");
        assert!(start.elapsed() < std::time::Duration::from_secs(5), "execution must be immediate, took {:?}", start.elapsed());
        assert!(!res.is_error);
    }

    #[test]
    fn bash_timeout_returns_code_124() {
        std::env::set_var("ONYX_BASH_TIMEOUT", "1");
        let tool = Bash::new();
        let args = json!({"command": "sleep 5"});
        let res = tool.execute(&args).expect("must return output");
        std::env::remove_var("ONYX_BASH_TIMEOUT");
        assert!(res.is_error, "timed out command must report is_error");
        assert!(res.content.contains("[exit 124]"), "content must indicate exit 124, got: {}", res.content);
        assert!(res.content.contains("timed out"), "content must indicate timed out, got: {}", res.content);
    }

    #[test]
    fn bash_background_grandchild_does_not_hang() {
        let tool = Bash::new();
        // Spawns a background sleep holding inherited file descriptors, but shell exits immediately.
        let args = json!({"command": "sleep 10 & echo done"});
        let start = std::time::Instant::now();
        let res = tool.execute(&args).expect("must return output");
        assert!(start.elapsed() < std::time::Duration::from_secs(3), "must not block on background process, took {:?}", start.elapsed());
        assert!(!res.is_error);
        assert!(res.content.contains("done"));
    }
}
