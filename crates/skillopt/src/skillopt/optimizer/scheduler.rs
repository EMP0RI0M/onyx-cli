use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SchedulerMode {
    Constant,
    Linear,
    Cosine,
    Autonomous,
}

impl Default for SchedulerMode {
    fn default() -> Self {
        Self::Constant
    }
}

/// Learning-rate / edit budget scheduler for ReflACT training steps.
#[derive(Debug, Clone)]
pub struct LRScheduler {
    pub mode: SchedulerMode,
    pub max_lr: usize,
    pub min_lr: usize,
    pub total_steps: usize,
    current_step: usize,
}

impl LRScheduler {
    pub const NO_LIMIT: usize = 999;

    pub fn new(mode: SchedulerMode, max_lr: usize, min_lr: usize, total_steps: usize) -> Self {
        Self {
            mode,
            max_lr,
            min_lr,
            total_steps,
            current_step: 0,
        }
    }

    /// Advance one step and return the edit budget for this step.
    pub fn step(&mut self) -> usize {
        self.current_step += 1;
        self.get_lr(self.current_step)
    }

    /// Compute edit budget for a 1-indexed step.
    pub fn get_lr(&self, step: usize) -> usize {
        match self.mode {
            SchedulerMode::Constant => self.max_lr,
            SchedulerMode::Autonomous => Self::NO_LIMIT,
            SchedulerMode::Linear => {
                if self.total_steps <= 1 {
                    return self.max_lr;
                }
                let t = (step.min(self.total_steps) as f64) / (self.total_steps as f64);
                let lr = (self.max_lr as f64) + ((self.min_lr as f64) - (self.max_lr as f64)) * t;
                self.min_lr.max(lr.round() as usize)
            }
            SchedulerMode::Cosine => {
                if self.total_steps <= 1 {
                    return self.max_lr;
                }
                let t = (step.min(self.total_steps) as f64) / (self.total_steps as f64);
                let lr = (self.min_lr as f64)
                    + 0.5 * ((self.max_lr as f64) - (self.min_lr as f64))
                        * (1.0 + (std::f64::consts::PI * t).cos());
                self.min_lr.max(lr.round() as usize)
            }
        }
    }

    pub fn current_step(&self) -> usize {
        self.current_step
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_constant_scheduler() {
        let mut s = LRScheduler::new(SchedulerMode::Constant, 8, 2, 10);
        assert_eq!(s.step(), 8);
        assert_eq!(s.step(), 8);
    }

    #[test]
    fn test_linear_scheduler() {
        let mut s = LRScheduler::new(SchedulerMode::Linear, 10, 2, 4);
        assert_eq!(s.step(), 8);
        assert_eq!(s.step(), 6);
        assert_eq!(s.step(), 4);
        assert_eq!(s.step(), 2);
    }

    #[test]
    fn test_cosine_scheduler() {
        let mut s = LRScheduler::new(SchedulerMode::Cosine, 8, 2, 4);
        assert!(s.step() >= 2);
        assert_eq!(s.get_lr(4), 2);
    }

    #[test]
    fn test_autonomous_scheduler() {
        let mut s = LRScheduler::new(SchedulerMode::Autonomous, 8, 2, 10);
        assert_eq!(s.step(), LRScheduler::NO_LIMIT);
    }
}
