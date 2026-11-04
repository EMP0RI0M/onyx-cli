//! OpenAI-compatible protocol: `/v1/chat/completions`, bearer auth, SSE deltas.
//!
//! Covers the overwhelming majority of the catalog (OpenAI itself plus every
//! gateway that mimics it — Groq, xAI, OpenRouter, DeepSeek, the local CLIProxyAPI
//! loopback, and Gemini's own `/openai` compatibility path). This is the default
//! adapter; only providers whose native API differs (Anthropic Messages, Google
//! generateContent) get their own module.

use super::types::{ParsedReply, ProviderProtocol, RequestCtx, StreamAcc, Usage};
use crate::{Message, ToolCall};
use serde_json::{json, Value};

pub struct OpenAiCompat;

impl ProviderProtocol for OpenAiCompat {
    fn name(&self) -> &'static str {
        "openai"
    }

    fn request_path(&self, base_url: &str) -> String {
        // base looks like http://host/v1 or https://api.openai.com/v1 → /v1/chat/completions
        let after = base_url.split("://").nth(1).unwrap_or(base_url);
        let sub = after.splitn(2, '/').nth(1).unwrap_or("v1");
        format!("/{}/chat/completions", sub.trim_end_matches('/'))
    }

    fn build_body(&self, ctx: &RequestCtx, history: &[Message]) -> Value {
        let mut msgs = vec![json!({ "role": "system", "content": ctx.system })];
        for m in history {
            match m.role.as_str() {
                "system" => {}
                "user" => msgs.push(json!({ "role": "user", "content": m.content })),
                "assistant" => {
                    let c = if m.content.is_empty() { "(calling a tool)" } else { &m.content };
                    msgs.push(json!({ "role": "assistant", "content": c }));
                }
                "tool" => msgs.push(json!({
                    "role": "user",
                    "content": format!("OBSERVATION (tool result): {}", m.content)
                })),
                _ => msgs.push(json!({ "role": "user", "content": m.content })),
            }
        }
        let mut body = json!({
            "model": ctx.model,
            "messages": msgs,
            "temperature": ctx.temperature,
            "max_tokens": ctx.max_tokens,
            "stream": ctx.stream,
        });
        if ctx.stream {
            body["stream_options"] = json!({ "include_usage": true });
        }
        if !ctx.effort.is_empty() {
            body["reasoning_effort"] = json!(ctx.effort);
        }
        if !ctx.tools.is_empty() {
            body["tools"] = json!(ctx.tools);
            body["tool_choice"] = json!("auto");
        }
        body
    }

    fn parse_reply(&self, raw: &str) -> Result<ParsedReply, String> {
        let v: Value = serde_json::from_str(raw).map_err(|e| format!("parse reply: {e}"))?;
        let choice = v.get("choices").and_then(|c| c.get(0));
        let msg = choice.and_then(|c| c.get("message"));
        let content = msg
            .and_then(|m| m.get("content"))
            .and_then(|c| c.as_str())
            .unwrap_or("")
            .to_string();
        let thinking = msg
            .and_then(|m| m.get("reasoning_content"))
            .and_then(|c| c.as_str())
            .map(|s| s.to_string());
        let tool_calls: Vec<ToolCall> = msg
            .and_then(|m| m.get("tool_calls"))
            .and_then(|t| t.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|tc| tc.get("function"))
                    .filter_map(|f| {
                        let name = f.get("name").and_then(|n| n.as_str())?.to_string();
                        let argstr = f.get("arguments").and_then(|a| a.as_str()).unwrap_or("{}");
                        let args = serde_json::from_str(argstr).unwrap_or(json!({}));
                        Some(ToolCall { name, args })
                    })
                    .collect()
            })
            .unwrap_or_default();
        Ok(ParsedReply { content, thinking, tool_calls, usage: usage_from(&v) })
    }

    fn parse_stream_event(&self, data: &str, acc: &mut StreamAcc) -> bool {
        if data == "[DONE]" {
            acc.done = true;
            return false;
        }
        let Ok(v) = serde_json::from_str::<Value>(data) else { return true };
        if let Some(u) = v.get("usage") {
            acc.usage = usage_from(&v).or(acc.usage.take());
            let _ = u;
        }
        let Some(choice) = v.get("choices").and_then(|c| c.get(0)) else { return true };
        let delta = choice.get("delta").cloned().unwrap_or_else(|| json!({}));
        if let Some(txt) = delta.get("content").and_then(|c| c.as_str()) {
            acc.content.push_str(txt);
        }
        if let Some(r) = delta.get("reasoning_content").and_then(|c| c.as_str()) {
            acc.thinking.push_str(r);
        }
        if let Some(tcs) = delta.get("tool_calls").and_then(|t| t.as_array()) {
            for tc in tcs {
                let idx = tc.get("index").and_then(|i| i.as_u64()).unwrap_or(0) as usize;
                let e = acc.calls.entry(idx).or_insert_with(|| (String::new(), String::new()));
                if let Some(f) = tc.get("function") {
                    if let Some(n) = f.get("name").and_then(|n| n.as_str()) {
                        e.0.push_str(n);
                    }
                    if let Some(a) = f.get("arguments").and_then(|a| a.as_str()) {
                        e.1.push_str(a);
                    }
                }
            }
        }
        true
    }
}

