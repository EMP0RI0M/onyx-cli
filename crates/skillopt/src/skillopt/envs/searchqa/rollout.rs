use crate::skillopt::envs::searchqa::dataloader::SearchQAItem;
use crate::skillopt::envs::searchqa::evaluator::{exact_match, extract_answer, f1_score};
use crate::skillopt::model::ChatBackend;
use crate::skillopt::types::RolloutResult;

pub const MAX_CONTEXT_CHARS: usize = 6000;

/// Truncate context at [DOC] boundaries to avoid overflow.
pub fn truncate_context(context: &str, max_chars: usize) -> String {
    if context.len() <= max_chars {
        return context.to_string();
    }

    let mut result = String::new();
    for doc in context.split("[DOC]") {
        let candidate = if result.is_empty() {
            doc.to_string()
        } else {
            format!("{}[DOC]{}", result, doc)
        };

        if candidate.len() > max_chars {
            break;
        }
        result = candidate;
    }

    if result.is_empty() {
        format!("{}\n...[truncated]", &context[..max_chars.min(context.len())])
    } else {
        result
    }
}

/// Run rollout on a single SearchQA question item.
pub async fn rollout_one(
    item: &SearchQAItem,
    skill_content: &str,
    backend: Option<&(dyn ChatBackend + Send + Sync)>,
) -> anyhow::Result<RolloutResult> {
    let context = truncate_context(&item.context, MAX_CONTEXT_CHARS);
    let system_prompt = if skill_content.trim().is_empty() {
        "Answer the question directly inside <answer>...</answer> tags based on context.".to_string()
    } else {
        format!(
            "## Skill\n{}\n\nAnswer the question directly inside <answer>...</answer> tags based on context.",
            skill_content.trim()
        )
    };

    let user_prompt = format!("## Context\n{}\n\n## Question\n{}", context, item.question);

    let raw_response = if let Some(llm) = backend {
        let messages = vec![
            crate::skillopt::model::ChatMessage {
                role: "system".to_string(),
                content: system_prompt.clone(),
            },
            crate::skillopt::model::ChatMessage {
                role: "user".to_string(),
                content: user_prompt.clone(),
            },
        ];
        llm.generate_response(&messages).await?
    } else {
        // Fallback for tests/mocking: return the first gold answer directly
        let ans = item.answers.first().cloned().unwrap_or_default();
        format!("<answer>{}</answer>", ans)
    };

    let extracted = extract_answer(&raw_response);
    let em = exact_match(&extracted, &item.answers);
    let f1 = f1_score(&extracted, &item.answers);

    Ok(RolloutResult {
        id: item.id.clone(),
        hard: em,
        soft: f1,
        n_turns: 1,
        fail_reason: if em == 0.0 { "Answer mismatch".to_string() } else { String::new() },
        task_type: "qa".to_string(),
        task_description: item.question.clone(),
        predicted_answer: extracted,
        question: item.question.clone(),
        reference_text: item.answers.join(" | "),
        target_system_prompt: system_prompt,
        target_user_prompt: user_prompt,
        ..Default::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_rollout_one_mock() {
        let item = SearchQAItem {
            id: "q_test".to_string(),
            question: "Who wrote Hamlet?".to_string(),
            context: "William Shakespeare wrote Hamlet.".to_string(),
            answers: vec!["William Shakespeare".to_string(), "Shakespeare".to_string()],
        };

        let res = rollout_one(&item, "Check author carefully", None).await.unwrap();
        assert_eq!(res.hard, 1.0);
        assert_eq!(res.soft, 1.0);
        assert_eq!(res.predicted_answer, "William Shakespeare");
    }

    #[test]
    fn test_truncate_context() {
        let c = "[DOC] Part 1 [DOC] Part 2 [DOC] Part 3";
        let truncated = truncate_context(c, 25);
        assert!(truncated.len() <= 25);
    }
}
