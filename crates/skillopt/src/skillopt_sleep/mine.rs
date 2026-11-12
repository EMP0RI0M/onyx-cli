use crate::skillopt_sleep::types::{SessionDigest, TaskRecord};
use sha2::{Digest, Sha256};
use std::collections::HashMap;

/// Generate deterministic task ID based on project and intent.
pub fn generate_task_id(project: &str, intent: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(format!("{}::{}", project, intent).as_bytes());
    let hex = hex::encode(hasher.finalize());
    format!("task_{}", &hex[..12])
}

fn truncate_text(text: &str, max_len: usize) -> String {
    let trimmed = text.trim();
    if trimmed.len() <= max_len {
        trimmed.to_string()
    } else {
        format!("{} ...", &trimmed[..max_len])
    }
}

pub fn looks_negative(signals: &[String]) -> bool {
    signals.iter().any(|s| s.starts_with("neg:"))
}

pub fn looks_positive(signals: &[String]) -> bool {
    signals.iter().any(|s| s.starts_with("pos:"))
}

/// Deterministic offline task miner — converts SessionDigest list into TaskRecord list.
pub fn heuristic_mine(digests: &[SessionDigest], max_tasks: usize) -> Vec<TaskRecord> {
    let mut tasks = Vec::new();

    for d in digests {
        if d.user_prompts.is_empty() {
            continue;
        }

        let intent = &d.user_prompts[0];
        if intent.trim().len() < 8 {
            continue;
        }

        let outcome = if looks_positive(&d.feedback_signals) && !looks_negative(&d.feedback_signals) {
            "success".to_string()
        } else if looks_negative(&d.feedback_signals) {
            "fail".to_string()
        } else if d.n_user_turns >= 3 {
            "mixed".to_string()
        } else {
            "unknown".to_string()
        };

        let attempted = d.assistant_finals.last().cloned().unwrap_or_default();
        let context = if d.user_prompts.len() > 1 {
            let follow_ups: Vec<String> = d.user_prompts[1..d.user_prompts.len().min(4)]
                .iter()
                .map(|p| format!("- {}", truncate_text(p, 200)))
                .collect();
            format!("Follow-up constraints from same session:\n{}", follow_ups.join("\n"))
        } else {
            String::new()
        };

        let mut tags = Vec::new();
        if !d.tools_used.is_empty() {
            tags.push(format!("tools:{}", d.tools_used[..d.tools_used.len().min(4)].join("+")));
        }
        if !d.git_branch.is_empty() {
            tags.push(format!("branch:{}", d.git_branch));
        }

        tasks.push(TaskRecord {
            id: generate_task_id(&d.project, intent),
            project: d.project.clone(),
            intent: truncate_text(intent, 800),
            context_excerpt: truncate_text(&context, 600),
            system: String::new(),
            attempted_solution: truncate_text(&attempted, 600),
            outcome,
            reference_kind: "none".to_string(),
            reference: String::new(),
            judge: HashMap::new(),
            tags,
            source_sessions: if d.session_id.is_empty() { vec![] } else { vec![d.session_id.clone()] },
            split: "train".to_string(),
            origin: "real".to_string(),
            derived_from: String::new(),
            skill_hint: d.skills_used.first().cloned().unwrap_or_default(),
        });

        if tasks.len() >= max_tasks {
            break;
        }
    }

    tasks
}

/// Enrich mined tasks with real Project Intelligence evidence: WorkMemory tasks,
/// regressions, decisions, and unverified/failing coverage components.
pub fn mine_with_project_intelligence(
    digests: &[SessionDigest],
    work_memory: Option<&onyx_project::WorkMemory>,
    coverage: Option<&onyx_project::CoverageTracker>,
    max_tasks: usize,
) -> Vec<TaskRecord> {
    let mut tasks = heuristic_mine(digests, max_tasks);

    // If WorkMemory is present, add high-value tasks from project regressions or pitfalls
    if let Some(mem) = work_memory {
        for t in &mem.task_summaries {
            if !t.regressions_discovered.is_empty() || !t.unresolved_questions.is_empty() {
                let context_desc = format!(
                    "Regressions: {} | Decisions: {}",
                    t.regressions_discovered.join("; "),
                    t.decisions.join("; ")
                );

                tasks.push(TaskRecord {
                    id: format!("proj-regress-{}", t.task_id),
                    project: "project-intelligence".to_string(),
                    intent: truncate_text(&format!("Prevent regression in {}: {}", t.components_involved.join(", "), t.objective), 800),
                    context_excerpt: truncate_text(&context_desc, 600),
                    system: String::new(),
                    attempted_solution: truncate_text(&t.successful_changes.join("; "), 600),
                    outcome: "fail".to_string(), // Regression implies prior failure to learn from
                    reference_kind: "regression_history".to_string(),
                    reference: t.task_id.clone(),
                    judge: HashMap::new(),
                    tags: vec!["evidence:work_memory".to_string(), "type:regression".to_string()],
                    source_sessions: t.session_id.clone().into_iter().collect(),
                    split: "train".to_string(),
                    origin: "work_memory".to_string(),
                    derived_from: t.task_id.clone(),
                    skill_hint: String::new(),
                });
            }

            if tasks.len() >= max_tasks {
                break;
            }
        }
    }

    // Ingest failing test evidence from CoverageTracker
    if let Some(cov) = coverage {
        for (file, record) in &cov.files {
            let has_failures = record.evidence_history.iter().any(|ev| {
                ev.status == onyx_project::VerificationStatus::Failed
            });

            if has_failures {
                tasks.push(TaskRecord {
                    id: format!("cov-fail-{}", generate_task_id("coverage", file)),
                    project: "coverage".to_string(),
                    intent: format!("Fix failing verification and regressions in `{file}`"),
                    context_excerpt: format!("File has failing test evidence. Last hash: {}", record.last_known_hash),
                    system: String::new(),
                    attempted_solution: String::new(),
                    outcome: "fail".to_string(),
                    reference_kind: "test_failure".to_string(),
                    reference: file.clone(),
                    judge: HashMap::new(),
                    tags: vec!["evidence:coverage_tracker".to_string(), "status:failing_test".to_string()],
                    source_sessions: vec![],
                    split: "train".to_string(),
                    origin: "coverage_tracker".to_string(),
                    derived_from: file.clone(),
                    skill_hint: String::new(),
                });
            }

            if tasks.len() >= max_tasks {
                break;
            }
        }
    }

    tasks.truncate(max_tasks);
    tasks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_heuristic_mine_outcomes() {
        let digests = vec![
            SessionDigest {
                session_id: "s1".to_string(),
                project: "repo_a".to_string(),
                user_prompts: vec!["Refactor database connections".to_string()],
                assistant_finals: vec!["Refactored connection pooling".to_string()],
                feedback_signals: vec!["pos:works now".to_string()],
                ..Default::default()
            },
            SessionDigest {
                session_id: "s2".to_string(),
                project: "repo_b".to_string(),
                user_prompts: vec!["Fix the authentication bug".to_string()],
                feedback_signals: vec!["neg:still broken".to_string()],
                ..Default::default()
            },
        ];

        let tasks = heuristic_mine(&digests, 10);
        assert_eq!(tasks.len(), 2);
        assert_eq!(tasks[0].outcome, "success");
        assert_eq!(tasks[1].outcome, "fail");
        assert!(tasks[0].id.starts_with("task_"));
    }
}
