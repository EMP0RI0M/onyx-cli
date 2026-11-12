//! Incremental project scanner and atomic architecture.json persistence.

use crate::cargo::CargoScanner;
use crate::filesystem::FileFilter;
use crate::jsts::JsTsExtractor;
use crate::models::{ArchitectureMap, FileMeta, ScanWarning};
use crate::rust_ast::RustAstExtractor;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub struct ProjectScanner {
    pub root: PathBuf,
}

impl ProjectScanner {
    pub fn new(root: impl AsRef<Path>) -> Self {
        Self {
            root: root.as_ref().to_path_buf(),
        }
    }

    pub fn output_path(&self) -> PathBuf {
        self.root.join(".onyx/project/architecture.json")
    }

    /// Load existing ArchitectureMap if present on disk.
    pub fn load_existing(&self) -> Option<ArchitectureMap> {
        let path = self.output_path();
        if !path.exists() {
            return None;
        }
        let content = fs::read_to_string(path).ok()?;
        serde_json::from_str(&content).ok()
    }

    /// Run full scan (re-indexes everything).
    pub fn scan_full(&self) -> Result<ArchitectureMap, String> {
        self.scan_internal(None)
    }

    /// Run incremental scan (reuses unchanged files based on SHA-256).
    pub fn scan_incremental(&self) -> Result<ArchitectureMap, String> {
        let existing = self.load_existing();
        self.scan_internal(existing)
    }

    fn scan_internal(&self, existing: Option<ArchitectureMap>) -> Result<ArchitectureMap, String> {
        let filter = FileFilter::new(&self.root);
        let eligible = filter.scan_files();

        let mut map = ArchitectureMap::new(self.root.to_string_lossy().to_string());
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        map.scan_timestamp = now;

        // 1. Cargo workspace & dependency scan
        let (ws, crates, crate_edges) = CargoScanner::scan_workspace(&self.root);
        map.workspace = ws;
        map.crates = crates;
        map.dependencies.extend(crate_edges);

        let mut existing_files: BTreeMap<String, FileMeta> = BTreeMap::new();
        let mut existing_symbols = Vec::new();
        let mut existing_edges = Vec::new();

        if let Some(prev) = existing {
            existing_files = prev.files;
            existing_symbols = prev.symbols;
            existing_edges = prev.dependencies;
        }

        // Retain edges that are crate deps
        map.dependencies.retain(|e| e.kind == crate::models::EdgeKind::CrateDep);

        // 2. Scan each eligible file incrementally
        for (rel_path, lang, size) in eligible {
            let full_path = self.root.join(&rel_path);
            let Ok(bytes) = fs::read(&full_path) else {
                map.scan_warnings.push(ScanWarning {
                    file: rel_path.clone(),
                    message: "Failed to read file for hashing".to_string(),
                });
                continue;
            };

            let mut hasher = Sha256::new();
            hasher.update(&bytes);
            let hash = hex::encode(hasher.finalize());

            // Check if file is unchanged
            let unchanged = match existing_files.get(&rel_path) {
                Some(prev) => prev.sha256 == hash,
                None => false,
            };

            if unchanged {
                // Reuse existing metadata, symbols, and edges
                map.files.insert(
                    rel_path.clone(),
                    existing_files.remove(&rel_path).unwrap(),
                );
                for sym in existing_symbols.iter().filter(|s| s.file == rel_path) {
                    map.symbols.push(sym.clone());
                }
                for edge in existing_edges.iter().filter(|e| e.from_file == rel_path) {
                    map.dependencies.push(edge.clone());
                }
            } else {
                // Content changed or new file — re-extract AST
                let content = String::from_utf8_lossy(&bytes);
                match lang {
                    "rust" => {
                        match RustAstExtractor::extract(&rel_path, &content) {
                            Ok((syms, edges)) => {
                                map.symbols.extend(syms);
                                map.dependencies.extend(edges);
                            }
                            Err(e) => {
                                map.scan_warnings.push(ScanWarning {
                                    file: rel_path.clone(),
                                    message: format!("Rust syntax parse error: {e}"),
                                });
                            }
                        }
                    }
                    "javascript" | "typescript" => {
                        let (syms, edges) = JsTsExtractor::extract(&rel_path, &content);
                        map.symbols.extend(syms);
                        map.dependencies.extend(edges);
                    }
                    _ => {}
                }

                map.files.insert(
                    rel_path.clone(),
                    FileMeta {
                        path: rel_path,
                        language: lang.to_string(),
                        sha256: hash,
                        size_bytes: size,
                        last_scanned_at: now,
                    },
                );
            }
        }

        // 3. Statically extract task profiles from manifests (package.json, nx.json, Makefile, etc.)
        map.task_profiles = crate::tasks::extract_all_tasks(&self.root);

        // 4. Atomically persist to .onyx/project/architecture.json
        self.save_atomic(&map)?;

        Ok(map)
    }

    /// Write architecture.json atomically via temporary file and rename.
    pub fn save_atomic(&self, map: &ArchitectureMap) -> Result<(), String> {
        let out_path = self.output_path();
        if let Some(parent) = out_path.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }

        let temp_path = out_path.with_extension("tmp");
        let json_data = serde_json::to_string_pretty(map).map_err(|e| e.to_string())?;
        fs::write(&temp_path, json_data).map_err(|e| e.to_string())?;
        fs::rename(&temp_path, &out_path).map_err(|e| e.to_string())?;

        Ok(())
    }
}
