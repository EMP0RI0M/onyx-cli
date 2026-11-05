use crate::skillopt::model::ChatBackend;
use crate::skillopt::prompts;
use crate::skillopt::types::{Edit, Patch};

/// Trims or selects edits to satisfy a maximum edit budget.
pub async fn rank_and_select(
    skill_content: &str,
    mut patch: Patch,
    max_edits: usize,
    backend: Option<&(dyn ChatBackend + Send + Sync)>,
) -> anyhow::Result<Patch> {
    if patch.edits.len() <= max_edits {
        return Ok(patch);
    }

    // If an LLM backend is provided, rank dynamically; otherwise, retain top-L by support_count or order
    if let Some(llm) = backend {
        let mut prompt_items = Vec::new();
        for (i, edit) in patch.edits.iter().enumerate() {
            prompt_items.push(format!(
                "[{}] {:?} target='{}' content='{}'",
                i, edit.op, edit.target, edit.content
            ));
        }

        let system_prompt = prompts::load_prompt("ranking").unwrap_or("Rank the most impactful edits.");
        let user_prompt = format!(
            "Current skill:\n{}\n\nProposed edits:\n{}\n\nSelect the best {} edit indices as a JSON list (e.g. [0, 1]).",
            skill_content,
            prompt_items.join("\n"),
            max_edits
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
            if let Ok(indices) = serde_json::from_str::<Vec<usize>>(&resp) {
                let selected: Vec<Edit> = indices
                    .into_iter()
                    .filter_map(|idx| patch.edits.get(idx).cloned())
                    .take(max_edits)
                    .collect();

                if !selected.is_empty() {
                    patch.edits = selected;
                    return Ok(patch);
                }
            }
        }
    }

    // Deterministic fallback: sort by support_count descending (if present) and truncate
    patch.edits.sort_by(|a, b| b.support_count.unwrap_or(0).cmp(&a.support_count.unwrap_or(0)));
    patch.edits.truncate(max_edits);
    Ok(patch)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skillopt::types::EditOp;

    #[tokio::test]
    async fn test_rank_and_select_within_budget() {
        let patch = Patch {
            edits: vec![Edit {
                op: EditOp::Append,
                content: "Rule 1".to_string(),
                ..Default::default()
            }],
            ..Default::default()
        };
        let res = rank_and_select("Skill", patch, 5, None).await.unwrap();
        assert_eq!(res.edits.len(), 1);
    }

    #[tokio::test]
    async fn test_rank_and_select_truncate() {
        let patch = Patch {
            edits: vec![
                Edit {
                    op: EditOp::Append,
                    content: "Low priority".to_string(),
                    support_count: Some(1),
                    ..Default::default()
                },
                Edit {
                    op: EditOp::Append,
                    content: "High priority".to_string(),
                    support_count: Some(10),
                    ..Default::default()
                },
                Edit {
                    op: EditOp::Append,
                    content: "Medium priority".to_string(),
                    support_count: Some(5),
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let res = rank_and_select("Skill", patch, 2, None).await.unwrap();
        assert_eq!(res.edits.len(), 2);
        assert_eq!(res.edits[0].content, "High priority");
        assert_eq!(res.edits[1].content, "Medium priority");
    }
}
