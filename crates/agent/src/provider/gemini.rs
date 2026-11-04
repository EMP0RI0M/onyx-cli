//! Google Gemini native protocol: `generateContent` / `streamGenerateContent`.
//!
//! Used when a provider's base URL targets the generativelanguage.googleapis.com
//! REST API rather than its `/openai` compatibility path. Differences from OpenAI:
//! auth is `x-goog-api-key`; roles are `user`/`model` (no system message — a
//! top-level `systemInstruction`); content is `{parts:[{text|functionCall|
//! functionResponse}]}`; tools are `functionDeclarations`; streaming is SSE with
//! ````json`-free chunked objects under `candidates[0].content.parts`.

use super::types::{ParsedReply, ProviderProtocol, RequestCtx, StreamAcc, Usage};
use crate::{Message, ToolCall};
use serde_json::{json, Value};

pub struct Gemini;

impl ProviderProtocol for Gemini {
    fn name(&self) -> &'static str {
        "gemini"
    }

    fn request_path(&self, base_url: &str) -> String {
        // base like https://generativelanguage.googleapis.com/v1beta → append model verb
        let after = base_url.split("://").nth(1).unwrap_or(base_url);
        let sub = after.splitn(2, '/').nth(1).unwrap_or("v1beta");
        format!("/{sub}", sub = sub.trim_end_matches('/'))
        // NOTE: full path needs the model + verb; build_url() composes it.
    }

    fn headers(&self, ctx: &RequestCtx) -> Vec<(String, String)> {
        let mut h = vec![("content-type".to_string(), "application/json".to_string())];
        if !ctx.key.is_empty() {
            h.push(("x-goog-api-key".to_string(), ctx.key.to_string()));
        }
        h
    }

    /// Gemini embeds the model in the path, so we override the caller's notion of
    /// "path" by returning the full endpoint here via a helper the transport uses.
    fn build_body(&self, ctx: &RequestCtx, history: &[Message]) -> Value {
        let mut contents: Vec<Value> = Vec::new();
        let mut pending_fn: Vec<Value> = Vec::new();
        for m in history {
            match m.role.as_str() {
                "user" => {
                    flush_fn(&mut contents, &mut pending_fn);
                    contents.push(json!({"role":"user","parts":[{"text": m.content}]}));
                }
                "assistant" => {
                    if let Some(tc) = &m.tool_call {
                        contents.push(json!({
                            "role":"model",
                            "parts":[{ "functionCall": {"name": tc.name, "args": tc.args} }]
                        }));
                    } else if !m.content.is_empty() && m.content != "(calling a tool)" {
                        contents.push(json!({"role":"model","parts":[{"text": m.content}]}));
                    }
                }
                "tool" => {
                    // The engine stores the last call name on the assistant msg; we
                    // emit a functionResponse keyed to that name.
                    let name = last_fn_name(&contents).unwrap_or_else(|| "result".to_string());
                    pending_fn.push(json!({
                        "functionResponse": {"name": name, "response": {"content": m.content}}
                    }));
                }
                _ => {}
            }
        }
        flush_fn(&mut contents, &mut pending_fn);

        let mut body = json!({ "contents": contents });
        if !ctx.system.is_empty() {
            body["systemInstruction"] = json!({"parts":[{"text": ctx.system}]});
        }
        let mut cfg = json!({
            "temperature": ctx.temperature,
            "maxOutputTokens": ctx.max_tokens,
        });
        if !ctx.effort.is_empty() && ctx.effort != "none" {
            cfg["thinkingConfig"] = json!({"includeThoughts": true});
        }
        body["generationConfig"] = cfg;
        if !ctx.tools.is_empty() {
            let decls: Vec<Value> = ctx
                .tools
                .iter()
                .filter_map(|t| t.get("function").or(Some(t)).cloned())
                .map(|f| json!({
                    "name": f.get("name").and_then(|n| n.as_str()).unwrap_or(""),
                    "description": f.get("description").and_then(|d| d.as_str()).unwrap_or(""),
                    "parameters": sanitize_schema(f.get("parameters").cloned().unwrap_or_else(|| json!({"type":"object"}))),
                }))
                .collect();
            body["tools"] = json!([{ "functionDeclarations": decls }]);
        }
        body
    }

    fn parse_reply(&self, raw: &str) -> Result<ParsedReply, String> {
        let v: Value = serde_json::from_str(raw).map_err(|e| format!("parse reply: {e}"))?;
        Ok(parse_candidate(&v))
    }

    fn parse_stream_event(&self, data: &str, acc: &mut StreamAcc) -> bool {
        // Each SSE data payload is one generateContent-shaped object.
        let Ok(v) = serde_json::from_str::<Value>(data) else { return true };
        let p = parse_candidate(&v);
        acc.content.push_str(&p.content);
        if let Some(t) = p.thinking {
            acc.thinking.push_str(&t);
        }
        acc.block_tools.extend(p.tool_calls);
        if p.usage.is_some() {
            acc.usage = p.usage.or(acc.usage.take());
        }
        true
    }
}

