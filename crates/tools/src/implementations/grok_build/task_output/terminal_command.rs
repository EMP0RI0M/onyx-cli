//! Background task process manager: tracking, output collection, termination, and awaiting.
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskInfo {
    pub task_id: String,
    pub command: String,
    pub pid: u32,
    pub stdout_path: PathBuf,
    pub stderr_path: PathBuf,
    pub completed: bool,
    pub exit_code: Option<i32>,
    pub started_at_secs: u64,
}

#[derive(Default)]
pub struct ProcessManager {
    tasks: Mutex<HashMap<String, TaskInfo>>,
}

impl ProcessManager {
    pub fn new() -> Self {
        Self {
            tasks: Mutex::new(HashMap::new()),
        }
    }

    pub fn register(&self, info: TaskInfo) {
        if let Ok(mut tasks) = self.tasks.lock() {
            tasks.insert(info.task_id.clone(), info);
        }
    }

    pub fn get_info(&self, task_id: &str) -> Option<TaskInfo> {
        let mut tasks = self.tasks.lock().ok()?;
        if let Some(info) = tasks.get_mut(task_id) {
            // Check status via /proc
            if !info.completed {
                let proc_path = format!("/proc/{}", info.pid);
                if !Path::new(&proc_path).exists() {
                    info.completed = true;
                }
            }
            return Some(info.clone());
        }
        None
    }

    /// Returns (is_running, output_content, is_truncated)
    pub fn get_output(&self, task_id: &str, max_bytes: usize) -> (bool, String, bool) {
        if let Some(info) = self.get_info(task_id) {
            let mut out = String::new();
            if let Ok(mut f) = File::open(&info.stdout_path) {
                let _ = f.read_to_string(&mut out);
            }
            let mut err = String::new();
            if let Ok(mut f) = File::open(&info.stderr_path) {
                let _ = f.read_to_string(&mut err);
            }

            let mut combined = if err.is_empty() {
                out
            } else {
                format!("{out}\n--- stderr ---\n{err}")
            };

            let truncated = if combined.len() > max_bytes {
                combined.truncate(max_bytes);
                true
            } else {
                false
            };

            (!info.completed, combined, truncated)
        } else {
            // Check system PID directly if not in manager
            let is_running = if let Ok(pid) = task_id.parse::<u32>() {
                Path::new(&format!("/proc/{pid}")).exists()
            } else {
                false
            };

            let ps = Command::new("ps").arg("-p").arg(task_id).output();
            let text = match ps {
                Ok(out) if out.status.success() => {
                    format!("Task {task_id} status:\n{}", String::from_utf8_lossy(&out.stdout))
                }
                _ => format!("Task {task_id} not active or not found in registry."),
            };
            (is_running, text, false)
        }
    }

    pub fn kill(&self, task_id: &str) -> Result<String, String> {
        let pid = if let Some(info) = self.get_info(task_id) {
            info.pid.to_string()
        } else {
            task_id.to_string()
        };

        let res = Command::new("kill").arg("-15").arg(&pid).output();
        match res {
            Ok(out) if out.status.success() => {
                if let Ok(mut tasks) = self.tasks.lock() {
                    if let Some(info) = tasks.get_mut(task_id) {
                        info.completed = true;
                        info.exit_code = Some(-15);
                    }
                }
                Ok(format!("Successfully terminated task/pid {task_id}"))
            }
            Ok(out) => {
                let err = String::from_utf8_lossy(&out.stderr);
                Err(format!("Failed to kill {task_id}: {err}"))
            }
            Err(e) => Err(e.to_string()),
        }
    }

    pub fn wait_all(&self, task_ids: &[String], timeout: Duration) -> HashMap<String, (bool, String)> {
        let start = Instant::now();
        let mut results = HashMap::new();

        loop {
            let mut all_done = true;
            for id in task_ids {
                let (running, output, _) = self.get_output(id, 8192);
                if running {
                    all_done = false;
                }
                results.insert(id.clone(), (running, output));
            }

            if all_done || start.elapsed() >= timeout {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }

        results
    }
}

pub static GLOBAL_PROCESS_MANAGER: std::sync::LazyLock<ProcessManager> =
    std::sync::LazyLock::new(ProcessManager::new);