fn usage_from(v: &Value) -> Option<Usage> {
    let u = v.get("usage")?;
    Some(Usage {
        prompt_tokens: u.get("prompt_tokens").and_then(|p| p.as_u64()).unwrap_or(0) as usize,
        completion_tokens: u.get("completion_tokens").and_then(|p| p.as_u64()).unwrap_or(0) as usize,
        total_tokens: u.get("total_tokens").and_then(|p| p.as_u64()).unwrap_or(0) as usize,
        cached_prompt_tokens: u
            .get("prompt_tokens_details")
            .and_then(|d| d.get("cached_tokens"))
            .and_then(|c| c.as_u64())
            .unwrap_or(0) as usize,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Message;

    fn ctx<'a>(model: &'a str, tools: &'a [Value]) -> RequestCtx<'a> {
        RequestCtx {
            model,
            key: "k",
            temperature: 0.1,
            max_tokens: 1024,
            effort: "",
            stream: false,
            tools,
            system: "sys",
        }
    }

    #[test]
    fn path_derivation() {
        assert_eq!(
            OpenAiCompat.request_path("http://127.0.0.1:8317/v1"),
            "/v1/chat/completions"
        );
        assert_eq!(
            OpenAiCompat.request_path("https://api.openai.com/v1"),
            "/v1/chat/completions"
        );
    }

    #[test]
    fn body_carries_model_and_tools() {
        let tools = vec![json!({"type":"function","function":{"name":"vecmath"}})];
        let body = OpenAiCompat.build_body(&ctx("gpt-x", &tools), &[Message::text("user", "hi")]);
        assert_eq!(body["model"], "gpt-x");
        assert_eq!(body["messages"][0]["role"], "system");
        assert!(body["tools"].is_array());
    }

    #[test]
    fn parses_native_tool_call() {
        let raw = r#"{"choices":[{"message":{"content":"","tool_calls":[{"function":{"name":"bash","arguments":"{\"command\":\"ls\"}"}}]}}],"usage":{"prompt_tokens":5,"completion_tokens":3,"total_tokens":8}}"#;
        let p = OpenAiCompat.parse_reply(raw).unwrap();
        assert_eq!(p.tool_calls.len(), 1);
        assert_eq!(p.tool_calls[0].name, "bash");
        assert_eq!(p.usage.unwrap().total_tokens, 8);
    }

    #[test]
    fn stitches_streamed_tool_call() {
        let mut acc = StreamAcc::default();
        OpenAiCompat.parse_stream_event(
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"name":"vec"}}]}}]}"#,
            &mut acc,
        );
        OpenAiCompat.parse_stream_event(
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"name":"math","arguments":"{\"op\":"}}]}}]}"#,
            &mut acc,
        );
        OpenAiCompat.parse_stream_event(
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"\"dot\"}"}}]}}]}"#,
            &mut acc,
        );
        let done = !OpenAiCompat.parse_stream_event("[DONE]", &mut acc);
        assert!(done);
        let p = acc.into_parsed();
        assert_eq!(p.tool_calls[0].name, "vecmath");
        assert_eq!(p.tool_calls[0].args["op"], "dot");
    }
}
