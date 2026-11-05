//! SkillOpt-Sleep — Stage 3 for Onyx: score mined tasks offline.
//!
//! Replay normally re-runs a task through a model under a candidate skill. That
//! needs a live backend; when there is none (the common case on this box, where
//! every gateway port refuses) the pipeline must still produce a *deterministic*
//! signal the gate can consume. These scorers are explicit proxy metrics over
//! the transcript evidence — they measure rule coverage and outcome quality, not
//! model accuracy — and are labelled as such so nobody reads them as evals.

use crate::skillopt::evaluation::gate::compute_semantic_density;
use crate::skillopt_sleep::types::{ReplayResult, TaskRecord};

/// Default phrase list an Onyx skill should cover to be useful.
pub const DEFAULT_LEADING_WORDS: &[&str] = &[
    "must", "always", "never", "only", "critical", "important", "resolve", "prefer", "ensure",
    "strict", "verify",
];

fn tokenise(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|s| s.len() > 2)
        .map(|s| s.to_string())
        .collect()
}

/// Deterministic proxy replay of one task against a skill document.
///
/// `hard` = the task's own outcome was a success (0.0 / 1.0).
/// `soft` = how much of the task's vocabulary the skill actually covers,
/// blended with the skill's directive density.
pub fn replay_task_deterministic(task: &TaskRecord, skill_content: &str) -> ReplayResult {
    let outcome_hard = match task.outcome.as_str() {
        "success" => 1.0,
        "mixed" => 0.5,
        "fail" => 0.0,
        _ => 0.0,
    };

    let skill_words: Vec<String> = tokenise(skill_content);
    if skill_words.is_empty() {
        return ReplayResult {
            id: task.id.clone(),
            hard: 0.0,
            soft: 0.0,
            fail_reason: "empty_skill".to_string(),
            task_type: "offline_proxy".to_string(),
            judge_rationale: "skill document has no tokens to match against".to_string(),
            ..Default::default()
        };
    }

    let mut intent_words = tokenise(&task.intent);
    intent_words.extend(tokenise(&task.context_excerpt));
    intent_words.sort();
    intent_words.dedup();

    let covered = if intent_words.is_empty() {
        0.0
    } else {
        intent_words.iter().filter(|w| skill_words.contains(w)).count() as f64
            / intent_words.len() as f64
    };

    let density = compute_semantic_density(skill_content, Some(DEFAULT_LEADING_WORDS));
    let soft = (0.7 * covered + 0.3 * (density * 8.0).min(1.0)).clamp(0.0, 1.0);

    let fail_reason = if outcome_hard < 1.0 && task.outcome != "unknown" {
        format!("outcome:{}", task.outcome)
    } else {
        String::new()
    };

    ReplayResult {
        id: task.id.clone(),
        hard: outcome_hard,
        soft,
        response: String::new(),
        fail_reason,
        task_type: "offline_proxy".to_string(),
        judge_rationale: format!(
            "intent coverage {covered:.2}, directive density {density:.3}, outcome {}",
            task.outcome
        ),
        tools_called: task.tags.iter().filter_map(|t| t.strip_prefix("tools:")).flat_map(|s| s.split('+')).map(|s| s.to_string()).collect(),
        tokens: 0,
        latency_ms: 0.0,
        optimizer_feedback: if covered < 0.15 {
            format!("Skill never mentions the vocabulary of: {}", task.intent)
        } else if outcome_hard == 0.0 {
            format!("Task failed and the skill offered no corrective rule: {}", task.intent)
        } else {
            String::new()
        },
    }
}

/// Aggregate replay results into mean (hard, soft), matching the online engine's
/// scoring convention so both paths feed the same gate.
pub fn aggregate(results: &[ReplayResult]) -> (f64, f64) {
    if results.is_empty() {
        return (0.0, 0.0);
    }
    let hard = results.iter().map(|r| r.hard).sum::<f64>() / results.len() as f64;
    let soft = results.iter().map(|r| r.soft).sum::<f64>() / results.len() as f64;
    (hard, soft)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(outcome: &str, intent: &str) -> TaskRecord {
        TaskRecord {
            id: "t1".into(),
            project: "p".into(),
            intent: intent.into(),
            outcome: outcome.into(),
            ..Default::default()
        }
    }

    #[test]
    fn empty_skill_scores_zero_and_is_labeled() {
        let r = replay_task_deterministic(&task("success", "build the parser"), "");
        assert_eq!(r.hard, 0.0);
        assert_eq!(r.fail_reason, "empty_skill");
    }

    #[test]
    fn failing_task_yields_optimizer_feedback() {
        let skill = "You MUST verify the build before shipping.";
        let r = replay_task_deterministic(&task("fail", "verify the build fails sometimes"), skill);
        assert_eq!(r.hard, 0.0);
        assert!(!r.optimizer_feedback.is_empty());
        assert!(r.soft > 0.0);
    }

    #[test]
    fn aggregate_matches_mean_of_pairs() {
        let rs = vec![
            ReplayResult { id: "a".into(), hard: 1.0, soft: 0.5, ..Default::default() },
            ReplayResult { id: "b".into(), hard: 0.0, soft: 0.1, ..Default::default() },
        ];
        let (h, s) = aggregate(&rs);
        assert_eq!(h, 0.5);
        assert!((s - 0.3).abs() < 1e-9);
    }
}
