use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs::{self, File};
use std::io::{BufReader, Write};
use std::path::{Path, PathBuf};
use sha2::{Digest, Sha256};

use onyx_project::{ArchitectureMap, CoverageTracker, ImpactAnalyzer, SymbolRetriever, WorkMemory};

/// Status of an autonomous engineering task step.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepStatus {
    Pending,
    InProgress,
    Completed,
    Failed,
    Skipped,
}

/// A planned step in an autonomous engineering task.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlannedStep {
    pub step_id: usize,
    pub description: String,
    pub target_component: Option<String>,
    pub required_tools: Vec<String>,
    pub status: StepStatus,
    pub outcome_summary: Option<String>,
}

/// An execution plan constructed by analyzing repository architecture,
/// dependencies, affected tests, and historical regressions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EngineeringPlan {
    pub task_id: String,
    pub objective: String,
    pub affected_crates: Vec<String>,
    pub affected_files: Vec<String>,
    pub direct_dependents: Vec<String>,
    pub targeted_tests: Vec<String>,
    pub potential_regressions: Vec<String>,
    pub steps: Vec<PlannedStep>,
    pub risk_score: f32,
    pub created_at: u64,
}

/// Diagnostic information parsed from a compiler or test failure.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiagnosticDiagnosis {
    pub tool_name: String,
    pub failed_command: String,
    pub error_file: Option<String>,
    pub error_line: Option<usize>,
    pub error_message: String,
    pub suggested_remedy: String,
    pub retry_count: usize,
}

/// Persistent checkpoint of an in-flight autonomous task for process termination
/// and context exhaustion recovery under `.onyx/checkpoints/{task_id}.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskCheckpoint {
    pub task_id: String,
    pub objective: String,
    pub plan: EngineeringPlan,
    pub completed_steps: Vec<usize>,
    pub remaining_steps: Vec<usize>,
    pub modified_files: Vec<String>,
    pub verified_files: Vec<String>,
    pub file_hashes: std::collections::HashMap<String, String>,
    pub verification_results: Vec<String>,
    pub current_diagnostics: Option<DiagnosticDiagnosis>,
    pub session_id: Option<String>,
    pub last_updated_at: u64,
}

impl TaskCheckpoint {
    pub fn new(task_id: impl Into<String>, objective: impl Into<String>, plan: EngineeringPlan) -> Self {
        use std::time::{SystemTime, UNIX_EPOCH};
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let remaining = plan.steps.iter().map(|s| s.step_id).collect();

        Self {
            task_id: task_id.into(),
            objective: objective.into(),
            plan,
            completed_steps: Vec::new(),
            remaining_steps: remaining,
            modified_files: Vec::new(),
            verified_files: Vec::new(),
            file_hashes: std::collections::HashMap::new(),
            verification_results: Vec::new(),
            current_diagnostics: None,
            session_id: None,
            last_updated_at: now,
        }
    }

    /// Record file hash for a modified file to detect tampering or staleness.
    pub fn record_file_hash<P: AsRef<Path>>(&mut self, root: P, rel_path: &str) {
        let full = root.as_ref().join(rel_path);
        if let Ok(bytes) = fs::read(&full) {
            let mut hasher = Sha256::new();
            hasher.update(&bytes);
            let hash = hex::encode(hasher.finalize());
            self.file_hashes.insert(rel_path.to_string(), hash);
        }
    }

    /// Validate modified files against recorded hashes. Returns list of modified/stale files.
    pub fn validate_file_hashes<P: AsRef<Path>>(&self, root: P) -> (Vec<String>, Vec<String>) {
        let mut unchanged = Vec::new();
        let mut changed_or_missing = Vec::new();

        for (file, stored_hash) in &self.file_hashes {
            let full = root.as_ref().join(file);
            match fs::read(&full) {
                Ok(bytes) => {
                    let mut hasher = Sha256::new();
                    hasher.update(&bytes);
                    let cur_hash = hex::encode(hasher.finalize());
                    if &cur_hash == stored_hash {
                        unchanged.push(file.clone());
                    } else {
                        changed_or_missing.push(file.clone());
                    }
                }
                Err(_) => {
                    changed_or_missing.push(file.clone());
                }
            }
        }

        (unchanged, changed_or_missing)
    }

