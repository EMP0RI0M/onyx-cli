use crate::skillopt::model::ChatBackend;
use crate::skillopt::prompts;
use crate::skillopt::types::{Edit, Patch};

/// Merges a slice of patches in a batch into a single combined patch.
pub async fn merge_batch(
    skill_content: &str,
    patches: &[Patch],
    system_prompt: &str,
    level: usize,
    backend: Option<&(dyn ChatBackend + Send + Sync)>,
) -> Patch {
    if patches.is_empty() {
        return Patch::default();
    }
    if patches.len() == 1 {
        let mut p = patches[0].clone();
        for edit in &mut p.edits {
            edit.merge_level = Some(level as i64);
        }
        return p;
    }

    // Try LLM-based intelligent merging if a model backend is provided
    if let Some(llm) = backend {
        let patches_json = serde_json::to_string_pretty(patches).unwrap_or_default();
        let user_prompt = format!(
            "## Current Skill\n{}\n\n## Patches to merge ({} total, merge level {})\n{}",
            skill_content,
            patches.len(),
            level,
            patches_json
        );

        let messages = vec![
            crate::skillopt::model::ChatMessage {
                role: "system".to_string(),
                content: system_prompt.to_string(),
            },
            crate::skillopt::model::ChatMessage {
                role: "user".to_string(),
                content: user_prompt,
            },
        ];

        if let Ok(resp) = llm.generate_response(&messages).await {
            if let Ok(mut parsed) = serde_json::from_str::<Patch>(&resp) {
                for edit in &mut parsed.edits {
                    edit.merge_level = Some(level as i64);
                }
                return parsed;
            }
        }
    }

    // Deterministic fallback: concatenate all edits preserving order
    let mut combined_edits = Vec::new();
    for p in patches {
        for edit in &p.edits {
            let mut e = edit.clone();
            if e.merge_level.is_none() {
                e.merge_level = Some(level as i64);
            }
            combined_edits.push(e);
        }
    }

    Patch {
        edits: combined_edits,
        reasoning: format!("Deterministic hierarchical merge level {}", level),
        ranking_details: None,
    }
}

/// Hierarchically merges an arbitrary number of patches with a branching factor.
pub async fn hierarchical_merge(
    skill_content: &str,
    patches: Vec<Patch>,
    system_prompt: &str,
    batch_size: usize,
    backend: Option<&(dyn ChatBackend + Send + Sync)>,
) -> Patch {
    if patches.is_empty() {
        return Patch::default();
    }
    if patches.len() == 1 {
        return patches[0].clone();
    }

    let b_size = batch_size.max(2);
    let mut current = patches;
    let mut level = 0;

    while current.len() > 1 {
        level += 1;
        let mut next_level = Vec::new();

        for chunk in current.chunks(b_size) {
            let merged = merge_batch(skill_content, chunk, system_prompt, level, backend).await;
            next_level.push(merged);
        }

        current = next_level;
    }

    current.into_iter().next().unwrap_or_default()
}

/// Full public Aggregate stage API: combines failure-driven and success-driven patches.
pub async fn merge_patches(
    skill_content: &str,
    failure_patches: Vec<Patch>,
    success_patches: Vec<Patch>,
    batch_size: usize,
    backend: Option<&(dyn ChatBackend + Send + Sync)>,
) -> Patch {
    let failure_prompt = prompts::load_prompt("merge_failure").unwrap_or("Merge failure-derived patches.");
    let success_prompt = prompts::load_prompt("merge_success").unwrap_or("Merge success-derived patches.");
    let final_prompt = prompts::load_prompt("merge_final").unwrap_or("Combine failure and success patches.");

    let merged_failure = if !failure_patches.is_empty() {
        Some(hierarchical_merge(skill_content, failure_patches, failure_prompt, batch_size, backend).await)
    } else {
        None
    };

    let merged_success = if !success_patches.is_empty() {
        Some(hierarchical_merge(skill_content, success_patches, success_prompt, batch_size, backend).await)
    } else {
        None
    };

    match (merged_failure, merged_success) {
        (Some(f), Some(s)) => {
            merge_batch(skill_content, &[f, s], final_prompt, 0, backend).await
        }
        (Some(f), None) => f,
        (None, Some(s)) => s,
        (None, None) => Patch::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skillopt::types::EditOp;

    #[tokio::test]
    async fn test_hierarchical_merge_empty() {
        let p = hierarchical_merge("skill", vec![], "prompt", 2, None).await;
        assert_eq!(p.edits.len(), 0);
    }

    #[tokio::test]
    async fn test_hierarchical_merge_deterministic_tree() {
        let p1 = Patch {
            edits: vec![Edit {
                op: EditOp::Append,
                content: "Edit 1".to_string(),
                ..Default::default()
            }],
            ..Default::default()
        };
        let p2 = Patch {
            edits: vec![Edit {
                op: EditOp::Append,
                content: "Edit 2".to_string(),
                ..Default::default()
            }],
            ..Default::default()
        };
        let p3 = Patch {
            edits: vec![Edit {
                op: EditOp::Append,
                content: "Edit 3".to_string(),
                ..Default::default()
            }],
            ..Default::default()
        };

        let merged = hierarchical_merge("skill", vec![p1, p2, p3], "prompt", 2, None).await;
        assert_eq!(merged.edits.len(), 3);
        assert_eq!(merged.edits[0].content, "Edit 1");
        assert_eq!(merged.edits[1].content, "Edit 2");
        assert_eq!(merged.edits[2].content, "Edit 3");
    }

    #[tokio::test]
    async fn test_merge_patches_prioritization() {
        let f_patch = Patch {
            edits: vec![Edit {
                op: EditOp::Replace,
                content: "Fix Bug".to_string(),
                ..Default::default()
            }],
            ..Default::default()
        };
        let s_patch = Patch {
            edits: vec![Edit {
                op: EditOp::Append,
                content: "Reinforce Pattern".to_string(),
                ..Default::default()
            }],
            ..Default::default()
        };

        let final_patch = merge_patches("skill", vec![f_patch], vec![s_patch], 2, None).await;
        assert_eq!(final_patch.edits.len(), 2);
        assert_eq!(final_patch.edits[0].content, "Fix Bug");
        assert_eq!(final_patch.edits[1].content, "Reinforce Pattern");
    }
}
