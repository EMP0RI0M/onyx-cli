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
        if let Some(err) = v.get("error") {
            let msg = err.get("message").and_then(|m| m.as_str()).unwrap_or("anthropic error");
            return Err(msg.to_string());
        }
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
        let usage = v.get("usage").map(|u| {
            let prompt_tokens = u.get("input_tokens").and_then(|x| x.as_u64()).unwrap_or(0) as usize;
            let completion_tokens = u.get("output_tokens").and_then(|x| x.as_u64()).unwrap_or(0) as usize;
            let cached_prompt_tokens = u.get("cache_read_input_tokens").and_then(|x| x.as_u64()).unwrap_or(0) as usize;
            let total_tokens = prompt_tokens + completion_tokens;
            Usage {
                prompt_tokens,
                completion_tokens,
                total_tokens,
                cached_prompt_tokens,
            }
        });
        Ok(ParsedReply {
            content,
            thinking: if thinking.is_empty() { None } else { Some(thinking) },
            tool_calls,
            usage,
        })
    }

    fn parse_stream_event(
        &self,
        data: &str,
        acc: &mut StreamAcc,
        on_delta: &mut dyn FnMut(&str),
        on_reasoning: &mut dyn FnMut(&str),
    ) -> bool {
        // data is the JSON after "data:"; Anthropic tags each with "type".
        let Ok(v) = serde_json::from_str::<Value>(data) else { return true };
        match v.get("type").and_then(|t| t.as_str()) {
            Some("content_block_delta") => {
                if let Some(d) = v.get("delta") {
                    match d.get("type").and_then(|t| t.as_str()) {
                        Some("text_delta") => {
                            if let Some(t) = d.get("text").and_then(|t| t.as_str()) {
                                if !t.is_empty() {
                                    acc.content.push_str(t);
                                    on_delta(t);
                                }
                            }
                        }
                        Some("thinking_delta") => {
                            if let Some(t) = d.get("thinking").and_then(|t| t.as_str()) {
                                if !t.is_empty() {
                                    acc.thinking.push_str(t);
                                    on_reasoning(t);
                                }
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
            Some("message_start") => {
                if let Some(msg) = v.get("message") {
                    if let Some(u) = msg.get("usage") {
                        let inp = u.get("input_tokens").and_then(|x| x.as_u64()).unwrap_or(0) as usize;
                        let cached = u.get("cache_read_input_tokens").and_then(|x| x.as_u64()).unwrap_or(0) as usize;
                        let e = acc.usage.get_or_insert_with(Usage::default);
                        e.prompt_tokens = inp;
                        e.cached_prompt_tokens = cached;
                        e.total_tokens = e.prompt_tokens + e.completion_tokens;
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
                    e.total_tokens = e.prompt_tokens + e.completion_tokens;
                }
            }
            Some("message_stop") => {
                acc.done = true;
                finalize_block_tools(acc);
                return false;
            }
            Some("error") => {
                let msg = v.get("error")
                    .and_then(|e| e.get("message"))
                    .and_then(|m| m.as_str())
                    .unwrap_or("anthropic streaming error");
                acc.content = format!("[error: {msg}]");
                on_delta(&acc.content);
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
        assert_eq!(p.usage.as_ref().unwrap().prompt_tokens, 7);
        assert_eq!(p.usage.as_ref().unwrap().completion_tokens, 4);
        assert_eq!(p.usage.as_ref().unwrap().total_tokens, 11);
    }

    #[test]
    fn parses_error_reply_and_stream_error() {
        let err_json = r#"{"type":"error","error":{"type":"authentication_error","message":"invalid x-api-key"}}"#;
        let res = Anthropic.parse_reply(err_json);
        assert!(res.is_err());
        assert_eq!(res.unwrap_err(), "invalid x-api-key");

        let mut acc = StreamAcc::default();
        let mut delta_out = String::new();
        let cont = Anthropic.parse_stream_event(
            err_json,
            &mut acc,
            &mut |d| delta_out.push_str(d),
            &mut |_| {},
        );
        assert!(!cont, "stream should stop on error");
        assert!(acc.done);
        assert_eq!(acc.content, "[error: invalid x-api-key]");
        assert_eq!(delta_out, "[error: invalid x-api-key]");
    }

    #[test]
    fn streams_tracks_usage_properly() {
        let mut acc = StreamAcc::default();
        Anthropic.parse_stream_event(
            r#"{"type":"message_start","message":{"usage":{"input_tokens":15,"cache_read_input_tokens":5}}}"#,
            &mut acc,
            &mut |_| {},
            &mut |_| {},
        );
        Anthropic.parse_stream_event(
            r#"{"type":"message_delta","usage":{"output_tokens":10}}"#,
            &mut acc,
            &mut |_| {},
            &mut |_| {},
        );
        let u = acc.usage.expect("usage present");
        assert_eq!(u.prompt_tokens, 15);
        assert_eq!(u.cached_prompt_tokens, 5);
        assert_eq!(u.completion_tokens, 10);
        assert_eq!(u.total_tokens, 25);
    }

    #[test]
    fn streams_text_then_tool_input() {
        let mut acc = StreamAcc::default();
        Anthropic.parse_stream_event(r#"{"type":"content_block_start","content_block":{"type":"tool_use","name":"vecmath"}}"#, &mut acc, &mut |_| {}, &mut |_| {});
        Anthropic.parse_stream_event(r#"{"type":"content_block_delta","delta":{"type":"input_json_delta","partial_json":"{\"op\":"}}"#, &mut acc, &mut |_| {}, &mut |_| {});
        Anthropic.parse_stream_event(r#"{"type":"content_block_delta","delta":{"type":"input_json_delta","partial_json":"\"dot\"}"}}"#, &mut acc, &mut |_| {}, &mut |_| {});
        let stop = !Anthropic.parse_stream_event(r#"{"type":"message_stop"}"#, &mut acc, &mut |_| {}, &mut |_| {});
        assert!(stop);
        let p = acc.into_parsed();
        assert_eq!(p.tool_calls[0].name, "vecmath");
        assert_eq!(p.tool_calls[0].args["op"], "dot");
    }
}
