//! Filesystem crawler respecting .gitignore, exclusions, and secret patterns.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

/// Standard directories to exclude from scanning.
const DEFAULT_IGNORED_DIRS: &[&str] = &[
    "target",
    "node_modules",
    "dist",
    "build",
    ".git",
    ".next",
    ".cache",
    ".onyx",
    "vendor",
];

/// Patterns of filenames that should NEVER be indexed for secrets/credentials safety.
const SECRET_FILE_PATTERNS: &[&str] = &[
    ".env",
    ".env.local",
    ".env.production",
    ".env.development",
    "id_rsa",
    "id_ed25519",
    ".pem",
    ".key",
    "credentials.json",
    "service-account",
    "token",
];

pub struct FileFilter {
    pub root: PathBuf,
    pub gitignore_entries: HashSet<String>,
}

impl FileFilter {
    pub fn new(root: impl AsRef<Path>) -> Self {
        let root = root.as_ref().to_path_buf();
        let mut gitignore_entries = HashSet::new();

        // Parse root .gitignore if present
        let gitignore_path = root.join(".gitignore");
        if let Ok(content) = fs::read_to_string(gitignore_path) {
            for line in content.lines() {
                let trimmed = line.trim();
                if !trimmed.is_empty() && !trimmed.starts_with('#') {
                    // Normalize entry: remove trailing slash
                    let entry = trimmed.trim_end_matches('/').trim_start_matches("./");
                    gitignore_entries.insert(entry.to_string());
                }
            }
        }

        Self {
            root,
            gitignore_entries,
        }
    }

    /// Check if a path should be ignored (directories, build artifacts, gitignore, secrets).
    pub fn is_ignored(&self, rel_path: &str, is_dir: bool) -> bool {
        let parts: Vec<&str> = rel_path.split('/').filter(|s| !s.is_empty()).collect();
        if parts.is_empty() {
            return false;
        }

        // Check if any path component is in the standard ignore list
        for part in &parts {
            if DEFAULT_IGNORED_DIRS.contains(part) {
                return true;
            }
        }

        // Check gitignore matches
        let name = parts.last().unwrap_or(&"");
        if self.gitignore_entries.contains(*name) || self.gitignore_entries.contains(rel_path) {
            return true;
        }

        // Check if any prefix directory matches a gitignore entry
        let mut prefix = String::new();
        for (i, part) in parts.iter().enumerate() {
            if i > 0 {
                prefix.push('/');
            }
            prefix.push_str(part);
            if self.gitignore_entries.contains(&prefix) || self.gitignore_entries.contains(*part) {
                return true;
            }
        }

        // Check secret file patterns
        if !is_dir {
            let lower = name.to_lowercase();
            for pat in SECRET_FILE_PATTERNS {
                if lower == *pat || lower.ends_with(pat) {
                    return true;
                }
            }
        }

        false
    }

    /// Identify the supported language from the file extension.
    pub fn detect_language(path: &str) -> Option<&'static str> {
        let ext = Path::new(path).extension()?.to_str()?.to_lowercase();
        match ext.as_str() {
            "rs" => Some("rust"),
            "ts" | "tsx" => Some("typescript"),
            "js" | "jsx" | "mjs" | "cjs" => Some("javascript"),
            "toml" => Some("toml"),
            "json" => Some("json"),
            "md" | "markdown" => Some("markdown"),
            _ => None,
        }
    }

    /// Scan directory and yield eligible files with relative paths.
    pub fn scan_files(&self) -> Vec<(String, &'static str, u64)> {
        let mut results = Vec::new();
        for entry in WalkDir::new(&self.root).follow_links(false).into_iter().filter_map(|e| e.ok()) {
            let path = entry.path();
            let is_dir = entry.file_type().is_dir();
            let Ok(rel) = path.strip_prefix(&self.root) else { continue };
            let rel_str = rel.to_string_lossy().to_string();
            if rel_str.is_empty() {
                continue;
            }

            if self.is_ignored(&rel_str, is_dir) {
                continue;
            }

            if !is_dir {
                if let Some(lang) = Self::detect_language(&rel_str) {
                    let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
                    results.push((rel_str, lang, size));
                }
            }
        }
        results.sort_by(|a, b| a.0.cmp(&b.0));
        results
    }
}
