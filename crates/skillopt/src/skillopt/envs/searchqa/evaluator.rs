use std::collections::HashMap;
use crate::skillopt::utils::arch_accel::fast_strip_ascii_punctuation;

/// Normalize answer string following SQuAD conventions:
/// - lowercase
/// - remove punctuation (accelerated with NEON / SIMD)
/// - remove English articles ("a", "an", "the")
/// - collapse multiple whitespace characters
pub fn normalize_answer(s: &str) -> String {
    let lower = s.to_lowercase();
    let no_punct = fast_strip_ascii_punctuation(&lower);

    let tokens: Vec<&str> = no_punct
        .split_whitespace()
        .filter(|&w| w != "a" && w != "an" && w != "the")
        .collect();

    tokens.join(" ")
}

/// Extract answer from `<answer>...</answer>` tags.
/// Fallback: last non-empty line of the response.
pub fn extract_answer(text: &str) -> String {
    let lower = text.to_lowercase();
    if let Some(start_tag) = lower.rfind("<answer>") {
        let after_start = start_tag + "<answer>".len();
        if let Some(end_tag) = lower[after_start..].find("</answer>") {
            return text[after_start..after_start + end_tag].trim().to_string();
        }
    }

    for line in text.lines().rev() {
        let trimmed = line.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }

    text.trim().to_string()
}

/// Compute Exact Match (EM) against a set of gold answers.
pub fn exact_match(prediction: &str, gold_answers: &[String]) -> f64 {
    let norm_pred = normalize_answer(prediction);
    for gold in gold_answers {
        if normalize_answer(gold) == norm_pred {
            return 1.0;
        }
    }
    0.0
}

/// Compute token-level F1 score between prediction and a single gold answer.
pub fn compute_f1(prediction: &str, gold: &str) -> f64 {
    let pred_tokens: Vec<&str> = prediction.split_whitespace().collect();
    let gold_tokens: Vec<&str> = gold.split_whitespace().collect();

    if pred_tokens.is_empty() || gold_tokens.is_empty() {
        return if pred_tokens == gold_tokens { 1.0 } else { 0.0 };
    }

    let mut gold_counts: HashMap<&str, usize> = HashMap::new();
    for &t in &gold_tokens {
        *gold_counts.entry(t).or_insert(0) += 1;
    }

    let mut common = 0usize;
    let mut pred_counts: HashMap<&str, usize> = HashMap::new();
    for &t in &pred_tokens {
        *pred_counts.entry(t).or_insert(0) += 1;
    }

    for (k, v) in &pred_counts {
        if let Some(&gold_v) = gold_counts.get(k) {
            common += (*v).min(gold_v);
        }
    }

    if common == 0 {
        return 0.0;
    }

    let precision = common as f64 / pred_tokens.len() as f64;
    let recall = common as f64 / gold_tokens.len() as f64;
    (2.0 * precision * recall) / (precision + recall)
}

/// Compute maximum F1 score against a set of gold answers.
pub fn f1_score(prediction: &str, gold_answers: &[String]) -> f64 {
    let norm_pred = normalize_answer(prediction);
    let mut max_f1 = 0.0;

    for gold in gold_answers {
        let norm_gold = normalize_answer(gold);
        let score = compute_f1(&norm_pred, &norm_gold);
        if score > max_f1 {
            max_f1 = score;
        }
    }

    max_f1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_answer() {
        assert_eq!(normalize_answer("The quick brown fox!"), "quick brown fox");
        assert_eq!(normalize_answer("A sample text..."), "sample text");
        assert_eq!(normalize_answer("An apple a day"), "apple day");
    }

    #[test]
    fn test_extract_answer() {
        let text1 = "Reasoning here\n<answer>Paris</answer>";
        assert_eq!(extract_answer(text1), "Paris");

        let text2 = "No tags, just final line.\nFinal answer.";
        assert_eq!(extract_answer(text2), "Final answer.");
    }

    #[test]
    fn test_exact_match() {
        let gold = vec!["Washington D.C.".to_string(), "DC".to_string()];
        assert_eq!(exact_match("washington dc", &gold), 1.0);
        assert_eq!(exact_match("New York", &gold), 0.0);
    }

    #[test]
    fn test_f1_score() {
        let gold = vec!["the president of united states".to_string()];
        let score = f1_score("president united states", &gold);
        assert!((score - 0.8571428571428571).abs() < 1e-4);
    }
}
