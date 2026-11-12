use std::sync::Arc;
use crate::protocol::ChatCompletionRequest;
use crate::provider::ProviderClient;

#[derive(Clone)]
pub struct UpstreamTarget {
    pub provider: Arc<dyn ProviderClient>,
    pub mapped_model: String,
}

#[derive(Default, Clone)]
pub struct Router {
    pub routes: Vec<UpstreamTarget>,
}

#[derive(Clone)]
pub struct RouteDecision {
    pub target: UpstreamTarget,
}

impl Router {
    pub fn new() -> Self {
        Self { routes: Vec::new() }
    }

    pub fn add_target(&mut self, target: UpstreamTarget) {
        self.routes.push(target);
    }

    pub fn route(&self, req: &ChatCompletionRequest) -> Result<RouteDecision, String> {
        if self.routes.is_empty() {
            return Err("No upstream targets configured".into());
        }

        for r in &self.routes {
            if r.mapped_model == req.model {
                return Ok(RouteDecision { target: r.clone() });
            }
        }

        // Auto fallback
        Ok(RouteDecision { target: self.routes[0].clone() })
    }
}
