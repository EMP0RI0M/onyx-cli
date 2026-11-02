//! Grep tool — fast content search across files in the workspace.
use crate::{Args, Output, Tool, ToolError};
use std::fs;
use std::path::{Path, PathBuf};

pub struct Grep {
    pub base_dir: PathBuf,
}

impl Grep {
    pub fn new(base_dir: impl AsRef<Path>) -> Self {
        Self { base_dir: base_dir.as_ref().to_path_buf() }
    }

    fn search_dir(&self, dir: &Path, pattern: &str, case_insensitive: bool, results: &mut Vec<String>, max_matches: usize) {
        let Ok(entries) = fs::read_dir(dir) else { return };
        for entry in entries.flatten() {
            if results.len() >= max_matches {
                return;
            }
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            // Skip common hidden or target dirs
            if name.starts_with('.') || name == "target" || name == "node_modules" {
                continue;
            }
            if path.is_dir() {
                self.search_dir(&path, pattern, case_insensitive, results, max_matches);
            } else if path.is_file() {
                if let Ok(content) = fs::read_to_string(&path) {
                    for (idx, line) in content.lines().enumerate() {
                        let matched = if case_insensitive {
                            line.to_lowercase().contains(&pattern.to_lowercase())
                        } else {
                            line.contains(pattern)
                        };
                        if matched {
                            let rel = path.strip_prefix(&self.base_dir).unwrap_or(&path).display();
                            results.push(format!("{rel}:{}: {line}", idx + 1));
                            if results.len() >= max_matches {
                                return;
                            }
                        }
                    }
                }
            }
        }
    }
}

impl Tool for Grep {
    fn name(&self) -> &str {
        "grep"
    }

    fn description(&self) -> &str {
        "Search files for regular text matches in the workspace. Args: pattern (string), path (optional directory, default .), case_insensitive (bool)."
    }

    fn arg_keys(&self) -> Vec<String> {
        vec!["pattern".into(), "path".into(), "case_insensitive".into()]
    }

    fn execute(&self, args: &Args) -> Result<Output, ToolError> {
        let pattern = args.get("pattern").and_then(|v| v.as_str()).ok_or_else(|| {
            ToolError::BadArgs("missing 'pattern'".into())
        })?;
        let sub = args.get("path").and_then(|v| v.as_str()).unwrap_or(".");
        let start_dir = self.base_dir.join(sub);
        let case_insensitive = args.get("case_insensitive").and_then(|v| v.as_bool()).unwrap_or(false);

        let mut results = Vec::new();
        self.search_dir(&start_dir, pattern, case_insensitive, &mut results, 100);

        if results.is_empty() {
            Ok(Output::ok("no matches found"))
        } else {
            Ok(Output::ok(results.join("\n")))
        }
    }
}
