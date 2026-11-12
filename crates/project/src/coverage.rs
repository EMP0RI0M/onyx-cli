use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{BufReader, Write};
use std::path::Path;
use serde::{Deserialize, Serialize};

use crate::events::{CoverageEvent, EvidenceSource, VerificationScope, VerificationStatus};

/// Independent investigation / verification statuses.
/// These states are NOT a simple linear progression.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub struct CoverageStatus {
    pub cataloged: bool,
    pub inspected: bool,
    pub modified: bool,
    pub tested: bool,
    pub runtime_verified: bool,
}

impl Default for CoverageStatus {
    fn default() -> Self {
        Self {
            cataloged: true,
            inspected: false,
            modified: false,
            tested: false,
            runtime_verified: false,
        }
    }
}

/// An evidence entry representing an inspection, modification, or verification pass.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvidenceRecord {
    pub timestamp: u64,
    pub source: EvidenceSource,
    pub session_id: Option<String>,
    pub status: VerificationStatus,
    pub scope: VerificationScope,
    pub content_hash_at_event: Option<String>,
    pub command: Option<String>,
    pub details: Option<String>,
}

/// Persistent coverage record for a specific file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileCoverageRecord {
    pub relative_path: String,
    pub last_known_hash: String,
    pub status: CoverageStatus,
    /// Whether the evidence collected is now stale because the file hash on disk has changed.
    pub is_stale: bool,
    /// When any evidence was last recorded for this file.
    pub updated_at: u64,
    /// Chronological history of evidence for this file.
    pub evidence_history: Vec<EvidenceRecord>,
}

impl FileCoverageRecord {
    pub fn new(relative_path: String, hash: String) -> Self {
        use std::time::{SystemTime, UNIX_EPOCH};
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        Self {
            relative_path,
            last_known_hash: hash,
            status: CoverageStatus::default(),
            is_stale: false,
            updated_at: now,
            evidence_history: Vec::new(),
        }
    }

    /// Add an evidence entry and update coverage states accordingly.
    pub fn add_evidence(&mut self, record: EvidenceRecord) {
        self.updated_at = record.timestamp;
        
        match record.source {
            EvidenceSource::FileRead => {
                self.status.inspected = true;
                if let Some(h) = &record.content_hash_at_event {
                    self.last_known_hash = h.clone();
                    self.is_stale = false;
                }
            }
            EvidenceSource::FileModify => {
                self.status.modified = true;
                // Modifying a file invalidates prior tested/verified claims for this new version
                self.status.tested = false;
                self.status.runtime_verified = false;
                if let Some(h) = &record.content_hash_at_event {
                    self.last_known_hash = h.clone();
                    self.is_stale = false;
                }
            }
            EvidenceSource::TestExecution => {
                if record.status == VerificationStatus::Passed {
                    self.status.tested = true;
                }
            }
            EvidenceSource::RuntimeVerification => {
                if record.status == VerificationStatus::Passed {
                    self.status.runtime_verified = true;
                }
            }
            EvidenceSource::BuildVerification => {
                // Compiles successfully
            }
            EvidenceSource::SearchHit => {
                // IMPORTANT: Search hits NEVER qualify as inspected!
            }
            EvidenceSource::ManualVerification => {
                if record.status == VerificationStatus::Passed {
                    self.status.runtime_verified = true;
                }
            }
        }

        self.evidence_history.push(record);
    }

    /// Mark as stale if current disk hash differs from the hash when evidence was gathered.
    pub fn check_stale(&mut self, current_hash: &str) {
        if self.last_known_hash != current_hash && !self.last_known_hash.is_empty() {
            self.is_stale = true;
        } else if self.last_known_hash == current_hash {
            self.is_stale = false;
        }
    }
}

/// The persistent coverage tracker.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoverageTracker {
    pub files: HashMap<String, FileCoverageRecord>,
    pub last_saved_at: u64,
}

impl Default for CoverageTracker {
    fn default() -> Self {
        Self {
            files: HashMap::new(),
            last_saved_at: 0,
        }
    }
}

