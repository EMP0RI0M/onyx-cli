//! SearchReplace tool — targeted multi-line string replacement in a file.
use crate::{Args, Output, Tool, ToolError};
use std::fs;
use std::path::{Path, PathBuf};

pub struct SearchReplace {
    pub base_dir: PathBuf,
}

impl SearchReplace {
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

impl Tool for SearchReplace {
    fn name(&self) -> &str {
        "search_replace"
    }

    fn description(&self) -> &str {
        "Replace occurrences of `old_str` with `new_str` in a target file at `path`."
    }

    fn arg_keys(&self) -> Vec<String> {
        vec!["path".into(), "old_str".into(), "new_str".into()]
    }

    fn execute(&self, args: &Args) -> Result<Output, ToolError> {
        let path = args.get("path").and_then(|v| v.as_str()).ok_or_else(|| {
            ToolError::BadArgs("missing 'path'".into())
        })?;
        let old_str = args.get("old_str").and_then(|v| v.as_str()).ok_or_else(|| {
            ToolError::BadArgs("missing 'old_str'".into())
        })?;
        let new_str = args.get("new_str").and_then(|v| v.as_str()).unwrap_or("");

        let full = self.resolve(path)?;
        let content = fs::read_to_string(&full).map_err(ToolError::Io)?;

        if !content.contains(old_str) {
            let norm_old = old_str.replace("\r\n", "\n");
            let norm_content = content.replace("\r\n", "\n");
            if norm_content.contains(&norm_old) {
                let occurrences = norm_content.matches(&norm_old).count();
                let replaced = norm_content.replace(&norm_old, new_str);
                fs::write(&full, replaced).map_err(ToolError::Io)?;
                return Ok(Output::ok(format!("successfully replaced {occurrences} occurrence(s) in `{path}` (with newline normalization)")));
            }
            return Err(ToolError::Other(format!("`old_str` not found in `{path}`. Inspect the file with `read_file` first.")));
        }

        let occurrences = content.matches(old_str).count();
        let replaced = content.replace(old_str, new_str);
        fs::write(&full, replaced).map_err(ToolError::Io)?;

        Ok(Output::ok(format!("successfully replaced {occurrences} occurrence(s) in `{path}`")))
    }
}
