use std::time::Instant;

/// Token and time budget controller for overnight sleep cycles.
#[derive(Debug, Clone)]
pub struct Budget {
    pub max_tokens: Option<usize>,
    pub max_minutes: Option<f64>,
    start_time: Option<Instant>,
    tokens_at_start: usize,
}

impl Budget {
    pub fn new(max_tokens: Option<usize>, max_minutes: Option<f64>) -> Self {
        Self {
            max_tokens,
            max_minutes,
            start_time: None,
            tokens_at_start: 0,
        }
    }

    pub fn start(&mut self, tokens_now: usize) {
        self.start_time = Some(Instant::now());
        self.tokens_at_start = tokens_now;
    }

    pub fn tokens_spent(&self, tokens_now: usize) -> usize {
        tokens_now.saturating_sub(self.tokens_at_start)
    }

    pub fn minutes_elapsed(&self) -> f64 {
        match self.start_time {
            Some(t) => t.elapsed().as_secs_f64() / 60.0,
            None => 0.0,
        }
    }

    pub fn is_exhausted(&self, tokens_now: usize) -> bool {
        if let Some(limit) = self.max_tokens {
            if self.tokens_spent(tokens_now) >= limit {
                return true;
            }
        }
        if let Some(limit) = self.max_minutes {
            if self.minutes_elapsed() >= limit {
                return true;
            }
        }
        false
    }

    pub fn remaining_fraction(&self, tokens_now: usize) -> f64 {
        let mut min_frac: f64 = 1.0;
        if let Some(limit) = self.max_tokens {
            let spent = self.tokens_spent(tokens_now) as f64;
            let frac = (1.0 - spent / limit as f64).clamp(0.0, 1.0);
            min_frac = min_frac.min(frac);
        }
        if let Some(limit) = self.max_minutes {
            let elapsed = self.minutes_elapsed();
            let frac = (1.0 - elapsed / limit).clamp(0.0, 1.0);
            min_frac = min_frac.min(frac);
        }
        min_frac
    }
}

/// Heuristically choose (nights, rollouts_per_task) from budget parameters.
pub fn plan_depth(
    budget: &Budget,
    n_tasks: usize,
    default_nights: usize,
    default_k: usize,
) -> (usize, usize) {
    let limit = match budget.max_tokens {
        Some(t) => t,
        None => return (default_nights, default_k),
    };

    let rollouts_affordable = limit as f64 / 1500.0;
    let per_night = (n_tasks.max(1) * 3) as f64;
    let nights = ((rollouts_affordable / per_night).floor() as usize).clamp(1, 5);
    (nights, default_k)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_budget_unlimited() {
        let mut b = Budget::new(None, None);
        b.start(0);
        assert!(!b.is_exhausted(100_000));
        assert_eq!(b.remaining_fraction(100_000), 1.0);
    }

    #[test]
    fn test_budget_token_limit() {
        let mut b = Budget::new(Some(1000), None);
        b.start(500);
        assert!(!b.is_exhausted(1000)); // 500 spent
        assert_eq!(b.remaining_fraction(1000), 0.5);
        assert!(b.is_exhausted(1500)); // 1000 spent
    }

    #[test]
    fn test_plan_depth() {
        let b = Budget::new(Some(15000), None);
        let (nights, k) = plan_depth(&b, 2, 2, 1);
        assert!(nights >= 1);
        assert_eq!(k, 1);
    }
}
