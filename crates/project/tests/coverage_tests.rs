use std::collections::HashMap;
use tempfile::tempdir;

use onyx_project::{
    CoverageTracker, EvidenceSource, TaskSummary, VerificationScope, VerificationStatus,
    WorkMemory,
};

#[test]
fn test_inspection_evidence() {
    let mut tracker = CoverageTracker::new();
    tracker.record_inspection(
        "src/main.rs",
        Some("hash_111".to_string()),
        Some("session_abc".to_string()),
        Some("Inspected main entrypoint".to_string()),
    );

    let rec = tracker.get_coverage("src/main.rs").expect("record exists");
    assert!(rec.status.inspected);
    assert!(!rec.status.modified);
    assert!(!rec.status.tested);
    assert!(!rec.status.runtime_verified);
    assert_eq!(rec.last_known_hash, "hash_111");
    assert_eq!(rec.evidence_history.len(), 1);
    assert_eq!(rec.evidence_history[0].source, EvidenceSource::FileRead);
}

#[test]
fn test_search_hit_does_not_count_as_inspection() {
    let mut tracker = CoverageTracker::new();
    tracker.record_search_hit("src/lib.rs", Some("session_1".to_string()), "fn run");

    let rec = tracker.get_coverage("src/lib.rs").expect("record exists");
    assert!(!rec.status.inspected, "Search hit must NEVER count as inspected!");
    assert_eq!(rec.evidence_history.len(), 1);
    assert_eq!(rec.evidence_history[0].source, EvidenceSource::SearchHit);
}

#[test]
fn test_modification_invalidates_prior_test_and_runtime_verification() {
    let mut tracker = CoverageTracker::new();
    let file = "crates/agent/src/lib.rs".to_string();

    // 1. Inspect
    tracker.record_inspection(&file, Some("hash_v1".to_string()), None, None);
    // 2. Test passes
    tracker.record_test_run(
        "cargo test -p onyx-agent",
        Some("test_discovery"),
        true,
        &[file.clone()],
        None,
        VerificationScope::Package("onyx-agent".into()),
        None,
    );

    let rec = tracker.get_coverage(&file).unwrap();
    assert!(rec.status.inspected);
    assert!(rec.status.tested);

    // 3. File modified -> should reset tested and runtime_verified!
    tracker.record_modification(&file, Some("hash_v2".to_string()), None, None);
    let rec_after = tracker.get_coverage(&file).unwrap();
    assert!(rec_after.status.modified);
    assert!(!rec_after.status.tested, "Modification must reset tested state for new hash");
    assert!(!rec_after.status.runtime_verified);
    assert_eq!(rec_after.last_known_hash, "hash_v2");
}

#[test]
fn test_failed_test_evidence() {
    let mut tracker = CoverageTracker::new();
    let file = "src/calc.rs".to_string();

    tracker.record_test_run(
        "cargo test",
        Some("test_calc_div_zero"),
        false,
        &[file.clone()],
        None,
        VerificationScope::File,
        Some("assertion failed: 1 == 0".to_string()),
    );

    let rec = tracker.get_coverage(&file).unwrap();
    assert!(!rec.status.tested, "Failed test must NOT set status.tested to true");
    assert_eq!(rec.evidence_history.len(), 1);
    assert_eq!(rec.evidence_history[0].status, VerificationStatus::Failed);
}

#[test]
fn test_runtime_verification_and_scope() {
    let mut tracker = CoverageTracker::new();
    let file = "src/server.rs".to_string();

    tracker.record_runtime_verification(
        "./target/release/server --port 8080",
        true,
        &[file.clone()],
        Some("sess_runtime".to_string()),
        VerificationScope::TargetedTest("smoke_probe".into()),
        Some("HTTP 200 OK received".to_string()),
    );

    let rec = tracker.get_coverage(&file).unwrap();
    assert!(rec.status.runtime_verified);
    assert_eq!(rec.evidence_history[0].source, EvidenceSource::RuntimeVerification);
}

