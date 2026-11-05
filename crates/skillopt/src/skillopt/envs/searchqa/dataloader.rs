use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchQAItem {
    pub id: String,
    pub question: String,
    #[serde(default)]
    pub context: String,
    #[serde(default)]
    pub answers: Vec<String>,
}

/// Load SearchQA items from JSON array or JSONL file.
pub fn load_searchqa_items<P: AsRef<Path>>(path: P) -> anyhow::Result<Vec<SearchQAItem>> {
    let content = fs::read_to_string(path)?;
    let trimmed = content.trim();

    if trimmed.starts_with('[') {
        let items: Vec<SearchQAItem> = serde_json::from_str(trimmed)?;
        return Ok(items);
    }

    // JSON lines fallback
    let mut items = Vec::new();
    for line in trimmed.lines() {
        let line = line.trim();
        if !line.is_empty() {
            let item: SearchQAItem = serde_json::from_str(line)?;
            items.push(item);
        }
    }

    Ok(items)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_load_searchqa_sample() {
        let sample = r#"[
            {"id": "q1", "question": "What is 2+2?", "context": "Math basics", "answers": ["4", "four"]},
            {"id": "q2", "question": "Capital of France?", "context": "Geography", "answers": ["Paris"]}
        ]"#;
        let items: Vec<SearchQAItem> = serde_json::from_str(sample).unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].id, "q1");
        assert_eq!(items[1].answers[0], "Paris");
    }
}
