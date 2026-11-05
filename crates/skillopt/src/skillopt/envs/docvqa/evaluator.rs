use serde::{Deserialize, Serialize};
use crate::skillopt::utils::arch_accel::min3_u32;

pub const DEFAULT_ANLS_THRESHOLD: f64 = 0.5;

/// Normalize text by lowercasing and collapsing whitespace.
pub fn normalize_text(value: &str) -> String {
    let lower = value.trim().to_lowercase();
    lower.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Compute Levenshtein edit distance between two strings.
/// Uses AArch64 inline assembly min3 in the dynamic programming recurrence matrix.
pub fn levenshtein_distance(a: &str, b: &str) -> usize {
    if a == b {
        return 0;
    }
    if a.is_empty() {
        return b.len();
    }
    if b.is_empty() {
        return a.len();
    }

    let a_chars: Vec<char> = a.chars().collect();
    let b_chars: Vec<char> = b.chars().collect();

    let mut previous: Vec<u32> = (0..=b_chars.len() as u32).collect();
    for (i, &char_a) in a_chars.iter().enumerate() {
        let mut current = vec![(i + 1) as u32];
        for (j, &char_b) in b_chars.iter().enumerate() {
            let insert_cost = current[j] + 1;
            let delete_cost = previous[j + 1] + 1;
            let replace_cost = previous[j] + if char_a == char_b { 0 } else { 1 };
            current.push(min3_u32(insert_cost, delete_cost, replace_cost));
        }
        previous = current;
    }

    previous[b_chars.len()] as usize
}

/// Compute Average Normalized Levenshtein Similarity (ANLS) between predicted and target.
pub fn score_single_answer(predicted: &str, target: &str, threshold: f64) -> f64 {
    let pred_norm = normalize_text(predicted);
    let target_norm = normalize_text(target);

    if pred_norm.is_empty() && target_norm.is_empty() {
        return 1.0;
    }
    if pred_norm.is_empty() || target_norm.is_empty() {
        return 0.0;
    }

    let dist = levenshtein_distance(&pred_norm, &target_norm) as f64;
    let max_len = pred_norm.len().max(target_norm.len()) as f64;
    let norm_dist = dist / max_len;

    if norm_dist >= threshold {
        0.0
    } else {
        1.0 - norm_dist
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocVQAOutput {
    pub exact_match: f64,
    pub anls: f64,
}

/// Evaluate predicted answer against multiple candidate answers.
pub fn evaluate_docvqa(predicted: &str, candidate_answers: &[String], threshold: Option<f64>) -> DocVQAOutput {
    let thresh = threshold.unwrap_or(DEFAULT_ANLS_THRESHOLD);
    let mut best_anls = 0.0;
    let mut exact_match = 0.0;

    let norm_pred = normalize_text(predicted);

    for candidate in candidate_answers {
        let norm_cand = normalize_text(candidate);
        if !norm_pred.is_empty() && norm_pred == norm_cand {
            exact_match = 1.0;
        }

        let score = score_single_answer(predicted, candidate, thresh);
        if score > best_anls {
            best_anls = score;
        }
    }

    DocVQAOutput {
        exact_match,
        anls: best_anls,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_levenshtein_distance() {
        assert_eq!(levenshtein_distance("kitten", "sitting"), 3);
        assert_eq!(levenshtein_distance("rosettacode", "raisethysword"), 8);
        assert_eq!(levenshtein_distance("same", "same"), 0);
        assert_eq!(levenshtein_distance("", "hello"), 5);
        assert_eq!(levenshtein_distance("world", ""), 5);
    }

    #[test]
    fn test_score_single_answer() {
        let score = score_single_answer("invoice 100", "invoice 100", 0.5);
        assert_eq!(score, 1.0);

        let score2 = score_single_answer("invoice 100", "invoice 101", 0.5);
        assert!(score2 > 0.0 && score2 < 1.0);

        let score_diff = score_single_answer("completely different", "invoice 100", 0.5);
        assert_eq!(score_diff, 0.0);
    }

    #[test]
    fn test_evaluate_docvqa() {
        let targets = vec!["$1,234.50".to_string(), "1234.50".to_string()];
        let out = evaluate_docvqa("1234.50", &targets, None);
        assert_eq!(out.exact_match, 1.0);
        assert_eq!(out.anls, 1.0);
    }
}
