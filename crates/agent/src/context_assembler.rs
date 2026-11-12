use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use onyx_project::{
    ArchitectureMap, CoverageTracker, ImpactAnalyzer, ProjectPitfall, ProjectScanner, TaskSummary,
    WorkMemory,
};

/// Configuration for project context assembly.
#[derive(Debug, Clone)]
pub struct ContextBudgetConfig {
    /// Maximum estimated tokens allocated for project intelligence context.
    /// (Core instructions and security policies remain outside this budget).
    pub max_tokens: usize,
    /// Maximum number of relevant components/files to include in architecture tree.
    pub max_components: usize,
    /// Maximum number of relevant symbols to list.
    pub max_symbols: usize,
    /// Maximum number of past task summaries to inject.
    pub max_tasks: usize,
    /// Maximum number of architectural pitfalls to inject.
    pub max_pitfalls: usize,
}

impl Default for ContextBudgetConfig {
    fn default() -> Self {
        Self {
            max_tokens: 1500,
            max_components: 8,
            max_symbols: 16,
            max_tasks: 3,
            max_pitfalls: 3,
        }
    }
}

/// Statistics and telemetry from an assembly run.
#[derive(Debug, Clone, Default)]
pub struct AssemblyStats {
    pub estimated_tokens: usize,
    pub latency_us: u128,
    pub components_matched: usize,
    pub symbols_included: usize,
    pub tasks_included: usize,
    pub pitfalls_included: usize,
    pub stale_count: usize,
}

/// Task-aware context assembler that gathers relevant architecture, work memory,
/// and coverage evidence within a strict token budget.
pub struct ContextAssembler {
    pub workspace_root: PathBuf,
    pub config: ContextBudgetConfig,
    /// Cached architecture map to avoid disk reads on every step within a turn.
    cached_architecture: Mutex<Option<Arc<ArchitectureMap>>>,
    /// Cached coverage tracker.
    cached_coverage: Mutex<Option<Arc<CoverageTracker>>>,
    /// Cached work memory.
    cached_memory: Mutex<Option<Arc<WorkMemory>>>,
}

impl ContextAssembler {
    pub fn new<P: AsRef<Path>>(root: P, config: ContextBudgetConfig) -> Self {
        Self {
            workspace_root: root.as_ref().to_path_buf(),
            config,
            cached_architecture: Mutex::new(None),
            cached_coverage: Mutex::new(None),
            cached_memory: Mutex::new(None),
        }
    }

    /// Invalidate in-memory caches (e.g. after a tool modifies the repository).
    pub fn invalidate_cache(&self) {
        if let Ok(mut c) = self.cached_architecture.lock() {
            *c = None;
        }
        if let Ok(mut c) = self.cached_coverage.lock() {
            *c = None;
        }
        if let Ok(mut c) = self.cached_memory.lock() {
            *c = None;
        }
    }

    fn get_architecture(&self) -> Arc<ArchitectureMap> {
        let mut lock = self.cached_architecture.lock().unwrap();
        if let Some(arch) = &*lock {
            return Arc::clone(arch);
        }

        let scanner = ProjectScanner::new(&self.workspace_root);
        let arch = scanner.load_existing().unwrap_or_else(|| {
            ArchitectureMap::new(self.workspace_root.to_string_lossy().to_string())
        });
        let arc = Arc::new(arch);
        *lock = Some(Arc::clone(&arc));
        arc
    }

    fn get_coverage(&self) -> Arc<CoverageTracker> {
        let mut lock = self.cached_coverage.lock().unwrap();
        if let Some(cov) = &*lock {
            return Arc::clone(cov);
        }

        let cov = CoverageTracker::load_or_init(&self.workspace_root);
        let arc = Arc::new(cov);
        *lock = Some(Arc::clone(&arc));
        arc
    }

    pub fn get_memory(&self) -> Arc<WorkMemory> {
        let mut lock = self.cached_memory.lock().unwrap();
        if let Some(mem) = &*lock {
            return Arc::clone(mem);
        }

        let mem = WorkMemory::load_or_init(&self.workspace_root);
        let arc = Arc::new(mem);
        *lock = Some(Arc::clone(&arc));
        arc
    }

    pub fn get_arch_ref(&self) -> Arc<ArchitectureMap> {
        self.get_architecture()
    }

    /// Extract meaningful keywords and identifiers from a query string.
    fn extract_query_terms(&self, query: &str) -> HashSet<String> {
        let mut terms = HashSet::new();
        let cleaned: String = query
            .chars()
            .map(|c| if c.is_alphanumeric() || c == '_' || c == '-' { c } else { ' ' })
            .collect();

        for word in cleaned.split_whitespace() {
            let lower = word.to_lowercase();
            if lower.len() >= 3 && !is_stopword(&lower) {
                terms.insert(lower);
            }
        }
        terms
    }