    /// Save checkpoint atomically to `.onyx/checkpoints/{task_id}.json`.
    pub fn save_atomic<P: AsRef<Path>>(&mut self, root: P) -> std::io::Result<()> {
        use std::time::{SystemTime, UNIX_EPOCH};
        self.last_updated_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let dir = root.as_ref().join(".onyx/checkpoints");
        fs::create_dir_all(&dir)?;

        let target = dir.join(format!("{}.json", self.task_id));
        let tmp = dir.join(format!(".{}.tmp.{}", self.task_id, std::process::id()));

        let json = serde_json::to_string_pretty(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;

        let mut file = File::create(&tmp)?;
        file.write_all(json.as_bytes())?;
        file.sync_all()?;
        drop(file);

        fs::rename(tmp, target)?;
        Ok(())
    }

    /// Load checkpoint from `.onyx/checkpoints/{task_id}.json`.
    pub fn load_checkpoint<P: AsRef<Path>>(root: P, task_id: &str) -> Option<Self> {
        let path = root.as_ref().join(".onyx/checkpoints").join(format!("{task_id}.json"));
        if !path.exists() {
            return None;
        }

        let file = File::open(path).ok()?;
        let reader = BufReader::new(file);
        serde_json::from_reader(reader).ok()
    }
}

/// The Autonomous Engineering Planner & Diagnostic Engine.
pub struct AutonomousEngine<'a> {
    pub arch: &'a ArchitectureMap,
    pub coverage: Option<&'a CoverageTracker>,
    pub memory: Option<&'a WorkMemory>,
    pub workspace_root: PathBuf,
}

