//! SkillOpt-Sleep — Stages 4-5 for Onyx: turn replay evidence into a gated
//! skill improvement.
//!
//! This is the offline counterpart of the online ReflACT loop. It reuses the
//! *same* primitives the trainer uses — hierarchical aggregation, the edit-budget
//! clip, `apply_patch`, and `evaluate_gate` — so an overnight consolidation and a
//! live training step produce byte-identical artefacts and are judged by one rule.

use crate::skillopt::engine::ReflActPipeline;
use crate::skillopt::evaluation::gate::{evaluate_gate, GateMetric, GateOptions};
use crate::skillopt::gradient::aggregate as gradient;
use crate::skillopt::model::ChatBackend;
use crate::skillopt::optimizer::{clip, scheduler::LRScheduler};
use crate::skillopt::types::{Edit, EditOp, GateAction, SourceType};
use crate::skillopt_sleep::onyx_replay::{self, DEFAULT_LEADING_WORDS};
use crate::skillopt_sleep::types::{ReplayResult, SkillGroupReport, TaskRecord};

/// Derive candidate rules from replay evidence, split by the source outcome,
/// exactly mirroring the online engine's failure/success patch separation.
pub fn edits_from_replays(tasks: &[TaskRecord], results: &[ReplayResult]) -> (Vec<Edit>, Vec<Edit>) {
    let by_id: std::collections::HashMap<&str, &ReplayResult> =
        results.iter().map(|r| (r.id.as_str(), r)).collect();

    let mut failure = Vec::new();
    let mut success = Vec::new();

    for t in tasks {
        let Some(r) = by_id.get(t.id.as_str()) else { continue };
        if !r.optimizer_feedback.trim().is_empty() {
            failure.push(Edit {
                op: EditOp::Append,
                content: format!("- ALWAYS {}", r.optimizer_feedback.trim()),
                support_count: Some(1),
                source_type: Some(SourceType::Failure),
                update_origin: t.project.clone(),
                update_target: "instructions".to_string(),
                ..Default::default()
            });
        } else if r.hard >= 1.0 && r.soft < 0.5 && !t.attempted_solution.trim().is_empty() {
            // The task went well but the skill barely describes it: reinforce
            // what worked so the next session does not rediscover it.
            let snippet: String = t.attempted_solution.trim().chars().take(180).collect();
            success.push(Edit {
                op: EditOp::Append,
                content: format!("- Prefer this verified approach: {snippet}",),
                support_count: Some(1),
                source_type: Some(SourceType::Success),
                update_origin: t.project.clone(),
                update_target: "instructions".to_string(),
                ..Default::default()
            });
        }
    }
    (failure, success)
}

/// One bounded consolidation pass over a skill document.
///
/// Returns the report plus the candidate text (unchanged when the gate rejects).
pub async fn consolidate_once(
    skill_name: &str,
    skill_content: &str,
    tasks: &[TaskRecord],
    baseline: f64,
    merge_batch_size: usize,
    budget: &mut LRScheduler,
    backend: Option<&(dyn ChatBackend + Send + Sync)>,
) -> anyhow::Result<(SkillGroupReport, String)> {
    let results: Vec<ReplayResult> = tasks
        .iter()
        .map(|t| onyx_replay::replay_task_deterministic(t, skill_content))
        .collect();
    let (cand_hard, cand_soft) = onyx_replay::aggregate(&results);

    let (failure_edits, success_edits) = edits_from_replays(tasks, &results);
    if failure_edits.is_empty() && success_edits.is_empty() {
        return Ok((
            SkillGroupReport {
                skill_name: skill_name.to_string(),
                status: "no_signal".to_string(),
                accepted: false,
                gate_action: "reject".to_string(),
                baseline_score: baseline,
                candidate_score: cand_hard,
                n_tasks: tasks.len(),
                n_applied_edits: 0,
                n_rejected_edits: 0,
                reason: "replay produced no actionable feedback".to_string(),
            },
            skill_content.to_string(),
        ));
    }

    let merged = gradient::merge_patches(
        skill_content,
        vec![ReflActPipeline::aggregate_edits(failure_edits, "failure-derived".into())],
        vec![ReflActPipeline::aggregate_edits(success_edits, "success-derived".into())],
        merge_batch_size.max(2),
        backend,
    )
    .await;

    let max_edits = budget.step();
    let clipped = clip::rank_and_select(skill_content, merged, max_edits, backend).await?;
    let proposed = clipped.edits.len();
    let candidate = ReflActPipeline::apply_patch(skill_content, &clipped);

    let gate = evaluate_gate(
        &candidate,
        cand_hard,
        skill_content,
        baseline,
        skill_content,
        baseline,
        0,
        budget.current_step() as i64,
        GateOptions {
            cand_soft,
            metric: GateMetric::Hard,
            mixed_weight: 0.5,
            use_semantic_density: true,
            semantic_density_weight: 0.05,
            leading_words: Some(DEFAULT_LEADING_WORDS),
        },
    );

    let accepted = !matches!(gate.action, GateAction::Reject);
    Ok((
        SkillGroupReport {
            skill_name: skill_name.to_string(),
            status: if accepted { "accepted".to_string() } else { "rejected".to_string() },
            accepted,
            gate_action: match gate.action {
                GateAction::AcceptNewBest => "accept_new_best".to_string(),
                GateAction::Accept => "accept".to_string(),
                GateAction::Reject => "reject".to_string(),
            },
            baseline_score: baseline,
            candidate_score: gate.current_score,
            n_tasks: tasks.len(),
            n_applied_edits: if accepted { proposed } else { 0 },
            n_rejected_edits: if accepted { 0 } else { proposed },
            reason: if accepted {
                format!("candidate score {:.3} beat baseline {:.3}", gate.current_score, baseline)
            } else {
                format!("candidate score {:.3} did not beat baseline {:.3}", gate.current_score, baseline)
            },
        },
        if accepted { gate.current_skill.clone() } else { skill_content.to_string() },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skillopt::optimizer::scheduler::SchedulerMode;
    use crate::skillopt_sleep::types::TaskRecord;

    #[test]
    fn failing_tasks_become_failure_edits() {
        let t = TaskRecord {
            id: "t1".into(),
            project: "p".into(),
            intent: "make the deploy script idempotent".into(),
            outcome: "fail".into(),
            ..Default::default()
        };
        let r = onyx_replay::replay_task_deterministic(&t, "unrelated text");
        let (f, s) = edits_from_replays(&[t], &[r]);
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].op, EditOp::Append);
        assert!(f[0].content.starts_with("- ALWAYS "));
        assert!(s.is_empty());
    }

    #[tokio::test]
    async fn consolidate_rejects_when_no_signal() {
        let mut sched = LRScheduler::new(SchedulerMode::Constant, 4, 1, 4);
        let (report, out) = consolidate_once(
            "s",
            "skill body",
            &[],
            0.5,
            2,
            &mut sched,
            None,
        )
        .await
        .unwrap();
        assert_eq!(report.status, "no_signal");
        assert!(!report.accepted);
        assert_eq!(out, "skill body");
    }
}
