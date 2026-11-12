use std::fs::{self, File};
use std::io::{BufReader, Write};
use std::path::Path;
use serde::{Deserialize, Serialize};

/// Known pitfall or architectural hazard in the project.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectPitfall {
    pub id: String,
    pub title: String,
    pub description: String,
    pub affected_components: Vec<String>,
    pub discovered_at: u64,
}

/// Compact task summary capturing objectives, decisions, outcomes, and regressions.
/// Does NOT dump raw conversations or whole files.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskSummary {
    pub task_id: String,
    pub session_id: Option<String>,
    pub timestamp: u64,
    pub objective: String,
    pub components_involved: Vec<String>,
    pub files_modified: Vec<String>,
    pub decisions: Vec<String>,
    pub successful_changes: Vec<String>,
    pub tests_performed: Vec<String>,
    pub regressions_discovered: Vec<String>,
    pub unresolved_questions: Vec<String>,
}

impl TaskSummary {
    pub fn new(task_id: impl Into<String>, objective: impl Into<String>) -> Self {
        use std::time::{SystemTime, UNIX_EPOCH};
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        Self {
            task_id: task_id.into(),
            session_id: None,
            timestamp: now,
            objective: objective.into(),
            components_involved: Vec::new(),
            files_modified: Vec::new(),
            decisions: Vec::new(),
            successful_changes: Vec::new(),
            tests_performed: Vec::new(),
            regressions_discovered: Vec::new(),
            unresolved_questions: Vec::new(),
        }
    }
}

/// Persistent work memory storage for tasks, decisions, regressions, and pitfalls.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkMemory {
    pub task_summaries: Vec<TaskSummary>,
    pub pitfalls: Vec<ProjectPitfall>,
    pub last_updated_at: u64,
}

impl Default for WorkMemory {
    fn default() -> Self {
        Self {
            task_summaries: Vec::new(),
            pitfalls: Vec::new(),
            last_updated_at: 0,
        }
    }
}

impl WorkMemory {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a completed or milestone task summary.
    pub fn record_task_summary(&mut self, summary: TaskSummary) {
        use std::time::{SystemTime, UNIX_EPOCH};
        self.last_updated_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        // Avoid duplicate task_id if re-recorded
        if let Some(pos) = self.task_summaries.iter().position(|t| t.task_id == summary.task_id) {
            self.task_summaries[pos] = summary;
        } else {
            self.task_summaries.push(summary);
        }
    }

    /// Add a known project pitfall.
    pub fn record_pitfall(
        &mut self,
        title: impl Into<String>,
        description: impl Into<String>,
        affected_components: Vec<String>,
    ) {
        use std::time::{SystemTime, UNIX_EPOCH};
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let pitfall = ProjectPitfall {
            id: format!("pitfall-{}", self.pitfalls.len() + 1),
            title: title.into(),
            description: description.into(),
            affected_components,
            discovered_at: now,
        };

        self.pitfalls.push(pitfall);
    }

    /// Query the most recent N task summaries.
    pub fn get_recent_work(&self, limit: usize) -> Vec<&TaskSummary> {
        let count = self.task_summaries.len();
        if count == 0 {
            return Vec::new();
        }

        let start = count.saturating_sub(limit);
        self.task_summaries[start..count].iter().rev().collect()
    }

    /// Find relevant past work by component name or search terms.
    pub fn get_relevant_work(&self, query: &str) -> Vec<&TaskSummary> {
        let query_lower = query.to_lowercase();
        self.task_summaries
            .iter()
            .filter(|t| {
                t.objective.to_lowercase().contains(&query_lower)
                    || t.components_involved.iter().any(|c| c.to_lowercase().contains(&query_lower))
                    || t.files_modified.iter().any(|f| f.to_lowercase().contains(&query_lower))
                    || t.decisions.iter().any(|d| d.to_lowercase().contains(&query_lower))
            })
            .collect()
    }

    /// Load work memory from .onyx/project/work_memory.json
    pub fn load_or_init<P: AsRef<Path>>(root: P) -> Self {
        let path = root.as_ref().join(".onyx/project/work_memory.json");
        if !path.exists() {
            return Self::new();
        }

        match File::open(&path) {
            Ok(file) => {
                let reader = BufReader::new(file);
                serde_json::from_reader(reader).unwrap_or_default()
            }
            Err(_) => Self::new(),
        }
    }

    /// Atomically save work memory to .onyx/project/work_memory.json
    pub fn save_atomic<P: AsRef<Path>>(&mut self, root: P) -> std::io::Result<()> {
        use std::time::{SystemTime, UNIX_EPOCH};
        self.last_updated_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let dir = root.as_ref().join(".onyx/project");
        fs::create_dir_all(&dir)?;

        let target_file = dir.join("work_memory.json");
        let temp_file = dir.join(format!(".work_memory.json.tmp.{}", std::process::id()));

        let json = serde_json::to_string_pretty(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;

        let mut file = File::create(&temp_file)?;
        file.write_all(json.as_bytes())?;
        file.sync_all()?;
        drop(file);

        fs::rename(temp_file, target_file)?;
        Ok(())
    }
}
