use crate::skillopt::datasets::base::DataLoader;
use crate::skillopt::types::{BatchSpec, Patch, RolloutResult};
use async_trait::async_trait;
use std::collections::HashMap;

/// Abstract Environment Adapter defining rollout and task type contracts.
#[async_trait]
pub trait EnvAdapter: Send + Sync {
    /// Adapter identifier name.
    fn name(&self) -> &str;

    /// Return available task types in this benchmark environment.
    fn get_task_types(&self) -> Vec<String> {
        vec![]
    }

    /// Check if this adapter requires distributed / external actor runtimes.
    fn requires_distributed(&self) -> bool {
        false
    }

    /// Execute a rollout batch against the current skill.
    async fn rollout(
        &self,
        batch: &BatchSpec,
        skill_content: &str,
        out_dir: &str,
    ) -> anyhow::Result<Vec<RolloutResult>>;

    /// Generate reflection patches based on rollout episode results.
    async fn reflect(
        &self,
        results: &[RolloutResult],
        skill_content: &str,
        out_dir: &str,
    ) -> anyhow::Result<Vec<Patch>> {
        let _ = (results, skill_content, out_dir);
        Ok(vec![])
    }
}

/// Generic Mock or Template Environment for testing and prototyping.
pub struct MockEnvAdapter {
    pub name: String,
    pub task_types: Vec<String>,
}

impl MockEnvAdapter {
    pub fn new(name: &str, task_types: Vec<&str>) -> Self {
        Self {
            name: name.to_string(),
            task_types: task_types.into_iter().map(|s| s.to_string()).collect(),
        }
    }
}

#[async_trait]
impl EnvAdapter for MockEnvAdapter {
    fn name(&self) -> &str {
        &self.name
    }

    fn get_task_types(&self) -> Vec<String> {
        self.task_types.clone()
    }

    async fn rollout(
        &self,
        batch: &BatchSpec,
        _skill_content: &str,
        _out_dir: &str,
    ) -> anyhow::Result<Vec<RolloutResult>> {
        let mut results = Vec::new();
        for i in 0..batch.batch_size {
            results.push(RolloutResult {
                id: format!("{}_task_{}", batch.split, i),
                hard: 1.0,
                soft: 1.0,
                n_turns: 1,
                task_type: self.task_types.first().cloned().unwrap_or_default(),
                extras: HashMap::new(),
                ..Default::default()
            });
        }
        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_mock_env_adapter() {
        let adapter = MockEnvAdapter::new("test_env", vec!["qa", "math"]);
        assert_eq!(adapter.name(), "test_env");
        assert_eq!(adapter.get_task_types().len(), 2);

        let batch = BatchSpec {
            phase: "train".to_string(),
            split: "train".to_string(),
            seed: 42,
            batch_size: 3,
            payload: None,
            metadata: HashMap::new(),
        };

        let res = adapter.rollout(&batch, "skill", "/tmp").await.unwrap();
        assert_eq!(res.len(), 3);
        assert_eq!(res[0].hard, 1.0);
    }
}
