//! StrReplaceEditor tool — create / view / str_replace over a sandboxed root dir.
//! OpenManus `StrReplaceEditor`, ported. Path is confined to `base_dir` so a tool
//! call cannot read or write outside the workspace.

use crate::{Args, Output, Tool, ToolError};
use std::fs;
use std::path::{Path, PathBuf};

pub struct StrReplaceEditor {
    pub base_dir: PathBuf,
}

impl StrReplaceEditor {
    pub fn new(base_dir: impl AsRef<Path>) -> Self {
        Self { base_dir: base_dir.as_ref().to_path_buf() }
    }

    /// Join `path` under base_dir, refusing absolute paths and `..` escapes.
    fn resolve(&self, path: &str) -> Result<PathBuf, ToolError> {
        let p = Path::new(path);
        if p.is_absolute() {
            return Err(ToolError::BadArgs("absolute paths not allowed".into()));
        }
        let joined = self.base_dir.join(p);
        // Reject any `..` component before touching the filesystem.
        if joined
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
        {
            return Err(ToolError::BadArgs("path traversal not allowed".into()));
        }
        Ok(joined)
    }
}

impl Tool for StrReplaceEditor {
    fn name(&self) -> &str {
        "editor"
    }
    fn arg_keys(&self) -> Vec<String> {
        vec!["command".into(), "path".into(), "file_text".into(), "old_str".into(), "new_str".into()]
    }
    fn description(&self) -> &str {
        "create|view|str_replace a file under the workspace root."
    }
    fn execute(&self, args: &Args) -> Result<Output, ToolError> {
        let command = args
            .get("command")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::BadArgs("missing `command`".into()))?;
        let path = args
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::BadArgs("missing `path`".into()))?;
        let full = self.resolve(path)?;

        match command {
            "view" => match fs::read_to_string(&full) {
                Ok(txt) => Ok(Output::ok(txt)),
                Err(e) => Err(ToolError::Io(e)),
            },
            "create" => {
                let text = args
                    .get("file_text")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if let Some(parent) = full.parent() {
                    fs::create_dir_all(parent).map_err(ToolError::Io)?;
                }
                fs::write(&full, text).map_err(ToolError::Io)?;
                Ok(Output::ok(format!("created {path} ({} bytes)", text.len())))
            }
            "str_replace" => {
                let old = args
                    .get("old_str")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ToolError::BadArgs("missing `old_str`".into()))?;
                let new = args
                    .get("new_str")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ToolError::BadArgs("missing `new_str`".into()))?;
                let cur = fs::read_to_string(&full).map_err(ToolError::Io)?;
                let count = cur.matches(old).count();
                if count == 0 {
                    return Ok(Output::err("old_str not found"));
                }
                if count > 1 {
                    return Ok(Output::err(format!("old_str not unique ({count} matches)")));
                }
                let updated = cur.replace(old, new);
                fs::write(&full, &updated).map_err(ToolError::Io)?;
                Ok(Output::ok(format!("patched {path}")))
            }
            other => Err(ToolError::BadArgs(format!("unknown command `{other}`"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn create_view_str_replace() {
        let dir = std::env::temp_dir().join("tektile_editor_test");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let ed = StrReplaceEditor::new(&dir);

        let o = ed
            .execute(&json!({"command":"create","path":"a.txt","file_text":"hello world"}))
            .unwrap();
        assert!(!o.is_error);

        let v = ed.execute(&json!({"command":"view","path":"a.txt"})).unwrap();
        assert_eq!(v.content, "hello world");

        let p = ed
            .execute(&json!({"command":"str_replace","path":"a.txt","old_str":"world","new_str":"rust"}))
            .unwrap();
        assert!(!p.is_error);
        let v2 = ed.execute(&json!({"command":"view","path":"a.txt"})).unwrap();
        assert_eq!(v2.content, "hello rust");

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn traversal_denied() {
        let dir = std::env::temp_dir().join("tektile_editor_test2");
        fs::create_dir_all(&dir).unwrap();
        let ed = StrReplaceEditor::new(&dir);
        let bad = ed.execute(&json!({"command":"view","path":"../../etc/passwd"}));
        assert!(matches!(bad, Err(ToolError::BadArgs(_))));
        fs::remove_dir_all(&dir).unwrap();
    }
}