fn parse_candidate(v: &Value) -> ParsedReply {
    let mut content = String::new();
    let thinking = String::new();
    let mut tool_calls = Vec::new();
    if let Some(cand) = v.get("candidates").and_then(|c| c.get(0)) {
        if let Some(parts) = cand.get("content").and_then(|c| c.get("parts")).and_then(|p| p.as_array()) {
            for part in parts {
                if let Some(fc) = part.get("functionCall") {
                    let name = fc.get("name").and_then(|n| n.as_str()).unwrap_or("").to_string();
                    let args = fc.get("args").cloned().unwrap_or_else(|| json!({}));
                    if !name.is_empty() {
                        tool_calls.push(ToolCall { name, args });
                    }
                } else if let Some(t) = part.get("text").and_then(|t| t.as_str()) {
                    // Thoughts vs answer: Gemini tags reasoning parts sometimes.
                    content.push_str(t);
                }
            }
        }
    }
    let usage = v.get("usageMetadata").map(|u| Usage {
        prompt_tokens: u.get("promptTokenCount").and_then(|x| x.as_u64()).unwrap_or(0) as usize,
        completion_tokens: u.get("candidatesTokenCount").and_then(|x| x.as_u64()).unwrap_or(0) as usize,
        total_tokens: u.get("totalTokenCount").and_then(|x| x.as_u64()).unwrap_or(0) as usize,
        cached_prompt_tokens: 0,
    });
    ParsedReply {
        content,
        thinking: if thinking.is_empty() { None } else { Some(thinking) },
        tool_calls,
        usage,
    }
}

/// Gemini's schema subset rejects OpenAI's `$schema`, `additionalProperties` and
/// some formats; strip the noisy keys recursively so declarations don't 400.
fn sanitize_schema(mut s: Value) -> Value {
    fn walk(v: &mut Value) {
        if let Some(o) = v.as_object_mut() {
            for k in ["$schema", "$ref", "additionalProperties"] {
                o.remove(k);
            }
            for (_k, val) in o.iter_mut() {
                walk(val);
            }
        } else if let Some(arr) = v.as_array_mut() {
            for item in arr.iter_mut() {
                walk(item);
            }
        }
    }
    walk(&mut s);
    s
}

fn last_fn_name(contents: &[Value]) -> Option<String> {
    for m in contents.iter().rev() {
        if m.get("role").and_then(|r| r.as_str()) == Some("model") {
            if let Some(parts) = m.get("parts").and_then(|p| p.as_array()) {
                for part in parts.iter().rev() {
                    if let Some(n) = part.get("functionCall").and_then(|f| f.get("name")).and_then(|n| n.as_str()) {
                        return Some(n.to_string());
                    }
                }
            }
        }
    }
    None
}

fn flush_fn(contents: &mut Vec<Value>, pending: &mut Vec<Value>) {
    if pending.is_empty() {
        return;
    }
    let blocks = std::mem::take(pending);
    contents.push(json!({"role":"user","parts":blocks}));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Message;

    fn ctx<'a>(model: &'a str, tools: &'a [Value]) -> RequestCtx<'a> {
        RequestCtx { model, key: "gk", temperature: 0.2, max_tokens: 512, effort: "", stream: false, tools, system: "sys" }
    }

    #[test]
    fn headers_use_goog_key() {
        let h = Gemini.headers(&ctx("gemini-2.0-flash", &[]));
        assert!(h.iter().any(|(k, v)| k == "x-goog-api-key" && v == "gk"));
    }

    #[test]
    fn system_instruction_and_roles() {
        let body = Gemini.build_body(&ctx("m", &[]), &[Message::text("user", "hi")]);
        assert_eq!(body["systemInstruction"]["parts"][0]["text"], "sys");
        assert_eq!(body["contents"][0]["role"], "user");
    }

    #[test]
    fn parses_function_call() {
        let raw = r#"{"candidates":[{"content":{"parts":[{"functionCall":{"name":"add","args":{"a":1,"b":2}}}]}}],"usageMetadata":{"promptTokenCount":3,"candidatesTokenCount":2,"totalTokenCount":5}}"#;
        let p = Gemini.parse_reply(raw).unwrap();
        assert_eq!(p.tool_calls[0].name, "add");
        assert_eq!(p.tool_calls[0].args["a"], 1);
        assert_eq!(p.usage.unwrap().total_tokens, 5);
    }

    #[test]
    fn schema_is_sanitized() {
        let tools = vec![json!({"type":"function","function":{"name":"f","parameters":{"type":"object","additionalProperties":false,"properties":{"x":{"type":"string"}}}}})];
        let body = Gemini.build_body(&ctx("m", &tools), &[]);
        let decl = &body["tools"][0]["functionDeclarations"][0];
        assert!(decl["parameters"].get("additionalProperties").is_none());
        assert_eq!(decl["parameters"]["properties"]["x"]["type"], "string");
    }
}
