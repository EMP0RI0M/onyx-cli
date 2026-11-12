//! ListDir tool — list directory entries with depth, recursion, and metadata.
use crate::{Args, Output, Tool, ToolError};
use std::fs;
use std::path::{Component, Path, PathBuf};

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
        if joined.components().any(|c| matches!(c, Component::ParentDir)) {
            return Err(ToolError::BadArgs("path traversal not allowed".into()));
        }
        Ok(joined)
    }

    fn collect_entries(
        &self,
        dir: &Path,
        current_depth: usize,
        max_depth: usize,
        prefix: &str,
        lines: &mut Vec<String>,
        dir_count: &mut usize,
        file_count: &mut usize,
    ) {
        let Ok(entries) = fs::read_dir(dir) else { return };
        let mut items: Vec<_> = entries.flatten().collect();
        items.sort_by_key(|e| e.file_name());

        for entry in items {
            let name = entry.file_name().to_string_lossy().to_string();
            let file_type = entry.file_type().ok();
            let is_dir = file_type.map_or(false, |ft| ft.is_dir());
            let rel_display = if prefix.is_empty() {
                name.clone()
            } else {
                format!("{prefix}/{name}")
            };

            if is_dir {
                *dir_count += 1;
                lines.push(format!("{rel_display}/"));
                if current_depth + 1 < max_depth {
                    self.collect_entries(
                        &entry.path(),
                        current_depth + 1,
                        max_depth,
                        &rel_display,
                        lines,
                        dir_count,
                        file_count,
                    );
                }
            } else {
                *file_count += 1;
                let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
                lines.push(format!("{rel_display:<35} ({size} B)"));
            }
        }
    }
}

impl Tool for ListDir {
    fn name(&self) -> &str {
        "list_dir"
    }

    fn description(&self) -> &str {
        "List entries in a directory at `path` (default current working directory '.'), with type, size, counts, depth traversal, and pagination."
    }

    fn schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to directory (defaults to current dir '.'). Use '.' for current working directory or '/' for root."
                },
                "offset": {
                    "type": "integer",
                    "description": "Starting index for pagination (default: 0)"
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of items to return (default: 50, max: 100)"
                },
                "depth": {
                    "type": "integer",
                    "description": "Directory depth traversal limit (default: 1 for shallow listing, >= 2 for recursive)"
                }
            },
            "additionalProperties": false
        })
    }

    fn execute(&self, args: &Args) -> Result<Output, ToolError> {
        let path_str = args.get("path").and_then(|v| v.as_str()).unwrap_or(".");
        let full = self.resolve(path_str)?;

        if !full.exists() {
            return Err(ToolError::Other(format!(
                "Directory not found: `{path_str}` (resolved to `{}`). Check path or use '.' for current directory.",
                full.display()
            )));
        }
        if !full.is_dir() {
            return Err(ToolError::Other(format!(
                "Path `{path_str}` is a file, not a directory. Use read_file to inspect its contents."
            )));
        }

        // Parse offset, limit and depth supporting both integer and string representations
        fn parse_usize_arg(args: &Args, key: &str) -> Option<usize> {
            args.get(key).and_then(|v| {
                if let Some(u) = v.as_u64() {
                    Some(u as usize)
                } else if let Some(s) = v.as_str() {
                    s.trim().parse::<usize>().ok()
                } else {
                    None
                }
            })
        }

        let max_depth = parse_usize_arg(args, "depth").unwrap_or(1).max(1).min(10);

        let mut lines = Vec::new();
        let mut dir_count = 0usize;
        let mut file_count = 0usize;

        self.collect_entries(&full, 0, max_depth, "", &mut lines, &mut dir_count, &mut file_count);

        let total = dir_count + file_count;

        const DEFAULT_PAGE_SIZE: usize = 50;
        const MAX_PAGE_SIZE: usize = 100;
        const MAX_UNPAGINATED_THRESHOLD: usize = 60;

        let req_offset = parse_usize_arg(args, "offset");
        let req_limit = parse_usize_arg(args, "limit");

        // If directory is large and caller did not specify pagination parameters, default to first page
        let is_auto_preview = total > MAX_UNPAGINATED_THRESHOLD && req_offset.is_none() && req_limit.is_none();

        let offset = req_offset.unwrap_or(0);
        let raw_limit = if is_auto_preview {
            DEFAULT_PAGE_SIZE
        } else {
            req_limit.unwrap_or(DEFAULT_PAGE_SIZE)
        };
        // Enforce maximum page size limit
        let limit = raw_limit.min(MAX_PAGE_SIZE);

        let start = offset.min(lines.len());
        let end = (start + limit).min(lines.len());
        let returned_count = end.saturating_sub(start);
        let has_more = end < lines.len();
        let next_offset = if has_more { Some(end) } else { None };

        let mut out = format!(
            "[Directory Listing: `{path_str}` | Total: {total} ({dir_count} directories, {file_count} files) | depth: {max_depth}, offset: {start}, count: {returned_count}, has_more: {has_more}, next_offset: {}]\n",
            next_offset.map(|o| o.to_string()).unwrap_or_else(|| "null".to_string())
        );

        for l in &lines[start..end] {
            out.push_str(l);
            out.push('\n');
        }

        if has_more {
            out.push_str(&format!(
                "\n... [Showing entries {start} to {end} of {total}. To continue fetching, call list_dir(path=\"{path_str}\", offset={end}, limit={limit})] ..."
            ));
        }

        Ok(Output::ok(out.trim_end().to_string()))
    }
}
