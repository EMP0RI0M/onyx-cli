//! Normalized data models for Onyx Project Intelligence.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Current schema version for `architecture.json`.
pub const SCHEMA_VERSION: &str = "1.0.0";
pub const SCANNER_VERSION: &str = "0.1.0";

/// Complete structural map of a project/repository.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ArchitectureMap {
    pub schema_version: String,
    pub scanner_version: String,
    pub root_path: String,
    pub scan_timestamp: u64,
    pub workspace: Option<WorkspaceMeta>,
    pub crates: BTreeMap<String, CrateMeta>,
    pub files: BTreeMap<String, FileMeta>,
    pub symbols: Vec<SymbolMeta>,
    pub dependencies: Vec<DependencyEdge>,
    pub scan_warnings: Vec<ScanWarning>,
    pub task_profiles: BTreeMap<String, ProjectTaskProfile>,
}

impl ArchitectureMap {
    pub fn new(root_path: impl Into<String>) -> Self {
        Self {
            schema_version: SCHEMA_VERSION.to_string(),
            scanner_version: SCANNER_VERSION.to_string(),
            root_path: root_path.into(),
            scan_timestamp: 0,
            workspace: None,
            crates: BTreeMap::new(),
            files: BTreeMap::new(),
            symbols: Vec::new(),
            dependencies: Vec::new(),
            scan_warnings: Vec::new(),
            task_profiles: BTreeMap::new(),
        }
    }
}

/// Category of developer task represented by a target.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskGoal {
    Test,
    Check,
    Clippy,
    Build,
    Lint,
    Custom,
}

/// A verified task profile extracted from a configuration manifest.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProjectTaskProfile {
    /// Task runner kind: "npm" | "nx" | "turbo" | "make" | "just" | "bazel"
    pub runner: String,
    /// Invocation target: e.g. "web:test", "test", "check", "app:lint"
    pub target: String,
    /// Inferred compression formatter kind (matches FormatterKind enum string name)
    pub formatter: String, // "cargo_test" | "jest_vitest" | "pytest" | "cargo_check" | "cargo_clippy"
    /// Primary task goal
    pub goal: TaskGoal,
    /// Provenance evidence
    pub source_file: String, // e.g. "package.json", "nx.json", "Makefile"
    pub raw_definition: String, // e.g. "vitest run --coverage"
}

/// Metadata about the workspace root (Cargo, npm/pnpm/yarn).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WorkspaceMeta {
    pub kind: String, // "cargo" | "npm" | "hybrid"
    pub members: Vec<String>,
}

/// Metadata about an individual package/crate.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CrateMeta {
    pub name: String,
    pub version: Option<String>,
    pub path: String,
    pub dependencies: Vec<String>,
}

/// File metadata tracking content hash, language, and size for incremental rescans.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FileMeta {
    pub path: String, // Relative to repo root
    pub language: String, // "rust" | "typescript" | "javascript" | "toml" | "json" | "markdown"
    pub sha256: String,
    pub size_bytes: u64,
    pub last_scanned_at: u64,
}

/// Category of extracted symbol.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SymbolKind {
    Function,
    Method,
    Struct,
    Enum,
    Trait,
    Impl,
    Class,
    Interface,
    TypeAlias,
    Module,
    Constant,
    Export,
}

/// A code symbol definition extracted from AST.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SymbolMeta {
    pub file: String,
    pub name: String,
    pub kind: SymbolKind,
    pub is_public: bool,
    pub line: usize,
    pub module_path: Option<String>,
    pub signature: Option<String>,
    pub conditional: Option<String>, // cfg(...) annotations
}

/// A dependency or relationship between files or modules.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DependencyEdge {
    pub from_file: String,
    pub to_target: String, // module, crate, or file path
    pub kind: EdgeKind,
    pub is_dynamic: bool,
    pub confidence: f32, // 1.0 = explicit static AST import
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EdgeKind {
    Import,      // use / import
    ReExport,    // pub use / export *
    CrateDep,    // Cargo dependency
    ModuleChild, // mod foo;
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ScanWarning {
    pub file: String,
    pub message: String,
}
