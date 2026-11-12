//! Agent-driven runtime capability discovery.
//!
//! Provides the `search_capabilities` tool and governance for on-demand
//! capability discovery during agent execution without restarting the session
//! or mutating the global ToolCollection.

use std::sync::{Arc, Mutex};
use std::time::Instant;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tools::{Args, Output, Tool, ToolError};
use onyx_retrieval::{Origin, SearchMethod, SearchOptions, TraceEvent};
use crate::retrieval::CapabilitySelector;

/// Configuration for capability discovery.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveryConfig {
    pub enabled: bool,
    pub strategy: String,
    pub initial_top_k: usize,
    pub additional_top_k: usize,
    pub max_discovery_calls: usize,
    pub max_selected_capabilities: usize,
    pub progressive_disclosure: bool,
}

impl Default for DiscoveryConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            strategy: "hybrid".to_string(),
            initial_top_k: 5,
            additional_top_k: 3,
            max_discovery_calls: 3,
            max_selected_capabilities: 12,
            progressive_disclosure: true,
        }
    }
}

/// Turn-scoped discovery session state tracking newly discovered tools and limits.
#[derive(Default, Clone)]
pub struct TurnDiscoveryState {
    pub call_count: usize,
    pub discovered_tools: Vec<String>,
    pub newly_mounted: Vec<String>,
    pub activated_skills: Vec<String>,
}

/// A capability match returned by `search_capabilities`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveredItem {
    pub id: String,
    pub name: String,
    pub description: String,
    pub kind: String,
    pub score: f32,
}

/// The `search_capabilities` runtime tool.
pub struct SearchCapabilitiesTool {
    pub selector: Arc<CapabilitySelector>,
    pub state: Arc<Mutex<TurnDiscoveryState>>,
    pub config: DiscoveryConfig,
}

impl SearchCapabilitiesTool {
    pub fn new(
        selector: Arc<CapabilitySelector>,
        state: Arc<Mutex<TurnDiscoveryState>>,
        config: DiscoveryConfig,
    ) -> Self {
        Self {
            selector,
            state,
            config,
        }
    }
}

impl Tool for SearchCapabilitiesTool {
    fn name(&self) -> &str {
        "search_capabilities"
    }

    fn description(&self) -> &str {
        "Search the capability catalog for additional tools or skills when the current set is insufficient. Returns capability IDs, names, descriptions, and kinds."
    }

    fn arg_keys(&self) -> Vec<String> {
        vec!["query".into(), "limit".into()]
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Description of the task, tool, or capability needed."
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of capabilities to discover (default: 3)."
                }
            },
            "required": ["query"],
            "additionalProperties": false
        })
    }

    fn execute(&self, args: &Args) -> Result<Output, ToolError> {
        let query = args
            .get("query")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::BadArgs("missing 'query'".into()))?;
        let limit = args
            .get("limit")
            .and_then(|v| v.as_u64())
            .map(|u| u as usize)
            .unwrap_or(self.config.additional_top_k);

        let mut st = self.state.lock().unwrap();
        if st.call_count >= self.config.max_discovery_calls {
            return Ok(Output::ok(format!(
                "Discovery limit reached: maximum {} calls per turn allowed.",
                self.config.max_discovery_calls
            )));
        }
        st.call_count += 1;

        let start = Instant::now();
        let tool_reg = self.selector.tool_registry();
        let skill_reg = self.selector.skill_registry();

        let tool_hits = tool_reg
            .search_with_options(query, limit, Origin::Agent, SearchOptions::new(SearchMethod::Bm25))
            .unwrap_or_default();
        let skill_hits = skill_reg
            .search_with_options(query, limit, Origin::Agent, SearchOptions::new(SearchMethod::Bm25))
            .unwrap_or_default();

        let mut results = Vec::new();

        // 1. Tool capabilities
        for hit in tool_hits {
            if let Some(tool) = tool_reg.get(&hit.tool_id) {
                if !st.discovered_tools.contains(&hit.tool_id) {
                    st.discovered_tools.push(hit.tool_id.clone());
                    st.newly_mounted.push(hit.tool_id.clone());
                }
                results.push(DiscoveredItem {
                    id: tool.id.clone(),
                    name: tool.name.clone(),
                    description: tool.description.clone(),
                    kind: "tool".to_string(),
                    score: hit.score,
                });
            }
        }

        // 2. Skill capabilities (Progressive disclosure: L0 metadata only, no full body)
        for hit in skill_hits {
            if let Some(skill) = skill_reg.get(&hit.skill_id) {
                if !st.activated_skills.contains(&hit.skill_id) {
                    st.activated_skills.push(hit.skill_id.clone());
                }
                results.push(DiscoveredItem {
                    id: skill.id.clone(),
                    name: skill.name.clone(),
                    description: skill.description.clone(),
                    kind: "skill".to_string(),
                    score: hit.score,
                });
            }
        }

        let took_ms = start.elapsed().as_millis() as u64;

        // Trace event via existing retrieval trace infrastructure
        self.selector.tool_sink().record(TraceEvent::GatewaySearch {
            query: query.to_string(),
            origin: Origin::Agent,
            top_k: limit as u32,
            hits: results.len() as u32,
            took_ms,
        });

        if results.is_empty() {
            return Ok(Output::ok("No matching capabilities found.".to_string()));
        }

        let out_json = serde_json::to_string_pretty(&json!({
            "capabilities": results,
            "mounted_tools": st.newly_mounted,
            "message": "The discovered tools have been activated and are now available for invocation in subsequent steps."
        })).unwrap_or_else(|_| "{}".to_string());

        Ok(Output::ok(out_json))
    }
}