impl CoverageTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register cataloged files discovered during repository scanning.
    pub fn sync_cataloged_files(&mut self, catalog: &HashMap<String, String>) {
        for (rel_path, hash) in catalog {
            let entry = self
                .files
                .entry(rel_path.clone())
                .or_insert_with(|| FileCoverageRecord::new(rel_path.clone(), hash.clone()));

            entry.status.cataloged = true;
            if entry.last_known_hash.is_empty() {
                entry.last_known_hash = hash.clone();
            } else if entry.last_known_hash != *hash {
                // File modified externally since last check
                entry.check_stale(hash);
            }
        }
    }

    /// Record a file inspection event (e.g. read_file).
    pub fn record_inspection(
        &mut self,
        rel_path: &str,
        current_hash: Option<String>,
        session_id: Option<String>,
        details: Option<String>,
    ) {
        use std::time::{SystemTime, UNIX_EPOCH};
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let hash_val = current_hash.unwrap_or_default();
        let record = self
            .files
            .entry(rel_path.to_string())
            .or_insert_with(|| FileCoverageRecord::new(rel_path.to_string(), hash_val.clone()));

        record.add_evidence(EvidenceRecord {
            timestamp: now,
            source: EvidenceSource::FileRead,
            session_id,
            status: VerificationStatus::Passed,
            scope: VerificationScope::File,
            content_hash_at_event: if hash_val.is_empty() { None } else { Some(hash_val) },
            command: None,
            details,
        });
    }

    /// Record a file modification event (e.g. editor, write_file).
    pub fn record_modification(
        &mut self,
        rel_path: &str,
        new_hash: Option<String>,
        session_id: Option<String>,
        details: Option<String>,
    ) {
        use std::time::{SystemTime, UNIX_EPOCH};
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let hash_val = new_hash.unwrap_or_default();
        let record = self
            .files
            .entry(rel_path.to_string())
            .or_insert_with(|| FileCoverageRecord::new(rel_path.to_string(), hash_val.clone()));

        record.add_evidence(EvidenceRecord {
            timestamp: now,
            source: EvidenceSource::FileModify,
            session_id,
            status: VerificationStatus::Passed,
            scope: VerificationScope::File,
            content_hash_at_event: if hash_val.is_empty() { None } else { Some(hash_val) },
            command: None,
            details,
        });
    }

    /// Record a search appearance (grep, file search).
    /// Does NOT mark the file as inspected.
    pub fn record_search_hit(
        &mut self,
        rel_path: &str,
        session_id: Option<String>,
        query: &str,
    ) {
        use std::time::{SystemTime, UNIX_EPOCH};
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let record = self
            .files
            .entry(rel_path.to_string())
            .or_insert_with(|| FileCoverageRecord::new(rel_path.to_string(), String::new()));

        record.add_evidence(EvidenceRecord {
            timestamp: now,
            source: EvidenceSource::SearchHit,
            session_id,
            status: VerificationStatus::Passed,
            scope: VerificationScope::File,
            content_hash_at_event: None,
            command: None,
            details: Some(format!("Search query: {}", query)),
        });
    }

    /// Record test verification outcome across affected files.
    pub fn record_test_run(
        &mut self,
        command: &str,
        test_name: Option<&str>,
        passed: bool,
        affected_files: &[String],
        session_id: Option<String>,
        scope: VerificationScope,
        details: Option<String>,
    ) {
        use std::time::{SystemTime, UNIX_EPOCH};
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let status = if passed {
            VerificationStatus::Passed
        } else {
            VerificationStatus::Failed
        };

        for path in affected_files {
            let record = self
                .files
                .entry(path.clone())
                .or_insert_with(|| FileCoverageRecord::new(path.clone(), String::new()));

            record.add_evidence(EvidenceRecord {
                timestamp: now,
                source: EvidenceSource::TestExecution,
                session_id: session_id.clone(),
                status: status.clone(),
                scope: scope.clone(),
                content_hash_at_event: if record.last_known_hash.is_empty() {
                    None
                } else {
                    Some(record.last_known_hash.clone())
                },
                command: Some(command.to_string()),
                details: details.clone().or_else(|| test_name.map(|t| format!("Test: {}", t))),
            });
        }
    }

    /// Record runtime verification (e.g. running binary or smoke tests).
    pub fn record_runtime_verification(
        &mut self,
        command: &str,
        passed: bool,
        affected_files: &[String],
        session_id: Option<String>,
        scope: VerificationScope,
        details: Option<String>,
    ) {
        use std::time::{SystemTime, UNIX_EPOCH};
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let status = if passed {
            VerificationStatus::Passed
        } else {
            VerificationStatus::Failed
        };

        for path in affected_files {
            let record = self
                .files
                .entry(path.clone())
                .or_insert_with(|| FileCoverageRecord::new(path.clone(), String::new()));

            record.add_evidence(EvidenceRecord {
                timestamp: now,
                source: EvidenceSource::RuntimeVerification,
                session_id: session_id.clone(),
                status: status.clone(),
                scope: scope.clone(),
                content_hash_at_event: if record.last_known_hash.is_empty() {
                    None
                } else {
                    Some(record.last_known_hash.clone())
                },
                command: Some(command.to_string()),
                details: details.clone(),
            });
        }
    }

    /// Process a generic CoverageEvent and apply it.
    pub fn apply_event(&mut self, event: CoverageEvent) {
        if let Some(path) = &event.relative_path {
            let record = self
                .files
                .entry(path.clone())
                .or_insert_with(|| FileCoverageRecord::new(path.clone(), event.content_hash.clone().unwrap_or_default()));

            record.add_evidence(EvidenceRecord {
                timestamp: event.timestamp,
                source: event.source,
                session_id: event.session_id,
                status: event.status,
                scope: event.scope,
                content_hash_at_event: event.content_hash,
                command: event.command,
                details: event.details,
            });
        }
    }

    /// Check against fresh repository hashes to mark stale records.
    pub fn check_stale_against_hashes(&mut self, current_hashes: &HashMap<String, String>) {
        for (path, record) in self.files.iter_mut() {
            if let Some(curr) = current_hashes.get(path) {
                record.check_stale(curr);
            }
        }
    }

    /// Query coverage for a specific file.
    pub fn get_coverage(&self, rel_path: &str) -> Option<&FileCoverageRecord> {
        self.files.get(rel_path)
    }

    /// Return unexplored or uninspected components/files up to a limit.
    pub fn get_unexplored_components(&self, limit: usize) -> Vec<String> {
        let mut unexplored: Vec<String> = self
            .files
            .iter()
            .filter(|(_, rec)| rec.status.cataloged && !rec.status.inspected)
            .map(|(path, _)| path.clone())
            .collect();

        unexplored.sort();
        unexplored.truncate(limit);
        unexplored
    }

    /// Return summary metrics.
    pub fn summary(&self) -> CoverageSummary {
        let mut cataloged = 0;
        let mut inspected = 0;
        let mut modified = 0;
        let mut tested = 0;
        let mut runtime_verified = 0;
        let mut stale = 0;

        for rec in self.files.values() {
            if rec.status.cataloged {
                cataloged += 1;
            }
            if rec.status.inspected {
                inspected += 1;
            }
            if rec.status.modified {
                modified += 1;
            }
            if rec.status.tested {
                tested += 1;
            }
            if rec.status.runtime_verified {
                runtime_verified += 1;
            }
            if rec.is_stale {
                stale += 1;
            }
        }

        CoverageSummary {
            total_files: self.files.len(),
            cataloged,
            inspected,
            modified,
            tested,
            runtime_verified,
            stale,
        }
    }

    /// Load coverage tracker from .onyx/project/coverage.json
    pub fn load_or_init<P: AsRef<Path>>(root: P) -> Self {
        let path = root.as_ref().join(".onyx/project/coverage.json");
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

    /// Atomically save coverage tracker to .onyx/project/coverage.json
    pub fn save_atomic<P: AsRef<Path>>(&mut self, root: P) -> std::io::Result<()> {
        use std::time::{SystemTime, UNIX_EPOCH};
        self.last_saved_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let dir = root.as_ref().join(".onyx/project");
        fs::create_dir_all(&dir)?;

        let target_file = dir.join("coverage.json");
        let temp_file = dir.join(format!(".coverage.json.tmp.{}", std::process::id()));

        let json = serde_json::to_string_pretty(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;

        let mut file = File::create(&temp_file)?;
        file.write_all(json.as_bytes())?;
        file.sync_all()?;
        drop(file);

        fs::rename(temp_file, target_file)?;
        Ok(())
    }

    /// Append a single event log to .onyx/project/events/
    pub fn log_event_atomic<P: AsRef<Path>>(root: P, event: &CoverageEvent) -> std::io::Result<()> {
        let events_dir = root.as_ref().join(".onyx/project/events");
        fs::create_dir_all(&events_dir)?;

        let filename = format!("{}-{}.json", event.timestamp, event.id);
        let target_file = events_dir.join(&filename);
        let temp_file = events_dir.join(format!(".{}.tmp", filename));

        let json = serde_json::to_string_pretty(event)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;

        let mut file = File::create(&temp_file)?;
        file.write_all(json.as_bytes())?;
        file.sync_all()?;
        drop(file);

        fs::rename(temp_file, target_file)?;
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoverageSummary {
    pub total_files: usize,
    pub cataloged: usize,
    pub inspected: usize,
    pub modified: usize,
    pub tested: usize,
    pub runtime_verified: usize,
    pub stale: usize,
}