    /// Assemble engineering context for the given user intent/task.
    /// Returns formatted markdown text and telemetry stats.
    pub fn assemble_context(&self, user_intent: &str) -> (String, AssemblyStats) {
        let start = Instant::now();
        let mut stats = AssemblyStats::default();

        let terms = self.extract_query_terms(user_intent);
        if terms.is_empty() {
            // No substantive keywords to ground on
            stats.latency_us = start.elapsed().as_micros();
            return (String::new(), stats);
        }

        let arch = self.get_architecture();
        let cov = self.get_coverage();
        let mem = self.get_memory();

        // 1. Rank & select relevant files from ArchitectureMap
        let mut matched_files: Vec<(&String, f32)> = Vec::new();
        for (rel_path, _file_meta) in &arch.files {
            let mut score = 0.0;
            let path_lower = rel_path.to_lowercase();
            for term in &terms {
                if path_lower.contains(term) {
                    score += 5.0;
                }
            }

            if score > 0.0 {
                matched_files.push((rel_path, score));
            }
        }

        // Also match symbols directly to lift their containing files
        let mut matched_symbols: Vec<String> = Vec::new();
        for sym in &arch.symbols {
            let name_lower = sym.name.to_lowercase();
            let mut sym_matched = false;
            for term in &terms {
                if name_lower.contains(term) {
                    sym_matched = true;
                    break;
                }
            }

            if sym_matched {
                if !matched_files.iter().any(|(p, _)| **p == sym.file) {
                    matched_files.push((&sym.file, 3.0));
                }
                if matched_symbols.len() < self.config.max_symbols {
                    let sig = sym.signature.as_deref().unwrap_or("");
                    matched_symbols.push(format!(
                        "{:?} `{}` (L{}) in `{}` {}",
                        sym.kind, sym.name, sym.line, sym.file, sig
                    ));
                }
            }
        }

        // Sort best match first
        matched_files.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        matched_files.truncate(self.config.max_components);
        stats.components_matched = matched_files.len();

        let relevant_paths: HashSet<String> = matched_files.iter().map(|(p, _)| (*p).clone()).collect();

        // 2. Collect dependency relationships among matched components
        let mut dep_notes: Vec<String> = Vec::new();
        for edge in &arch.dependencies {
            if relevant_paths.contains(&edge.from_file) || relevant_paths.contains(&edge.to_target) {
                dep_notes.push(format!("`{}` -> `{}` ({:?})", edge.from_file, edge.to_target, edge.kind));
            }
            if dep_notes.len() >= 6 {
                break;
            }
        }

        // Fill remaining symbols from matched files if not full
        if matched_symbols.len() < self.config.max_symbols {
            for sym in &arch.symbols {
                if relevant_paths.contains(&sym.file) && !matched_symbols.iter().any(|s| s.contains(&sym.name)) {
                    let sig = sym.signature.as_deref().unwrap_or("");
                    matched_symbols.push(format!(
                        "{:?} `{}` (L{}) in `{}` {}",
                        sym.kind, sym.name, sym.line, sym.file, sig
                    ));
                    if matched_symbols.len() >= self.config.max_symbols {
                        break;
                    }
                }
            }
        }
        stats.symbols_included = matched_symbols.len();

        // 3. Query relevant work memory (tasks and pitfalls)
        let mut relevant_tasks: Vec<&TaskSummary> = Vec::new();
        for task in &mem.task_summaries {
            let obj_lower = task.objective.to_lowercase();
            let matches_term = terms.iter().any(|t| obj_lower.contains(t));
            let matches_comp = task.components_involved.iter().any(|c| relevant_paths.iter().any(|p| p.contains(c)));
            let matches_file = task.files_modified.iter().any(|f| relevant_paths.contains(f));

            if matches_term || matches_comp || matches_file {
                relevant_tasks.push(task);
            }
        }
        // Recent tasks first
        relevant_tasks.reverse();
        relevant_tasks.truncate(self.config.max_tasks);
        stats.tasks_included = relevant_tasks.len();

        // Query pitfalls
        let mut relevant_pitfalls: Vec<&ProjectPitfall> = Vec::new();
        for pit in &mem.pitfalls {
            let title_lower = pit.title.to_lowercase();
            let desc_lower = pit.description.to_lowercase();
            let matches_comp = pit.affected_components.iter().any(|c| relevant_paths.iter().any(|p| p.contains(c)));
            let matches_kw = terms.iter().any(|t| title_lower.contains(t) || desc_lower.contains(t));

            if matches_comp || matches_kw {
                relevant_pitfalls.push(pit);
            }
        }
        relevant_pitfalls.truncate(self.config.max_pitfalls);
        stats.pitfalls_included = relevant_pitfalls.len();

        // 4. Build Investigation Coverage Status for relevant paths
        let mut coverage_lines: Vec<String> = Vec::new();
        for path in &relevant_paths {
            if let Some(record) = cov.get_coverage(path) {
                if record.is_stale {
                    stats.stale_count += 1;
                }
                let status_str = format_coverage_status(&record.status, record.is_stale);
                coverage_lines.push(format!("- `{}`: {}", path, status_str));
            } else {
                coverage_lines.push(format!("- `{}`: cataloged (uninspected)", path));
            }
        }

        // If no files, symbols, tasks, or pitfalls matched the query, fallback cleanly
        if matched_files.is_empty() && matched_symbols.is_empty() && relevant_tasks.is_empty() && relevant_pitfalls.is_empty() {
            stats.latency_us = start.elapsed().as_micros();
            return (String::new(), stats);
        }

        // 5. Assemble into structured markdown with token budgeting
        let mut output = String::new();
        output.push_str("### Project Intelligence & Architectural Context\n");

        if !coverage_lines.is_empty() {
            output.push_str("\n**Investigation Coverage:**\n");
            for line in coverage_lines {
                output.push_str(&line);
                output.push('\n');
            }
        }

        if !dep_notes.is_empty() {
            output.push_str("\n**Component Dependencies:**\n");
            for dep in dep_notes {
                output.push_str(&format!("- {}\n", dep));
            }
        }

        if !matched_symbols.is_empty() {
            output.push_str("\n**Key Architecture Symbols:**\n");
            for sym in matched_symbols {
                output.push_str(&format!("- {}\n", sym));
            }
        }

        if !relevant_tasks.is_empty() {
            output.push_str("\n**Past Work & Decisions:**\n");
            for t in relevant_tasks {
                let decs = if t.decisions.is_empty() {
                    String::new()
                } else {
                    format!(" | Decisions: {}", t.decisions.join("; "))
                };
                let regr = if t.regressions_discovered.is_empty() {
                    String::new()
                } else {
                    format!(" | Regressions noted: {}", t.regressions_discovered.join("; "))
                };
                output.push_str(&format!("- [{}] {}{}{}\n", t.task_id, t.objective, decs, regr));
            }
        }

        if !relevant_pitfalls.is_empty() {
            output.push_str("\n**Known Architectural Pitfalls:**\n");
            for p in relevant_pitfalls {
                output.push_str(&format!("- **{}**: {}\n", p.title, p.description));
            }
        }

        // 6. Impact Analysis & Blast Radius (for primary matched component)
        if let Some((primary_file, _)) = matched_files.first() {
            let impact_analyzer = ImpactAnalyzer::new(&arch)
                .with_coverage(&cov)
                .with_memory(&mem);
            let impact = impact_analyzer.analyze_impact(primary_file, None);

            if !impact.direct_dependents.is_empty() || !impact.related_tests.is_empty() {
                output.push_str(&format!(
                    "\n**Impact & Blast Radius (`{}`):**\n- Risk Score: {:.2} | Direct Dependents: {}\n",
                    primary_file, impact.risk_score, impact.direct_dependents.len()
                ));
                if !impact.direct_dependents.is_empty() {
                    let direct_sample: Vec<_> = impact.direct_dependents.iter().take(4).cloned().collect();
                    output.push_str(&format!("- Dependents to verify: {}\n", direct_sample.join(", ")));
                }
                if !impact.related_tests.is_empty() {
                    let tests_sample: Vec<_> = impact.related_tests.iter().take(3).cloned().collect();
                    output.push_str(&format!("- Targeted tests to run: {}\n", tests_sample.join(", ")));
                }
            }
        }

        // Enforce Token Budgeting (approx 4 chars per token)
        let est_tokens = output.len() / 4;
        stats.estimated_tokens = est_tokens;

        if est_tokens > self.config.max_tokens {
            let max_chars = self.config.max_tokens * 4;
            if output.len() > max_chars {
                output.truncate(max_chars);
                output.push_str("\n... [context truncated to stay within token budget]\n");
            }
        }

        stats.latency_us = start.elapsed().as_micros();
        (output, stats)
    }
}

/// Format the independent coverage booleans into a precise human-readable status.
fn format_coverage_status(status: &onyx_project::CoverageStatus, is_stale: bool) -> String {
    if is_stale {
        return "⚠️ STALE (source file changed on disk since last verification)".to_string();
    }

    let mut parts = Vec::new();
    if status.inspected {
        parts.push("inspected");
    } else {
        parts.push("not inspected");
    }

    if status.modified {
        parts.push("modified");
    }

    if status.tested {
        parts.push("tests passing");
    }

    if status.runtime_verified {
        parts.push("runtime verified");
    }

    parts.join(", ")
}

fn is_stopword(word: &str) -> bool {
    matches!(
        word,
        "the"
            | "and"
            | "for"
            | "that"
            | "this"
            | "with"
            | "from"
            | "what"
            | "when"
            | "where"
            | "which"
            | "who"
            | "why"
            | "how"
            | "all"
            | "any"
            | "both"
            | "each"
            | "few"
            | "more"
            | "most"
            | "some"
            | "such"
            | "than"
            | "too"
            | "very"
            | "can"
            | "will"
            | "just"
            | "should"
            | "now"
    )
}
