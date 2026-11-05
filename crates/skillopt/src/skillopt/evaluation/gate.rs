use crate::skillopt::types::{GateAction, GateResult};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GateMetric {
    Hard,
    Soft,
    Mixed,
}

impl Default for GateMetric {
    fn default() -> Self {
        Self::Hard
    }
}

/// Compute semantic density of leading words in a skill document.
pub fn compute_semantic_density(skill_content: &str, leading_words: Option<&[&str]>) -> f64 {
    if skill_content.trim().is_empty() {
        return 0.0;
    }

    let default_words = [
        "must", "always", "never", "only", "critical", "important",
        "resolve", "prefer", "ensure", "strict", "verify",
    ];

    let mut skill = skill_content.to_string();
    for (start, end) in [
        ("<!-- SLOW_UPDATE_START -->", "<!-- SLOW_UPDATE_END -->"),
        ("<!-- APPENDIX_START -->", "<!-- APPENDIX_END -->"),
    ] {
        while let Some(s_idx) = skill.find(start) {
            if let Some(e_idx) = skill[s_idx..].find(end) {
                let actual_end = s_idx + e_idx + end.len();
                skill = format!("{}{}", &skill[..s_idx], &skill[actual_end..]);
            } else {
                skill = format!("{}{}", &skill[..s_idx], &skill[s_idx + start.len()..]);
                break;
            }
        }
    }

    // Extract alphanumeric word tokens
    let words: Vec<String> = skill
        .split(|c: char| !c.is_alphanumeric())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_ascii_lowercase())
        .collect();

    if words.is_empty() {
        return 0.0;
    }

    let target_words: Vec<String> = match leading_words {
        Some(list) => list.iter().map(|s| s.to_ascii_lowercase()).collect(),
        None => default_words.iter().map(|s| s.to_string()).collect(),
    };

    let leading_count = words
        .iter()
        .filter(|w| target_words.contains(w))
        .count();

    leading_count as f64 / words.len() as f64
}

/// Project (hard, soft) onto a single comparison metric.
pub fn select_gate_score(
    hard: f64,
    soft: f64,
    metric: GateMetric,
    mixed_weight: f64,
    skill_content: &str,
    use_semantic_density: bool,
    semantic_density_weight: f64,
    leading_words: Option<&[&str]>,
) -> f64 {
    let mut score = match metric {
        GateMetric::Hard => hard,
        GateMetric::Soft => soft,
        GateMetric::Mixed => {
            let w = mixed_weight.clamp(0.0, 1.0);
            (1.0 - w) * hard + w * soft
        }
    };

    if use_semantic_density {
        let density = compute_semantic_density(skill_content, leading_words);
        score += semantic_density_weight * density;
    }

    score
}

#[derive(Debug, Clone)]
pub struct GateOptions<'a> {
    pub cand_soft: f64,
    pub metric: GateMetric,
    pub mixed_weight: f64,
    pub use_semantic_density: bool,
    pub semantic_density_weight: f64,
    pub leading_words: Option<&'a [&'a str]>,
}

impl<'a> Default for GateOptions<'a> {
    fn default() -> Self {
        Self {
            cand_soft: 0.0,
            metric: GateMetric::Hard,
            mixed_weight: 0.5,
            use_semantic_density: false,
            semantic_density_weight: 0.05,
            leading_words: None,
        }
    }
}

/// Pure gate decision: compare candidate score to current and best.
pub fn evaluate_gate(
    candidate_skill: &str,
    cand_hard: f64,
    current_skill: &str,
    current_score: f64,
    best_skill: &str,
    best_score: f64,
    best_step: i64,
    global_step: i64,
    opts: GateOptions,
) -> GateResult {
    let cand_score = select_gate_score(
        cand_hard,
        opts.cand_soft,
        opts.metric,
        opts.mixed_weight,
        candidate_skill,
        opts.use_semantic_density,
        opts.semantic_density_weight,
        opts.leading_words,
    );

    if cand_score > current_score {
        if cand_score > best_score {
            GateResult {
                action: GateAction::AcceptNewBest,
                current_skill: candidate_skill.to_string(),
                current_score: cand_score,
                best_skill: candidate_skill.to_string(),
                best_score: cand_score,
                best_step: global_step,
            }
        } else {
            GateResult {
                action: GateAction::Accept,
                current_skill: candidate_skill.to_string(),
                current_score: cand_score,
                best_skill: best_skill.to_string(),
                best_score,
                best_step,
            }
        }
    } else {
        GateResult {
            action: GateAction::Reject,
            current_skill: current_skill.to_string(),
            current_score,
            best_skill: best_skill.to_string(),
            best_score,
            best_step,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gate_accept_new_best() {
        let res = evaluate_gate(
            "new_skill",
            0.8,
            "old_skill",
            0.5,
            "old_skill",
            0.5,
            0,
            1,
            GateOptions::default(),
        );
        assert_eq!(res.action, GateAction::AcceptNewBest);
        assert_eq!(res.current_score, 0.8);
        assert_eq!(res.best_step, 1);
    }

    #[test]
    fn test_gate_reject() {
        let res = evaluate_gate(
            "bad_skill",
            0.4,
            "current_skill",
            0.6,
            "best_skill",
            0.7,
            0,
            1,
            GateOptions::default(),
        );
        assert_eq!(res.action, GateAction::Reject);
        assert_eq!(res.current_skill, "current_skill");
    }

    #[test]
    fn test_semantic_density() {
        let content = "You MUST ALWAYS VERIFY your answers";
        let density = compute_semantic_density(content, None);
        assert!(density > 0.0);
    }
}
