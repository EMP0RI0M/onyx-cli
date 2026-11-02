//! ListDir tool — list directory entries with depth and metadata.
use crate::{Args, Output, Tool, ToolError};
use std::fs;
use std::path::{Path, PathBuf};

pub struct ListDir {
    pub base_dir: PathBuf,
}

impl ListDir {
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

impl Tool for ListDir {
    fn name(&self) -> &str {
        "list_dir"
    }

    fn description(&self) -> &str {
        "List entries in a directory at `path` (default current dir), with type and size."
    }

    fn arg_keys(&self) -> Vec<String> {
        vec!["path".into(), "depth".into()]
    }

    fn execute(&self, args: &Args) -> Result<Output, ToolError> {
        let path_str = args.get("path").and_then(|v| v.as_str()).unwrap_or(".");
        let full = self.resolve(path_str)?;
        let entries = fs::read_dir(&full).map_err(ToolError::Io)?;

        let mut lines = Vec::new();
        for entry in entries.flatten() {
            let file_name = entry.file_name().to_string_lossy().to_string();
            let file_type = entry.file_type().ok();
            let is_dir = file_type.map_or(false, |ft| ft.is_dir());
            let size = entry.metadata().map(|m| m.len()).unwrap_or(0);

            if is_dir {
                lines.push(format!("{file_name}/"));
            } else {
                lines.push(format!("{file_name:<30} ({size} B)"));
            }
        }
        lines.sort();

        Ok(Output::ok(lines.join("\n")))
    }
}
