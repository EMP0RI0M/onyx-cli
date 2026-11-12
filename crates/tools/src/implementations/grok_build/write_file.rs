//! WriteFile tool — create or overwrite a file with given content.
use crate::{Args, Output, Tool, ToolError};
use std::fs;
use std::path::{Path, PathBuf};

pub struct WriteFile {
    pub base_dir: PathBuf,
}

impl WriteFile {
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

impl Tool for WriteFile {
    fn name(&self) -> &str {
        "write_file"
    }

    fn description(&self) -> &str {
        "Write or overwrite a file at `path` with `content` (or `file_text`). Creates parent directories if needed."
    }

    fn arg_keys(&self) -> Vec<String> {
        vec!["path".into(), "content".into(), "file_text".into()]
    }

    fn execute(&self, args: &Args) -> Result<Output, ToolError> {
        let path = args.get("path")
            .or_else(|| args.get("file"))
            .or_else(|| args.get("file_path"))
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::BadArgs("missing 'path'".into()))?;

        let content = args.get("content")
            .or_else(|| args.get("file_text"))
            .or_else(|| args.get("text"))
            .and_then(|v| v.as_str())
            .unwrap_or("");

        let full = self.resolve(path)?;
        if let Some(parent) = full.parent() {
            fs::create_dir_all(parent).map_err(ToolError::Io)?;
        }

        fs::write(&full, content).map_err(ToolError::Io)?;
        let byte_cnt = content.len();
        let line_cnt = content.lines().count();
        Ok(Output::ok(format!("successfully wrote `{path}` ({byte_cnt} bytes, {line_cnt} lines)")))
    }
}
