//! `lsp` tool — code intelligence and symbol navigation across workspaces.
use crate::{Args, Output, Tool, ToolError};
use std::fs;
use std::path::PathBuf;

#[derive(Default)]
pub struct LspTool {
    base_dir: PathBuf,
}

impl LspTool {
    pub fn new(base_dir: PathBuf) -> Self {
        Self { base_dir }
    }
}

impl Tool for LspTool {
    fn name(&self) -> &str {
        "lsp"
    }

    fn description(&self) -> &str {
        "Code intelligence via language symbols and definitions. Operations: goToDefinition, findReferences, hover, documentSymbol, workspaceSymbol. Args: operation (string), file_path (optional string), line (optional int), character (optional int), query (optional string)."
    }

    fn arg_keys(&self) -> Vec<String> {
        vec![
            "operation".into(),
            "file_path".into(),
            "line".into(),
            "character".into(),
            "query".into(),
        ]
    }

    fn execute(&self, args: &Args) -> Result<Output, ToolError> {
        let op = args.get("operation").and_then(|v| v.as_str()).unwrap_or("documentSymbol");
        let file_path = args.get("file_path").and_then(|v| v.as_str());
        let query = args.get("query").and_then(|v| v.as_str());
        let line = args.get("line").and_then(|v| v.as_u64()).map(|n| n as usize);

        match op {
            "documentSymbol" => {
                let path = file_path.ok_or_else(|| ToolError::BadArgs("missing 'file_path' for documentSymbol".into()))?;
                let target = self.base_dir.join(path);
                let content = fs::read_to_string(&target).map_err(ToolError::Io)?;
                let mut symbols = Vec::new();
                for (idx, l) in content.lines().enumerate() {
                    let trimmed = l.trim();
                    if trimmed.starts_with("pub fn ") || trimmed.starts_with("fn ") {
                        let name = trimmed.split('(').next().unwrap_or("").trim();
                        symbols.push(format!("Function: {name} (line {})", idx + 1));
                    } else if trimmed.starts_with("pub struct ") || trimmed.starts_with("struct ") {
                        let name = trimmed.split('{').next().unwrap_or("").trim();
                        symbols.push(format!("Struct: {name} (line {})", idx + 1));
                    } else if trimmed.starts_with("pub enum ") || trimmed.starts_with("enum ") {
                        let name = trimmed.split('{').next().unwrap_or("").trim();
                        symbols.push(format!("Enum: {name} (line {})", idx + 1));
                    } else if trimmed.starts_with("pub trait ") || trimmed.starts_with("trait ") {
                        let name = trimmed.split('{').next().unwrap_or("").trim();
                        symbols.push(format!("Trait: {name} (line {})", idx + 1));
                    } else if trimmed.starts_with("class ") || trimmed.starts_with("def ") {
                        symbols.push(format!("Python Def/Class: {trimmed} (line {})", idx + 1));
                    }
                }
                if symbols.is_empty() {
                    Ok(Output::ok(format!("No top-level symbols matched in {path}")))
                } else {
                    Ok(Output::ok(format!("Symbols in {path}:\n{}", symbols.join("\n"))))
                }
            }
            "workspaceSymbol" => {
                let q = query.ok_or_else(|| ToolError::BadArgs("missing 'query' for workspaceSymbol".into()))?;
                let mut results = Vec::new();
                // Grep for definition of symbol
                let res = std::process::Command::new("grep")
                    .arg("-rnE")
                    .arg(format!("(fn|struct|enum|class|def|type)[[:space:]]+{q}"))
                    .arg(self.base_dir.to_string_lossy().to_string())
                    .output();
                if let Ok(out) = res {
                    let text = String::from_utf8_lossy(&out.stdout);
                    for line in text.lines().take(20) {
                        results.push(line.to_string());
                    }
                }
                if results.is_empty() {
                    Ok(Output::ok(format!("No symbols found matching query: \"{q}\"")))
                } else {
                    Ok(Output::ok(format!("Workspace symbols matching \"{q}\":\n{}", results.join("\n"))))
                }
            }
            "goToDefinition" => {
                let path = file_path.ok_or_else(|| ToolError::BadArgs("missing 'file_path'".into()))?;
                let q = query.unwrap_or("");
                if q.is_empty() {
                    return Ok(Output::ok(format!("goToDefinition requested on {path} at line {line:?}")));
                }
                // Scan workspace
                let res = std::process::Command::new("grep")
                    .arg("-rnE")
                    .arg(format!("(pub[[:space:]]+)?(fn|struct|enum|class|trait)[[:space:]]+{q}"))
                    .arg(self.base_dir.to_string_lossy().to_string())
                    .output();
                match res {
                    Ok(out) => {
                        let text = String::from_utf8_lossy(&out.stdout);
                        let matches: Vec<&str> = text.lines().take(5).collect();
                        if matches.is_empty() {
                            Ok(Output::ok(format!("Definition for \"{q}\" not found.")))
                        } else {
                            Ok(Output::ok(format!("Definition(s) for \"{q}\":\n{}", matches.join("\n"))))
                        }
                    }
                    Err(e) => Err(ToolError::Io(e)),
                }
            }
            "findReferences" => {
                let q = query.ok_or_else(|| ToolError::BadArgs("missing 'query' (symbol name) to find references".into()))?;
                let res = std::process::Command::new("grep")
                    .arg("-rn")
                    .arg(q)
                    .arg(self.base_dir.to_string_lossy().to_string())
                    .output();
                match res {
                    Ok(out) => {
                        let text = String::from_utf8_lossy(&out.stdout);
                        let matches: Vec<&str> = text.lines().take(25).collect();
                        Ok(Output::ok(format!("References to \"{q}\" ({} found):\n{}", matches.len(), matches.join("\n"))))
                    }
                    Err(e) => Err(ToolError::Io(e)),
                }
            }
            "hover" => {
                let path = file_path.ok_or_else(|| ToolError::BadArgs("missing 'file_path'".into()))?;
                let l = line.unwrap_or(1);
                let target = self.base_dir.join(path);
                if let Ok(content) = fs::read_to_string(&target) {
                    if let Some(target_line) = content.lines().nth(l.saturating_sub(1)) {
                        return Ok(Output::ok(format!("Hover at {path}:{l}:\n`{}`", target_line.trim())));
                    }
                }
                Ok(Output::ok(format!("Hover info unavailable for {path}:{l}")))
            }
            other => Err(ToolError::BadArgs(format!("unsupported LSP operation: '{other}'"))),
        }
    }
}
