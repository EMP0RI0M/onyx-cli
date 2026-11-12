//! Onyx Project Intelligence — Phase 1, 2 & 4:
//! Repository Architecture, Investigation Coverage, Work Memory & Architectural Intelligence.

pub mod cargo;
pub mod coverage;
pub mod events;
pub mod filesystem;
pub mod gaps;
pub mod impact;
pub mod jsts;
pub mod models;
pub mod rust_ast;
pub mod scanner;
pub mod symbol_retrieval;
pub mod tasks;
pub mod work_memory;

pub use coverage::{CoverageStatus, CoverageSummary, CoverageTracker, EvidenceRecord, FileCoverageRecord};
pub use events::{CoverageEvent, EvidenceSource, VerificationScope, VerificationStatus};
pub use gaps::{GapConfidence, GapKind, IntegrationGap, IntegrationGapDetector};
pub use impact::{ImpactAnalyzer, ImpactReport};
pub use models::{
    ArchitectureMap, CrateMeta, DependencyEdge, EdgeKind, FileMeta, ProjectTaskProfile,
    ScanWarning, SymbolKind, SymbolMeta, TaskGoal, WorkspaceMeta,
};
pub use scanner::ProjectScanner;
pub use symbol_retrieval::{RetrievedSymbol, SymbolRetriever};
pub use tasks::{extract_all_tasks, extract_makefile_targets, extract_package_json_scripts};
pub use work_memory::{ProjectPitfall, TaskSummary, WorkMemory};
