//! Anthropic Messages protocol: `/v1/messages`.
//!
//! Genuinely different from OpenAI, which is why it can't reuse the compat
//! adapter: auth uses `x-api-key` + `anthropic-version` headers (not bearer);
//! the system prompt is a top-level field, not a message; assistant/user turns
//! carry typed **content blocks** (`text`, `tool_use`, `tool_result`); and
//! reasoning is an explicit `thinking` block with a token budget rather than a
//! `reasoning_effort` string. Streaming is SSE but with named event types
//! (`content_block_delta`, etc.) instead of bare deltas.

use super::types::{ParsedReply, ProviderProtocol, RequestCtx, StreamAcc, Usage};
use crate::{Message, ToolCall};
use serde_json::{json, Value};

/// Default API version we speak. Anthropic pins behaviour to this header.
const ANTHROPIC_VERSION: &str = "2023-06-01";

pub struct Anthropic;

impl ProviderProtocol for Anthropic {
    fn name(&self) -> &'static str {
        "anthropic"
    }

    fn request_path(&self, base_url: &str) -> String {
        let after = base_url.split("://").nth(1).unwrap_or(base_url);
        let sub = after.splitn(2, '/').nth(1).unwrap_or("v1");
        format!("/{}/messages", sub.trim_end_matches('/'))
    }

    fn headers(&self, ctx: &RequestCtx) -> Vec<(String, String)> {
        // Anthropic wants the key in x-api-key, NOT Authorization: Bearer. Signal
        // that by supplying our own authorization-less scheme; transport's bearer is
        // suppressed only when an `authorization` header is present, so instead we
        // pass the key here and callers must NOT also set bearer. We emit x-api-key.
        let mut h = vec![
            ("anthropic-version".to_string(), ANTHROPIC_VERSION.to_string()),
            ("content-type".to_string(), "application/json".to_string()),
        ];
        if !ctx.key.is_empty() {
            h.push(("x-api-key".to_string(), ctx.key.to_string()));
            // Suppress the generic bearer by providing a dummy authorization header
            // value equal to empty is not allowed; instead callers pass key=None.
        }
        h
    }

    fn build_body(&self, ctx: &RequestCtx, history: &[Message]) -> Value {
        // Anthropic requires alternating user/assistant turns and no trailing
        // assistant message. We collapse engine Messages into content blocks.
        let mut msgs: Vec<Value> = Vec::new();
        let mut pending_tool_results: Vec<Value> = Vec::new();
        for m in history {
            match m.role.as_str() {
                "user" => {
                    flush_results(&mut msgs, &mut pending_tool_results);
                    msgs.push(json!({"role":"user","content": m.content}));
                }
                "assistant" => {
                    if let Some(tc) = &m.tool_call {
                        let block = json!({
                            "type":"tool_use","id":safe_id(tc),"name":tc.name,"input":tc.args
                        });
                        push_or_merge_assistant(&mut msgs, block);
                    } else if !m.content.is_empty() && m.content != "(calling a tool)" {
                        msgs.push(json!({"role":"assistant","content": m.content}));
                    }
                }
                "tool" => {
                    // Each prior tool_use needs a matching tool_result block.
                    let id = last_tool_use_id(&msgs).unwrap_or_else(|| "toolu_0".to_string());
                    pending_tool_results.push(json!({
                        "type":"tool_result","tool_use_id":id,"content": m.content
                    }));
                }
                _ => {}
            }
        }
        flush_results(&mut msgs, &mut pending_tool_results);

        let mut body = json!({
            "model": ctx.model,
            "messages": msgs,
            "max_tokens": ctx.max_tokens,
            "temperature": ctx.temperature,
        });
        if !ctx.system.is_empty() {
            body["system"] = json!(ctx.system);
        }
        if ctx.stream {
            body["stream"] = json!(true);
        }
        // Reasoning: map effort → an extended-thinking token budget.
        if !ctx.effort.is_empty() && ctx.effort != "none" {
            let budget = match ctx.effort {
                "low" => 1024,
                "high" | "max" => 8192,
                _ => 4096, // medium
            };
            body["thinking"] = json!({"type":"enabled","budget_tokens":budget});
        }
        if !ctx.tools.is_empty() {
            // OpenAI tool defs → Anthropic {name, description, input_schema}.
            let tools: Vec<Value> = ctx
                .tools
                .iter()
                .filter_map(|t| t.get("function").or(Some(t)).cloned())
                .map(|f| json!({
                    "name": f.get("name").and_then(|n| n.as_str()).unwrap_or(""),
                    "description": f.get("description").and_then(|d| d.as_str()).unwrap_or(""),
                    "input_schema": f.get("parameters").cloned().unwrap_or_else(|| json!({"type":"object"})),
                }))
                .collect();
            body["tools"] = json!(tools);
        }
        body
    }

    fn parse_reply(&self, raw: &str) -> Result<ParsedReply, String> {
        let v: Value = serde_json::from_str(raw).map_err(|e| format!("parse reply: {e}"))?;
        let mut content = String::new();
        let mut thinking = String::new();
        let mut tool_calls = Vec::new();
        if let Some(blocks) = v.get("content").and_then(|c| c.as_array()) {
            for b in blocks {
                match b.get("type").and_then(|t| t.as_str()) {
                    Some("text") => {
                        if let Some(t) = b.get("text").and_then(|t| t.as_str()) {
                            content.push_str(t);
                        }
                    }
                    Some("thinking") => {
                        if let Some(t) = b.get("thinking").and_then(|t| t.as_str()) {
                            thinking.push_str(t);
                        }
                    }
                    Some("tool_use") => {
                        let name = b.get("name").and_then(|n| n.as_str()).unwrap_or("").to_string();
                        let input = b.get("input").cloned().unwrap_or_else(|| json!({}));
                        if !name.is_empty() {
                            tool_calls.push(ToolCall { name, args: input });
                        }
                    }
                    _ => {}
                }
            }
        }
        let usage = v.get("usage").map(|u| Usage {
            prompt_tokens: u.get("input_tokens").and_then(|x| x.as_u64()).unwrap_or(0) as usize,
            completion_tokens: u.get("output_tokens").and_then(|x| x.as_u64()).unwrap_or(0) as usize,
            total_tokens: 0,
            cached_prompt_tokens: 0,
        });
        Ok(ParsedReply {
            content,
            thinking: if thinking.is_empty() { None } else { Some(thinking) },
            tool_calls,
            usage,
        })
    }

    fn parse_stream_event(&self, data: &str, acc: &mut StreamAcc) -> bool {
        // data is the JSON after "data:"; Anthropic tags each with "type".
        let Ok(v) = serde_json::from_str::<Value>(data) else { return true };
        match v.get("type").and_then(|t| t.as_str()) {
            Some("content_block_delta") => {
                if let Some(d) = v.get("delta") {
                    match d.get("type").and_then(|t| t.as_str()) {
                        Some("text_delta") => {
                            if let Some(t) = d.get("text").and_then(|t| t.as_str()) {
                                acc.content.push_str(t);
                            }
                        }
                        Some("thinking_delta") => {
                            if let Some(t) = d.get("thinking").and_then(|t| t.as_str()) {
                                acc.thinking.push_str(t);
                            }
                        }
                        Some("input_json_delta") => {
                            // Accumulate partial tool input onto the last block tool.
                            if let Some(t) = d.get("partial_json").and_then(|t| t.as_str()) {
                                if let Some(last) = acc.block_tools.last_mut() {
                                    if let Some(s) = last.args.as_str() {
                                        let mut s = s.to_string();
                                        s.push_str(t);
                                        last.args = Value::String(s);
                                    }
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
            Some("content_block_start") => {
                if let Some(b) = v.get("content_block") {
                    if b.get("type").and_then(|t| t.as_str()) == Some("tool_use") {
                        let name = b.get("name").and_then(|n| n.as_str()).unwrap_or("").to_string();
                        acc.block_tools.push(ToolCall { name, args: Value::String(String::new()) });
                    }
                }
            }
            Some("message_delta") => {
                if let Some(u) = v.get("usage") {
                    let out = u.get("output_tokens").and_then(|x| x.as_u64()).unwrap_or(0) as usize;
                    let e = acc.usage.get_or_insert_with(Usage::default);
                    e.completion_tokens = out;
                }
            }
            Some("message_stop") => {
                acc.done = true;
                finalize_block_tools(acc);
                return false;
            }
            Some("error") => {
                acc.done = true;
                return false;
            }
            _ => {}
        }
        true
    }
}

/// Turn accumulated string-form tool args back into parsed JSON objects.
fn finalize_block_tools(acc: &mut StreamAcc) {
    for tc in &mut acc.block_tools {
        if let Some(s) = tc.args.as_str() {
            let parsed = if s.trim().is_empty() {
                json!({})
            } else {
                serde_json::from_str(s).unwrap_or_else(|_| json!({}))
            };
            tc.args = parsed;
        }
    }
}

fn safe_id(tc: &ToolCall) -> String {
    // Deterministic-ish id; Anthropic only needs it unique within the turn.
    format!("toolu_{}", fnv(&format!("{}{}", tc.name, tc.args)))
}

fn fnv(s: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

fn push_or_merge_assistant(msgs: &mut Vec<Value>, block: Value) {
    if let Some(last) = msgs.last_mut() {
        if last.get("role").and_then(|r| r.as_str()) == Some("assistant") {
            if let Some(arr) = last.get_mut("content").and_then(|c| c.as_array_mut()) {
                arr.push(block);
                return;
            }
            // Convert a plain-string assistant turn into a block array first.
            let prev = last["content"].as_str().unwrap_or("").to_string();
            *last = json!({"role":"assistant","content":[ {"type":"text","text":prev}, block ]});
            return;
        }
    }
    msgs.push(json!({"role":"assistant","content":[block]}));
}

fn last_tool_use_id(msgs: &[Value]) -> Option<String> {
    for m in msgs.iter().rev() {
        if let Some(arr) = m.get("content").and_then(|c| c.as_array()) {
            for b in arr.iter().rev() {
                if b.get("type").and_then(|t| t.as_str()) == Some("tool_use") {
                    if let Some(id) = b.get("id").and_then(|i| i.as_str()) {
                        return Some(id.to_string());
                    }
                }
            }
        }
    }
    None
}

fn flush_results(msgs: &mut Vec<Value>, pending: &mut Vec<Value>) {
    if pending.is_empty() {
        return;
    }
    let blocks = std::mem::take(pending);
    msgs.push(json!({"role":"user","content":blocks}));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Message;

    fn ctx<'a>(model: &'a str, tools: &'a [Value], stream: bool) -> RequestCtx<'a> {
        RequestCtx { model, key: "sk-ant", temperature: 0.1, max_tokens: 1024, effort: "medium", stream, tools, system: "be helpful" }
    }

    #[test]
    fn path_and_headers() {
        assert_eq!(Anthropic.request_path("https://api.anthropic.com/v1"), "/v1/messages");
        let h = Anthropic.headers(&ctx("claude", &[], false));
        assert!(h.iter().any(|(k, v)| k == "x-api-key" && v == "sk-ant"));
        assert!(h.iter().any(|(k, _)| k == "anthropic-version"));
    }

    #[test]
    fn system_is_top_level_and_thinking_budget() {
        let body = Anthropic.build_body(&ctx("claude-x", &[], false), &[Message::text("user", "hi")]);
        assert_eq!(body["system"], "be helpful");
        assert!(body.get("messages").unwrap().is_array());
        assert_eq!(body["thinking"]["budget_tokens"], 4096);
    }

    #[test]
    fn parses_tool_use_block() {
        let raw = r#"{"content":[{"type":"text","text":"working"},{"type":"tool_use","name":"bash","input":{"command":"ls"}}],"usage":{"input_tokens":7,"output_tokens":4}}"#;
        let p = Anthropic.parse_reply(raw).unwrap();
        assert_eq!(p.content, "working");
        assert_eq!(p.tool_calls[0].name, "bash");
        assert_eq!(p.tool_calls[0].args["command"], "ls");
        assert_eq!(p.usage.unwrap().prompt_tokens, 7);
    }

    #[test]
    fn streams_text_then_tool_input() {
        let mut acc = StreamAcc::default();
        Anthropic.parse_stream_event(r#"{"type":"content_block_start","content_block":{"type":"tool_use","name":"vecmath"}}"#, &mut acc);
        Anthropic.parse_stream_event(r#"{"type":"content_block_delta","delta":{"type":"input_json_delta","partial_json":"{\"op\":"}}"#, &mut acc);
        Anthropic.parse_stream_event(r#"{"type":"content_block_delta","delta":{"type":"input_json_delta","partial_json":"\"dot\"}"}}"#, &mut acc);
        let stop = !Anthropic.parse_stream_event(r#"{"type":"message_stop"}"#, &mut acc);
        assert!(stop);
        let p = acc.into_parsed();
        assert_eq!(p.tool_calls[0].name, "vecmath");
        assert_eq!(p.tool_calls[0].args["op"], "dot");
    }
}
