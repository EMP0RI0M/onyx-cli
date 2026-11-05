use crate::skillopt::types::BatchSpec;
use serde_json::Value;
use std::collections::HashMap;

/// Maps dataset / trainer split names to canonical names.
pub fn canonicalize_split(split: &str) -> &str {
    match split {
        "train" => "train",
        "valid_seen" | "selection" | "val" => "val",
        "valid_unseen" | "test" => "test",
        other => other,
    }
}

/// Computes the deterministic base seeds pool for training batches.
pub fn make_base_seeds(steps_per_epoch: usize, accumulation: usize, seed: u64) -> Vec<u64> {
    let batches_per_epoch = steps_per_epoch * accumulation;
    (0..batches_per_epoch)
        .map(|i| seed + (i as u64) + 1)
        .collect()
}

/// Deterministic pseudo-random shuffle for epoch seed planning (matching Python's LCG/shuffle properties).
pub fn shuffle_epoch_seeds(base_seeds: &[u64], epoch: usize, seed: u64) -> Vec<u64> {
    let mut shuffled = base_seeds.to_vec();
    if shuffled.is_empty() {
        return shuffled;
    }

    // Deterministic linear congruential generator for shuffling based on (seed + epoch * 1000)
    let mut state = seed.wrapping_add((epoch as u64).wrapping_mul(1000)).wrapping_add(1);
    for i in (1..shuffled.len()).rev() {
        state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        let j = (state % ((i + 1) as u64)) as usize;
        shuffled.swap(i, j);
    }
    shuffled
}

/// Abstract DataLoader trait for task batch construction.
pub trait DataLoader: Send + Sync {
    fn build_train_batch(&self, batch_size: usize, seed: u64, payload: Option<Value>) -> BatchSpec {
        BatchSpec {
            phase: "train".to_string(),
            split: "train".to_string(),
            seed,
            batch_size,
            payload,
            metadata: HashMap::new(),
        }
    }

    fn build_eval_batch(&self, split: &str, batch_size: usize, seed: u64, payload: Option<Value>) -> BatchSpec {
        BatchSpec {
            phase: "eval".to_string(),
            split: canonicalize_split(split).to_string(),
            seed,
            batch_size,
            payload,
            metadata: HashMap::new(),
        }
    }

    fn plan_train_epoch(
        &self,
        epoch: usize,
        steps_per_epoch: usize,
        accumulation: usize,
        batch_size: usize,
        seed: u64,
    ) -> Vec<BatchSpec> {
        let base_seeds = make_base_seeds(steps_per_epoch, accumulation, seed);
        let shuffled = shuffle_epoch_seeds(&base_seeds, epoch, seed);
        shuffled
            .into_iter()
            .map(|s| self.build_train_batch(batch_size, s, None))
            .collect()
    }
}

/// Basic standard dataloader implementation.
#[derive(Debug, Default, Clone)]
pub struct StandardDataLoader;

impl DataLoader for StandardDataLoader {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_canonicalize_split() {
        assert_eq!(canonicalize_split("valid_seen"), "val");
        assert_eq!(canonicalize_split("selection"), "val");
        assert_eq!(canonicalize_split("valid_unseen"), "test");
        assert_eq!(canonicalize_split("train"), "train");
    }

    #[test]
    fn test_make_base_seeds() {
        let seeds = make_base_seeds(2, 2, 100);
        assert_eq!(seeds, vec![101, 102, 103, 104]);
    }

    #[test]
    fn test_plan_train_epoch() {
        let loader = StandardDataLoader;
        let batches = loader.plan_train_epoch(0, 3, 1, 4, 42);
        assert_eq!(batches.len(), 3);
        assert_eq!(batches[0].batch_size, 4);
        assert_eq!(batches[0].phase, "train");
    }
}
