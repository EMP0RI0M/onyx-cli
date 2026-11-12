use crate::skillopt::types::{BatchSpec, Edit, Patch, RolloutResult};
use async_trait::async_trait;

/// Abstract Task Environment interface for rollouts.
#[async_trait]
pub trait TaskEnvironment: Send + Sync {
    async fn run_rollout(&self, batch: &BatchSpec, current_skill: &str) -> anyhow::Result<Vec<RolloutResult>>;
}

/// Abstract LLM Backend for reflection and optimization.
#[async_trait]
pub trait ModelBackend: Send + Sync {
    async fn complete(&self, system_prompt: &str, user_prompt: &str) -> anyhow::Result<String>;
}

/// Core ReflACT 5-stage optimizer operations.
pub struct ReflActPipeline;

impl ReflActPipeline {
    /// Stage 3: Aggregate reflections into a combined patch
    pub fn aggregate_edits(edits: Vec<Edit>, reasoning: String) -> Patch {
        Patch {
            edits,
            reasoning,
            ranking_details: None,
        }
    }

    /// Stage 5: Apply a patch of edits to update a skill document
    pub fn apply_patch(skill: &str, patch: &Patch) -> String {
        let mut updated = skill.to_string();
        for edit in &patch.edits {
            match edit.op {
                crate::skillopt::types::EditOp::Append => {
                    if !updated.is_empty() && !updated.ends_with('\n') {
                        updated.push('\n');
                    }
                    updated.push_str(&edit.content);
                }
                crate::skillopt::types::EditOp::InsertAfter => {
                    if let Some(pos) = updated.find(&edit.target) {
                        let idx = pos + edit.target.len();
                        updated.insert_str(idx, &format!("\n{}", edit.content));
                    } else {
                        updated.push_str(&format!("\n{}", edit.content));
                    }
                }
                crate::skillopt::types::EditOp::Replace => {
                    if !edit.target.is_empty() {
                        updated = updated.replace(&edit.target, &edit.content);
                    }
                }
                crate::skillopt::types::EditOp::Delete => {
                    if !edit.target.is_empty() {
                        updated = updated.replace(&edit.target, "");
                    }
                }
            }
        }
        updated
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skillopt::types::EditOp;

    #[test]
    fn test_apply_patch_append() {
        let skill = "Header";
        let patch = Patch {
            edits: vec![Edit {
                op: EditOp::Append,
                content: "Step 1: Check inputs".to_string(),
                ..Default::default()
            }],
            reasoning: "Missing initial check".to_string(),
            ranking_details: None,
        };
        let res = ReflActPipeline::apply_patch(skill, &patch);
        assert_eq!(res, "Header\nStep 1: Check inputs");
    }

    #[test]
    fn test_apply_patch_replace() {
        let skill = "Rule: Do X";
        let patch = Patch {
            edits: vec![Edit {
                op: EditOp::Replace,
                target: "Do X".to_string(),
                content: "Do Y".to_string(),
                ..Default::default()
            }],
            ..Default::default()
        };
        let res = ReflActPipeline::apply_patch(skill, &patch);
        assert_eq!(res, "Rule: Do Y");
    }
}
