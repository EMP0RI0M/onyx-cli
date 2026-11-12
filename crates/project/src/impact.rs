use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

use crate::coverage::CoverageTracker;
use crate::models::{ArchitectureMap, EdgeKind};
use crate::work_memory::WorkMemory;

/// Report detailing the blast radius and architectural consequences of modifying a component.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImpactReport {
    pub target_file: String,
    pub target_symbol: Option<String>,
    pub direct_dependents: Vec<String>,
    pub indirect_dependents: Vec<String>,
    pub affected_modules: Vec<String>,
    pub related_tests: Vec<String>,
    pub relevant_configs: Vec<String>,
    pub historical_regressions: Vec<String>,
    pub stale_or_unverified_dependents: Vec<String>,
    pub risk_score: f32, // 0.0 (safe leaf) to 1.0 (critical core component)
}

/// Analyzer to compute blast radius before editing symbols or files.
pub struct ImpactAnalyzer<'a> {
    pub arch: &'a ArchitectureMap,
    pub coverage: Option<&'a CoverageTracker>,
    pub memory: Option<&'a WorkMemory>,
}

impl<'a> ImpactAnalyzer<'a> {
    pub fn new(arch: &'a ArchitectureMap) -> Self {
        Self {
            arch,
            coverage: None,
            memory: None,
        }
    }

    pub fn with_coverage(mut self, cov: &'a CoverageTracker) -> Self {
        self.coverage = Some(cov);
        self
    }

    pub fn with_memory(mut self, mem: &'a WorkMemory) -> Self {
        self.memory = Some(mem);
        self
    }

    /// Analyze impact for a given file path and optional symbol name.
    pub fn analyze_impact(&self, file_path: &str, symbol_name: Option<&str>) -> ImpactReport {
        // Build reverse dependency graph (who imports `file_path`)
        let mut reverse_deps: HashMap<&str, Vec<&str>> = HashMap::new();
        for edge in &self.arch.dependencies {
            if edge.kind == EdgeKind::Import || edge.kind == EdgeKind::CrateDep || edge.kind == EdgeKind::ModuleChild {
                // edge.from_file depends on edge.to_target
                reverse_deps
                    .entry(edge.to_target.as_str())
                    .or_default()
                    .push(edge.from_file.as_str());
            }
        }

        // 1. Direct dependents
        let mut direct = HashSet::new();
        for (target, callers) in &reverse_deps {
            if target.contains(file_path) || file_path.ends_with(target) {
                for caller in callers {
                    if *caller != file_path {
                        direct.insert(caller.to_string());
                    }
                }
            }
        }

        // Also check if any symbol matches specifically
        if let Some(sym) = symbol_name {
            for edge in &self.arch.dependencies {
                if edge.to_target.contains(sym) && edge.from_file != file_path {
                    direct.insert(edge.from_file.clone());
                }
            }
        }

        // 2. Transitive (indirect) dependents (BFS up to depth 3)
        let mut indirect = HashSet::new();
        let mut queue: Vec<String> = direct.iter().cloned().collect();
        let mut visited: HashSet<String> = direct.iter().cloned().collect();
        visited.insert(file_path.to_string());

        let mut depth = 0;
        while !queue.is_empty() && depth < 3 {
            let mut next_queue = Vec::new();
            for item in queue {
                for (target, callers) in &reverse_deps {
                    if target.contains(&item) || item.ends_with(target) {
                        for caller in callers {
                            if !visited.contains(*caller) {
                                visited.insert(caller.to_string());
                                indirect.insert(caller.to_string());
                                next_queue.push(caller.to_string());
                            }
                        }
                    }
                }
            }
            queue = next_queue;
            depth += 1;
        }

        // 3. Affected modules / crates
        let mut affected_modules = HashSet::new();
        for p in direct.iter().chain(indirect.iter()).chain(std::iter::once(&file_path.to_string())) {
            if let Some(first_segment) = p.split('/').next() {
                affected_modules.insert(first_segment.to_string());
            }
        }

        // 4. Related tests
        let mut related_tests = HashSet::new();
        for p in self.arch.files.keys() {
            if p.contains("test") {
                if direct.contains(p) || indirect.contains(p) || p.contains(file_path) {
                    related_tests.insert(p.clone());
                }
            }
        }

        // 5. Relevant configs (Cargo.toml, tsconfig, etc.)
        let mut relevant_configs = Vec::new();
        for p in self.arch.files.keys() {
            if p.ends_with("Cargo.toml") || p.ends_with("package.json") || p.ends_with("tsconfig.json") {
                for module in &affected_modules {
                    if p.contains(module) {
                        relevant_configs.push(p.clone());
                    }
                }
            }
        }
        relevant_configs.sort();
        relevant_configs.dedup();

        // 6. Historical regressions from WorkMemory
        let mut regressions = Vec::new();
        if let Some(mem) = self.memory {
            for task in &mem.task_summaries {
                if task.files_modified.iter().any(|f| f == file_path || direct.contains(f)) {
                    for regr in &task.regressions_discovered {
                        regressions.push(format!("[{}] {}", task.task_id, regr));
                    }
                }
            }
        }

        // 7. Stale or unverified components among dependents
        let mut stale_or_unverified = Vec::new();
        if let Some(cov) = self.coverage {
            for dep in direct.iter().chain(indirect.iter()) {
                if let Some(rec) = cov.get_coverage(dep) {
                    if rec.is_stale || !rec.status.tested {
                        stale_or_unverified.push(dep.clone());
                    }
                } else {
                    stale_or_unverified.push(dep.clone());
                }
            }
        }

        // 8. Compute risk score
        // Leaf file with no dependents = ~0.1
        // Widely imported core utility = ~0.8-1.0
        let total_dependents = direct.len() + indirect.len();
        let risk_score = ((total_dependents as f32 * 0.15) + (regressions.len() as f32 * 0.2)).min(1.0);

        let mut direct_vec: Vec<_> = direct.into_iter().collect();
        direct_vec.sort();

        let mut indirect_vec: Vec<_> = indirect.into_iter().collect();
        indirect_vec.sort();

        let mut tests_vec: Vec<_> = related_tests.into_iter().collect();
        tests_vec.sort();

        ImpactReport {
            target_file: file_path.to_string(),
            target_symbol: symbol_name.map(|s| s.to_string()),
            direct_dependents: direct_vec,
            indirect_dependents: indirect_vec,
            affected_modules: affected_modules.into_iter().collect(),
            related_tests: tests_vec,
            relevant_configs,
            historical_regressions: regressions,
            stale_or_unverified_dependents: stale_or_unverified,
            risk_score,
        }
    }
}
