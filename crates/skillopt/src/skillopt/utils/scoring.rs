use crate::skillopt::types::RolloutResult;
use sha2::{Digest, Sha256};

/// Trait representing anything that provides hard and soft scores.
pub trait Scoreable {
    fn hard(&self) -> f64;
    fn soft(&self) -> f64;
}

impl Scoreable for RolloutResult {
    fn hard(&self) -> f64 {
        self.hard
    }
    fn soft(&self) -> f64 {
        self.soft
    }
}

impl Scoreable for (f64, f64) {
    fn hard(&self) -> f64 {
        self.0
    }
    fn soft(&self) -> f64 {
        self.1
    }
}

/// Compute average hard and soft accuracy from a slice of scoreable items.
pub fn compute_score<T: Scoreable>(results: &[T]) -> (f64, f64) {
    if results.is_empty() {
        return (0.0, 0.0);
    }

    let hard_sum: f64 = results.iter().map(|r| r.hard()).sum();
    let soft_sum: f64 = results.iter().map(|r| r.soft()).sum();
    let n = results.len() as f64;

    (hard_sum / n, soft_sum / n)
}

/// Return a short 16-character deterministic sha256 hash of skill content.
pub fn skill_hash(content: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    let hex_str = hex::encode(hasher.finalize());
    hex_str[..16.min(hex_str.len())].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compute_score_empty() {
        assert_eq!(compute_score::<RolloutResult>(&[]), (0.0, 0.0));
    }

    #[test]
    fn test_compute_score_values() {
        let items = vec![(1.0, 0.5), (0.0, 0.5)];
        let (hard, soft) = compute_score(&items);
        assert!((hard - 0.5).abs() < 1e-6);
        assert!((soft - 0.5).abs() < 1e-6);
    }

    #[test]
    fn test_skill_hash_deterministic() {
        let h1 = skill_hash("test_skill_content");
        let h2 = skill_hash("test_skill_content");
        assert_eq!(h1, h2);
        assert_eq!(h1.len(), 16);
    }
}
