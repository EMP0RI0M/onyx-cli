use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Normalize numeric and financial answers in OfficeQA:
/// - lowercasing and trimming
/// - removing commas (e.g., "$1,000,000" -> "$1000000")
/// - removing standard punctuation while preserving numbers, decimals, negatives, and percentage signs
/// - stripping financial units ("million", "billion", "dollars")
pub fn normalize_answer(text: &str) -> String {
    let lower = text.trim().to_lowercase().replace(',', "");
    let filtered: String = lower
        .chars()
        .filter(|&c| {
            !c.is_ascii_punctuation()
                || c.is_ascii_digit()
                || c == '.'
                || c == '-'
                || c == '%'
        })
        .collect();

    let units = ["million", "millions", "billion", "billions", "dollars", "dollar", "nominal"];
    let tokens: Vec<&str> = filtered
        .split_whitespace()
        .filter(|&w| !units.contains(&w))
        .collect();

    tokens.join(" ")
}

/// Compute exact match between predicted and gold financial text.
pub fn exact_match(prediction: &str, gold: &str) -> f64 {
    if normalize_answer(prediction) == normalize_answer(gold) {
        1.0
    } else {
        0.0
    }
}

/// Token F1 score for OfficeQA responses.
pub fn token_f1(prediction: &str, gold: &str) -> f64 {
    let pred_norm = normalize_answer(prediction);
    let gold_norm = normalize_answer(gold);

    let pred_tokens: Vec<&str> = pred_norm.split_whitespace().collect();
    let gold_tokens: Vec<&str> = gold_norm.split_whitespace().collect();

    if pred_tokens.is_empty() || gold_tokens.is_empty() {
        return if pred_tokens == gold_tokens { 1.0 } else { 0.0 };
    }

    let mut pred_counts = HashMap::new();
    for &tok in &pred_tokens {
        *pred_counts.entry(tok).or_insert(0) += 1;
    }

    let mut gold_counts = HashMap::new();
    for &tok in &gold_tokens {
        *gold_counts.entry(tok).or_insert(0) += 1;
    }

    let mut common = 0;
    for (tok, &p_count) in &pred_counts {
        if let Some(&g_count) = gold_counts.get(tok) {
            common += p_count.min(g_count);
        }
    }

    if common == 0 {
        return 0.0;
    }

    let precision = (common as f64) / (pred_tokens.len() as f64);
    let recall = (common as f64) / (gold_tokens.len() as f64);
    2.0 * precision * recall / (precision + recall)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OfficeQAEvalResult {
    pub em: f64,
    pub f1: f64,
    pub predicted_answer: String,
    pub gold_answer: String,
}

pub fn evaluate_officeqa(prediction: &str, gold: &str) -> OfficeQAEvalResult {
    OfficeQAEvalResult {
        em: exact_match(prediction, gold),
        f1: token_f1(prediction, gold),
        predicted_answer: prediction.trim().to_string(),
        gold_answer: gold.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_financial_answer() {
        assert_eq!(normalize_answer("$1,500.50 million"), "1500.50");
        assert_eq!(normalize_answer("35.4% increase"), "35.4% increase");
    }

    #[test]
    fn test_officeqa_evaluation() {
        let res = evaluate_officeqa("$1,200 million", "1200");
        assert_eq!(res.em, 1.0);
        assert_eq!(res.f1, 1.0);
    }
}
