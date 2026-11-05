// Auto-synthesized Rust implementation for skillopt_sleep/evidence.py
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use async_trait::async_trait;

/* SkillOpt-Sleep — the per-night evidentiary chain (``evidence.jsonl``).

The existing report/diagnostics answer *what* the cycle decided; they do not
answer *why*. This module records the full causal chain, per night:

    transcript session  ->  miner exchange (prompt + raw reply)
                        ->  mined task (+ its checks, + source session ids)
                        ->  split assignment (train / val)
                        ->  every replay attempt (phase-tagged, full prompt
                            and response, cache hits marked)
                        ->  per-task scores with the failing checks named
                        ->  the reflect exchange (prompt, raw reply, parsed
                            edits) and every gate trial
                        ->  the final gate decision with the score arithmetic
                        ->  what was staged

Design constraints (matching the sleep engine's contract):
  * pure stdlib, thread-safe (replay batches run in a thread pool);
  * every persisted string passes through best-effort ``redact_secrets``
    before the per-field length cap is applied;
  * append-only JSONL so a crashed night still leaves its partial chain;
  * zero behavior change when disabled (``evidence_log: false``).

Events share the shape::

    {"ts": <iso8601>, "seq": <int>, "stage": <str>, "event": <str>, ...} */

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EvidenceLog {
    #[serde(flatten)]
    pub data: HashMap<String, serde_json::Value>,
}

impl EvidenceLog {
    pub fn new() -> Self {
        Self::default()
    }
}

pub fn _now_iso() -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn _clean(value: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn log(stage: serde_json::Value, event: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn attach(backend: serde_json::Value, ev: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn get(backend: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn set_phase(backend: serde_json::Value, phase: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn phase(backend: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}

pub fn read_events(path: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::Null)
}
