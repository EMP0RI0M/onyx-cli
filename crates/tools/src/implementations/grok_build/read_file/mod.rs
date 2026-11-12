//! ReadFile tool — reads file content with optional line windowing.
use crate::{Args, Output, Tool, ToolError};
use std::fs;
use std::path::{Path, PathBuf};

pub struct ReadFile {
    pub base_dir: PathBuf,
}

impl ReadFile {
    pub fn new(base_dir: impl AsRef<Path>) -> Self {
        Self { base_dir: base_dir.as_ref().to_path_buf() }
    }

    fn resolve(&self, path: &str) -> Result<PathBuf, ToolError> {
        let p = Path::new(path);
        let joined = if p.is_absolute() {
            p.to_path_buf()
        } else {
            self.base_dir.join(p)
        };
        if joined.components().any(|c| matches!(c, std::path::Component::ParentDir)) {
            return Err(ToolError::BadArgs("path traversal not allowed".into()));
        }
        Ok(joined)
    }
}

impl Tool for ReadFile {
    fn name(&self) -> &str {
        "read_file"
    }

    fn description(&self) -> &str {
        "Read contents of a file at `path`. Optionally specify `offset` (1-indexed start line) and `limit` (line count)."
    }

    fn arg_keys(&self) -> Vec<String> {
        vec!["path".into(), "offset".into(), "limit".into()]
    }

    fn execute(&self, args: &Args) -> Result<Output, ToolError> {
        let path = args.get("path").and_then(|v| v.as_str()).ok_or_else(|| {
            ToolError::BadArgs("missing 'path'".into())
        })?;
        let full = self.resolve(path)?;
        let content = match fs::read_to_string(&full) {
            Ok(c) => c,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                // If not found, check if common extension variation exists (e.g. .tsx vs .js, .ts vs .js)
                let p = Path::new(path);
                let mut alt_suggestion = None;
                if let Some(stem) = p.file_stem().and_then(|s| s.to_str()) {
                    let parent = p.parent().unwrap_or(Path::new(""));
                    for ext in &["tsx", "ts", "jsx", "js", "json", "rs", "html", "css"] {
                        let candidate = parent.join(format!("{stem}.{ext}"));
                        if let Ok(c_full) = self.resolve(&candidate.to_string_lossy()) {
                            if c_full.exists() {
                                alt_suggestion = Some(candidate.to_string_lossy().to_string());
                                break;
                            }
                        }
                    }
                }
                let hint = if let Some(alt) = alt_suggestion {
                    format!(" (Did you mean `{alt}`? That file exists.)")
                } else {
                    String::new()
                };
                return Err(ToolError::Other(format!("File not found: `{path}`.{hint} Check directory contents with `list_dir`.")));
            }
            Err(e) => return Err(ToolError::Io(e)),
        };
        
        let offset = args.get("offset").and_then(|v| v.as_u64()).unwrap_or(1) as usize;
        let limit = args.get("limit").and_then(|v| v.as_u64()).map(|v| v as usize);

        let lines: Vec<&str> = content.lines().collect();
        let total = lines.len();
        let start = offset.saturating_sub(1).min(total);
        let end = match limit {
            Some(lim) => (start + lim).min(total),
            None => total,
        };

        let mut out = String::new();
        for (i, line) in lines[start..end].iter().enumerate() {
            let line_no = start + i + 1;
            out.push_str(&format!("{line_no:6}│ {line}\n"));
        }

        Ok(Output::ok(out))
    }
}
