use std::collections::HashMap;
use std::fs;
use tempfile::tempdir;

use onyx_project::{TaskSummary, WorkMemory};
use skillopt::skillopt_sleep::onyx_facade::optimize_skills;
use skillopt::skillopt_sleep::proposals::{OptimizationProposal, ProposalStatus, ProposalStore};
use skills::SkillCatalog;

#[test]
fn test_end_to_end_skillopt_learning_and_versioning_lifecycle() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    std::env::set_var("ONYX_HOME", root.to_string_lossy().to_string());

    // 1. Set up a skill file
    let skills_dir = root.join("skills").join("deploy");
    fs::create_dir_all(&skills_dir).unwrap();
    let skill_path = skills_dir.join("SKILL.md");

    let initial_content = r#"---
name: deploy
description: Deploy application to production
tools_allowed: ["bash"]
---
# Deploy Playbook
Run deploy script directly.
"#;
    fs::write(&skill_path, initial_content).unwrap();

    let mut catalog = SkillCatalog::new();
    catalog.scan_all_default();

    let initial_skill = catalog.get("deploy").expect("skill exists");
    let initial_hash = initial_skill.content_hash();

    // 2. Simulate agent execution recording turn events to session transcript
    let sessions_dir = root.join("sessions");
    fs::create_dir_all(&sessions_dir).unwrap();
    let session_file = sessions_dir.join("sess_e2e_1.jsonl");

    // Realistic JSONL session transcript containing failure and user feedback
    let transcript = concat!(
        r#"{"kind":"meta","model":"test-model","ts":1,"cwd":"/root"}"#,
        "\n",
        r#"{"kind":"message","role":"user","content":"Deploy to production failed: env variable DB_URL is missing","ts":2}"#,
        "\n",
        r#"{"kind":"message","role":"assistant","content":"I need to check database credentials before deploying","ts":3}"#,
        "\n",
        r#"{"kind":"message","role":"user","content":"Always check DB_URL and run preflight checks before running deploy","ts":4}"#,
        "\n"
    );
    fs::write(&session_file, transcript).unwrap();

    // 3. Record Project Intelligence evidence (WorkMemory regression)
    let mut mem = WorkMemory::new();
    let mut task = TaskSummary::new("task-deploy-1", "Deploy service");
    task.regressions_discovered = vec!["Deploy crashed when DB_URL was unset".to_string()];
    task.decisions = vec!["Enforce preflight checks in deploy playbook".to_string()];
    mem.record_task_summary(task);
    mem.save_atomic(root).unwrap();

    // 4. Trigger SkillOpt optimization cycle
    let mut skills_map = HashMap::new();
    skills_map.insert("deploy".to_string(), initial_skill.instructions.clone());

    let outcome = optimize_skills(sessions_dir.clone(), skills_map).expect("optimization succeeds");
    assert!(!outcome.tasks.is_empty(), "Mined tasks should not be empty");

    // 5. Verify Proposal persistence
    let mut proposal_store = ProposalStore::load_or_init(root);
    let proposal = OptimizationProposal {
        id: "prop-deploy-1".to_string(),
        skill_name: "deploy".to_string(),
        baseline_hash: initial_hash.clone(),
        candidate_hash: "hash_cand_123".to_string(),
        baseline_score: 0.4,
        candidate_score: 0.9,
        gate_action: "Accept".to_string(),
        gate_reason: "Score improved".to_string(),
        candidate_instructions: "# Deploy Playbook\nAlways check DB_URL and run preflight checks before running deploy.".to_string(),
        source_tasks: vec!["task-deploy-1".to_string()],
        created_at: 1000,
        status: ProposalStatus::Pending,
        applied_at: None,
    };
    proposal_store.record_proposal(proposal);
    proposal_store.save_atomic(root).unwrap();

    // Reload proposal store from disk to verify cross-restart persistence
    let mut loaded_store = ProposalStore::load_or_init(root);
    assert!(loaded_store.proposals.contains_key("prop-deploy-1"));

    // 6. Apply proposal & verify atomic backup snapshot creation
    let current_disk_hash = initial_hash.clone();
    loaded_store.mark_applied("prop-deploy-1", &current_disk_hash).expect("apply proposal succeeds");

    let prev_hash = catalog
        .persist_instructions("deploy", "# Deploy Playbook\nAlways check DB_URL and run preflight checks before running deploy.")
        .expect("persist instructions succeeds");

    assert_eq!(prev_hash, initial_hash);

    // Verify backup exists under .onyx/skills/backups/
    let backup_dir = root.join("skills").join("backups");
    assert!(backup_dir.exists(), "Backup directory must exist");
    let backup_file = backup_dir.join(format!("deploy.{initial_hash}.md"));
    assert!(backup_file.exists(), "Snapshot of original version must be preserved");

    // 7. Verify rollback restores exact previous content
    let restored_hash = catalog.rollback_skill("deploy", None).expect("rollback succeeds");
    assert_eq!(restored_hash, initial_hash, "Restored hash must match original initial hash");

    let re_read = fs::read_to_string(&skill_path).unwrap();
    assert!(re_read.contains("Run deploy script directly."), "Rollback must restore original instruction body");

    // 8. Test stale proposal protection
    // Modifying file changes disk hash
    catalog
        .persist_instructions("deploy", "Modified content v2")
        .unwrap();

    let stale_apply_err = loaded_store.mark_applied("prop-deploy-1", "hash_new_different_version");
    assert!(stale_apply_err.is_err(), "Stale proposal must be blocked from overwriting newer version");
}