impl<'a> AutonomousEngine<'a> {
    pub fn new(arch: &'a ArchitectureMap, workspace_root: impl AsRef<Path>) -> Self {
        Self {
            arch,
            coverage: None,
            memory: None,
            workspace_root: workspace_root.as_ref().to_path_buf(),
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

    /// 6.1: Architecture-Aware Task Planning & 6.2: Pre-Edit Impact Analysis
    pub fn plan_task(&self, task_id: &str, objective: &str) -> EngineeringPlan {
        use std::time::{SystemTime, UNIX_EPOCH};
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        // 1. Identify candidate files and symbols matching objective
        let retriever = SymbolRetriever::new(self.arch);
        let symbols = retriever.retrieve_symbols(objective, 8);

        let mut affected_files = HashSet::new();
        for sym in &symbols {
            affected_files.insert(sym.file.clone());
        }

        // Also check if any file paths directly match objective keywords
        let terms: Vec<String> = objective
            .split_whitespace()
            .filter(|w| w.len() >= 3)
            .map(|w| w.to_lowercase())
            .collect();

        for file in self.arch.files.keys() {
            let f_lower = file.to_lowercase();
            if terms.iter().any(|t| f_lower.contains(t)) {
                affected_files.insert(file.clone());
            }
        }

        // 2. Pre-Edit Impact Analysis across affected components
        let mut direct_dependents = HashSet::new();
        let mut targeted_tests = HashSet::new();
        let mut affected_crates = HashSet::new();
        let mut max_risk: f32 = 0.1;

        let impact_analyzer = ImpactAnalyzer::new(self.arch);
        for file in &affected_files {
            let report = impact_analyzer.analyze_impact(file, None);
            if report.risk_score > max_risk {
                max_risk = report.risk_score;
            }
            for dep in report.direct_dependents {
                direct_dependents.insert(dep);
            }
            for test in report.related_tests {
                targeted_tests.insert(test);
            }
            for module in report.affected_modules {
                affected_crates.insert(module);
            }
        }

        // 3. Historical Regressions from WorkMemory
        let mut regressions = Vec::new();
        if let Some(mem) = self.memory {
            for task in &mem.task_summaries {
                if task.files_modified.iter().any(|f| affected_files.contains(f)) {
                    for r in &task.regressions_discovered {
                        regressions.push(format!("[{}] {}", task.task_id, r));
                    }
                }
            }
        }

        // 4. Synthesize Step Sequence
        let mut steps = Vec::new();
        let mut step_num = 1;

        // Step 1: Pre-inspection & impact verification
        steps.push(PlannedStep {
            step_id: step_num,
            description: format!("Inspect target component and verify dependencies for {}", objective),
            target_component: affected_files.iter().next().cloned(),
            required_tools: vec!["read_file".into()],
            status: StepStatus::Pending,
            outcome_summary: None,
        });
        step_num += 1;

        // Step 2: Implementation of edits
        steps.push(PlannedStep {
            step_id: step_num,
            description: "Apply targeted code modifications".into(),
            target_component: affected_files.iter().next().cloned(),
            required_tools: vec!["write_file".into(), "editor".into()],
            status: StepStatus::Pending,
            outcome_summary: None,
        });
        step_num += 1;

        // Step 3: Targeted compilation check
        let crate_check = affected_crates.iter().next().cloned().unwrap_or_else(|| "workspace".into());
        steps.push(PlannedStep {
            step_id: step_num,
            description: format!("Targeted compilation check: `cargo check -p {}`", crate_check),
            target_component: Some(crate_check.clone()),
            required_tools: vec!["bash".into()],
            status: StepStatus::Pending,
            outcome_summary: None,
        });
        step_num += 1;

        // Step 4: Run targeted unit & integration tests
        let test_target = targeted_tests.iter().next().cloned().unwrap_or_else(|| format!("cargo test -p {}", crate_check));
        steps.push(PlannedStep {
            step_id: step_num,
            description: format!("Run targeted verification tests: {}", test_target),
            target_component: Some(test_target),
            required_tools: vec!["bash".into()],
            status: StepStatus::Pending,
            outcome_summary: None,
        });

        let mut files_vec: Vec<_> = affected_files.into_iter().collect();
        files_vec.sort();

        let mut direct_vec: Vec<_> = direct_dependents.into_iter().collect();
        direct_vec.sort();

        let mut tests_vec: Vec<_> = targeted_tests.into_iter().collect();
        tests_vec.sort();

        EngineeringPlan {
            task_id: task_id.to_string(),
            objective: objective.to_string(),
            affected_crates: affected_crates.into_iter().collect(),
            affected_files: files_vec,
            direct_dependents: direct_vec,
            targeted_tests: tests_vec,
            potential_regressions: regressions,
            steps,
            risk_score: max_risk,
            created_at: now,
        }
    }

    /// 6.3: Automatic Test Selection
    pub fn select_tests_for_changes(&self, changed_files: &[String]) -> Vec<String> {
        let mut tests = HashSet::new();
        let impact_analyzer = ImpactAnalyzer::new(self.arch);

        for file in changed_files {
            let report = impact_analyzer.analyze_impact(file, None);
            for t in report.related_tests {
                tests.insert(t);
            }
        }

        // If no test files matched directly, fallback to crate-level test command
        if tests.is_empty() {
            for file in changed_files {
                if let Some(crate_name) = file.split('/').next() {
                    tests.insert(format!("cargo test -p {crate_name} --lib"));
                }
            }
        }

        let mut list: Vec<_> = tests.into_iter().collect();
        list.sort();
        list
    }

    /// 6.4: Self-Correction & Compiler-Error Diagnostic Reasoning
    pub fn diagnose_failure(&self, command: &str, output: &str, retry_count: usize) -> DiagnosticDiagnosis {
        self.diagnose_failure_with_observation_and_focus(command, output, retry_count, None, None)
    }

    /// Diagnose a failure using the observation already produced by the command
    /// output processor. The legacy entrypoint above remains available for
    /// callers that only have raw command output.
    pub fn diagnose_failure_with_observation(
        &self,
        command: &str,
        output: &str,
        retry_count: usize,
        observation: Option<&tools::compress::CommandObservation>,
    ) -> DiagnosticDiagnosis {
        self.diagnose_failure_with_observation_and_focus(command, output, retry_count, observation, None)
    }

    /// Prefer diagnostics in files affected by the active task while retaining
    /// the complete observation for callers that need all failures.
    pub fn diagnose_failure_with_observation_and_focus(
        &self,
        command: &str,
        output: &str,
        retry_count: usize,
        observation: Option<&tools::compress::CommandObservation>,
        focus_files: Option<&[String]>,
    ) -> DiagnosticDiagnosis {
        let mut error_file = None;
        let mut error_line = None;
        let mut error_msg = String::new();

        // Check for permission or policy denials first — never suggest retrying denied operations
        if output.contains("DENIED") || output.contains("denied by policy") || output.contains("Permission denied") {
            return DiagnosticDiagnosis {
                tool_name: "bash".into(),
                failed_command: command.into(),
                error_file: None,
                error_line: None,
                error_message: "Permission / policy denied: operation is restricted by security policy.".to_string(),
                suggested_remedy: "Do NOT retry this command. Propose an alternate approach within user permissions or request approval.".to_string(),
                retry_count,
            };
        }

        // Check for command timeouts
        if output.contains("timed out") || output.contains("SIGKILL") || output.contains("killed after") {
            return DiagnosticDiagnosis {
                tool_name: "bash".into(),
                failed_command: command.into(),
                error_file: None,
                error_line: None,
                error_message: "Execution timed out.".to_string(),
                suggested_remedy: "The command exceeded execution limits. Run a more targeted or incremental check (e.g. `--lib` or `-p <crate>`).".to_string(),
                retry_count,
            };
        }

        // Reuse the unified observation when the command tool already parsed it.
        let owned_obs;
        let obs = if let Some(obs) = observation {
            obs
        } else {
            owned_obs = tools::compress::process_observation(command, output, None).0;
            &owned_obs
        };
        let focused_error = focus_files.and_then(|files| {
            obs.errors.iter().find(|err| {
                err.file
                    .as_deref()
                    .map(|path| files.iter().any(|focus| path.contains(focus) || focus.contains(path)))
                    .unwrap_or(false)
            })
        });
        if let Some(err) = focused_error.or_else(|| obs.errors.first()) {
            error_msg = if let Some(code) = &err.code {
                format!("error[{}]: {}", code, err.message)
            } else {
                format!("error: {}", err.message)
            };
            error_file = err.file.clone();
            error_line = err.line;
        } else if let Some(first_fail) = obs.failed_tests.first() {
            error_msg = format!("Test failed: {first_fail}");
        }

        // Fallback line scanner if observation found no structured error
        if error_msg.is_empty() {
            for line in output.lines() {
                // Rustc error pattern: "error[E0...]: ... --> src/foo.rs:12:5"
                if line.contains("error[") || line.contains("error:") {
                    if error_msg.is_empty() || line.contains("error[") {
                        error_msg = line.to_string();
                    }
                }
                if let Some(arrow_idx) = line.find("--> ") {
                    if error_file.is_none() {
                        let loc = &line[arrow_idx + 4..];
                        let parts: Vec<&str> = loc.trim().split(':').collect();
                        if parts.len() >= 2 {
                            error_file = Some(parts[0].to_string());
                            error_line = parts[1].parse().ok();
                        }
                    }
                }
            }
        }

        if error_msg.is_empty() {
            error_msg = output.lines().take(3).collect::<Vec<_>>().join(" ");
        }

        // Formulate remedy hint based on diagnostic signature
        let remedy = if error_msg.contains("cannot find value") || error_msg.contains("unresolved import") || error_msg.contains("E0432") || error_msg.contains("E0425") {
            "Missing import or symbol in scope. Check use statements or crate dependencies.".to_string()
        } else if error_msg.contains("mismatched types") || error_msg.contains("E0308") {
            "Type mismatch. Check signature expectations and adjust argument or return type.".to_string()
        } else if error_msg.contains("borrow of moved value") || error_msg.contains("cannot borrow") || error_msg.contains("E0382") || error_msg.contains("E0502") {
            "Borrow-checker conflict. Consider borrowing (`&`), cloning, or scoping the borrow guard.".to_string()
        } else if error_msg.contains("expected one of") || error_msg.contains("syntax error") {
            "Compilation syntax error. Verify matching delimiters, semicolons, and macro arguments.".to_string()
        } else if error_msg.contains("test failed") || error_msg.contains("FAILED") {
            "Test assertion failure. Inspect test inputs and target code logic.".to_string()
        } else {
            format!("Analyze failure location in {:?}:{} and adjust patch.", error_file, error_line.unwrap_or(0))
        };

        DiagnosticDiagnosis {
            tool_name: "bash".into(),
            failed_command: command.into(),
            error_file,
            error_line,
            error_message: error_msg,
            suggested_remedy: remedy,
            retry_count,
        }
    }
}

/// Detect whether user query represents a genuine engineering task (refactor, fix, add, test, implement)
/// or an ordinary conversational greeting/question.
pub fn is_engineering_task(user_query: &str) -> bool {
    let lower = user_query.to_lowercase();
    let keywords = [
        "refactor", "implement", "fix", "bug", "error", "test", "add", "remove", "update",
        "create", "edit", "modify", "patch", "cargo", "build", "compile", "check", "function",
        "struct", "module", "crate", "file", "feature", "code", "remedy", "repair",
    ];

    keywords.iter().any(|&k| lower.contains(k))
}
