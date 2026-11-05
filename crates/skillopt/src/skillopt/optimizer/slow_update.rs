use crate::skillopt::model::ChatBackend;
use crate::skillopt::prompts;
use crate::skillopt::types::SlowUpdateResult;

pub const SLOW_UPDATE_START: &str = "<!-- SLOW_UPDATE_START -->";
pub const SLOW_UPDATE_END: &str = "<!-- SLOW_UPDATE_END -->";

/// Check whether the protected slow update markers are present.
pub fn has_slow_update_field(skill: &str) -> bool {
    skill.contains(SLOW_UPDATE_START) && skill.contains(SLOW_UPDATE_END)
}

/// Inject an empty slow update block placeholder if not present.
pub fn inject_empty_slow_update_field(skill: &str) -> String {
    if has_slow_update_field(skill) {
        return skill.to_string();
    }
    format!("{}\n\n{}\n{}\n", skill.trim_end(), SLOW_UPDATE_START, SLOW_UPDATE_END)
}

/// Extract content contained inside the protected slow update markers.
pub fn extract_slow_update_field(skill: &str) -> String {
    let start_pos = match skill.find(SLOW_UPDATE_START) {
        Some(pos) => pos + SLOW_UPDATE_START.len(),
        None => return String::new(),
    };
    let end_pos = match skill[start_pos..].find(SLOW_UPDATE_END) {
        Some(pos) => start_pos + pos,
        None => return String::new(),
    };
    skill[start_pos..end_pos].trim().to_string()
}

/// Overwrite the protected slow update content.
pub fn replace_slow_update_field(skill: &str, new_content: &str) -> String {
    let start_pos = match skill.find(SLOW_UPDATE_START) {
        Some(pos) => pos + SLOW_UPDATE_START.len(),
        None => return skill.to_string(),
    };
    let end_pos = match skill[start_pos..].find(SLOW_UPDATE_END) {
        Some(pos) => start_pos + pos,
        None => return skill.to_string(),
    };
    format!(
        "{}\n{}\n{}",
        &skill[..start_pos],
        new_content.trim(),
        &skill[end_pos..]
    )
}

/// Epoch-level longitudinal slow update engine.
pub async fn run_slow_update(
    skill_content: &str,
    comparison_text: &str,
    backend: Option<&(dyn ChatBackend + Send + Sync)>,
) -> anyhow::Result<SlowUpdateResult> {
    let system_prompt = prompts::load_prompt("slow_update").unwrap_or("Write slow update guidance.");
    let user_prompt = format!(
        "Current skill:\n{}\n\nComparison:\n{}\n\nReturn JSON with keys 'slow_update_content', 'reasoning', and 'action'.",
        skill_content, comparison_text
    );

    if let Some(llm) = backend {
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
            if let Ok(res) = serde_json::from_str::<SlowUpdateResult>(&resp) {
                return Ok(res);
            }
        }
    }

    Ok(SlowUpdateResult {
        reasoning: "Deterministic baseline slow update".to_string(),
        slow_update_content: "Follow verified guidelines consistently.".to_string(),
        action: "update".to_string(),
        ..Default::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_inject_extract_replace() {
        let base = "# Title";
        let with_block = inject_empty_slow_update_field(base);
        assert!(has_slow_update_field(&with_block));
        assert_eq!(extract_slow_update_field(&with_block), "");

        let updated = replace_slow_update_field(&with_block, "Important Epoch Rules");
        assert_eq!(extract_slow_update_field(&updated), "Important Epoch Rules");
    }

    #[tokio::test]
    async fn test_run_slow_update_fallback() {
        let res = run_slow_update("skill", "prev vs curr", None).await.unwrap();
        assert!(!res.slow_update_content.is_empty());
        assert_eq!(res.action, "update");
    }
}