#[test]
fn test_stale_hash_detection_and_preservation() {
    let mut tracker = CoverageTracker::new();
    let file = "src/model.rs";

    tracker.record_inspection(file, Some("hash_initial".to_string()), None, None);
    assert!(!tracker.get_coverage(file).unwrap().is_stale);

    // External change detected: hash changed on disk
    let mut fresh_hashes = HashMap::new();
    fresh_hashes.insert(file.to_string(), "hash_changed_by_git".to_string());

    tracker.check_stale_against_hashes(&fresh_hashes);
    let rec = tracker.get_coverage(file).unwrap();
    assert!(rec.is_stale, "Record should be flagged as stale");
    // Verify historical evidence was preserved
    assert_eq!(rec.evidence_history.len(), 1);
    assert_eq!(rec.last_known_hash, "hash_initial");
}

#[test]
fn test_unexplored_components_query() {
    let mut tracker = CoverageTracker::new();
    let mut catalog = HashMap::new();
    catalog.insert("src/a.rs".to_string(), "hash_a".to_string());
    catalog.insert("src/b.rs".to_string(), "hash_b".to_string());
    catalog.insert("src/c.rs".to_string(), "hash_c".to_string());

    tracker.sync_cataloged_files(&catalog);
    assert_eq!(tracker.get_unexplored_components(10).len(), 3);

    // Inspect one file
    tracker.record_inspection("src/b.rs", Some("hash_b".to_string()), None, None);
    let unexplored = tracker.get_unexplored_components(10);
    assert_eq!(unexplored.len(), 2);
    assert!(!unexplored.contains(&"src/b.rs".to_string()));
}

#[test]
fn test_atomic_persistence_and_reload() {
    let dir = tempdir().unwrap();
    let root = dir.path();

    let mut tracker = CoverageTracker::new();
    tracker.record_inspection("src/app.rs", Some("hash_app".to_string()), None, None);
    tracker.save_atomic(root).expect("save succeeded");

    assert!(root.join(".onyx/project/coverage.json").exists());

    // Reload from disk
    let loaded = CoverageTracker::load_or_init(root);
    assert_eq!(loaded.files.len(), 1);
    let rec = loaded.get_coverage("src/app.rs").unwrap();
    assert!(rec.status.inspected);
    assert_eq!(rec.last_known_hash, "hash_app");
}

#[test]
fn test_work_memory_task_summaries_and_pitfalls() {
    let dir = tempdir().unwrap();
    let root = dir.path();

    let mut mem = WorkMemory::new();

    let mut task = TaskSummary::new("task-1", "Implement Phase 2 Work Memory");
    task.components_involved = vec!["crates/project".to_string()];
    task.files_modified = vec!["crates/project/src/coverage.rs".to_string()];
    task.decisions = vec!["Use independent status booleans instead of linear states".to_string()];
    task.tests_performed = vec!["cargo test -p onyx-project".to_string()];

    mem.record_task_summary(task);

    mem.record_pitfall(
        "Search Hit False Positive",
        "Search hits must never be recorded as inspected files.",
        vec!["crates/project/src/coverage.rs".to_string()],
    );

    mem.save_atomic(root).expect("work memory saved");

    // Reload
    let loaded = WorkMemory::load_or_init(root);
    assert_eq!(loaded.task_summaries.len(), 1);
    assert_eq!(loaded.pitfalls.len(), 1);

    // Query recent work
    let recent = loaded.get_recent_work(5);
    assert_eq!(recent.len(), 1);
    assert_eq!(recent[0].task_id, "task-1");

    // Query relevant work
    let relevant = loaded.get_relevant_work("linear states");
    assert_eq!(relevant.len(), 1);
    assert_eq!(relevant[0].task_id, "task-1");

    let empty = loaded.get_relevant_work("nonexistent query xyz");
    assert!(empty.is_empty());
}
