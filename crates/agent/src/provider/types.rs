//! Protocol abstraction: what a provider speaks, independent of transport.
//!
//! `HttpModel` historically hardcoded one wire format (OpenAI
//! `/chat/completions`) for all 174 catalog entries, which silently breaks every
//! non-OpenAI-native endpoint. This module defines the seam: a `ProviderProtocol`
//! turns the engine's `Message` history into a provider request and parses the
//! provider reply back into text + native tool calls + usage. Concrete protocols
//! live in sibling modules (`openai`, `anthropic`, `gemini`).
//!
//! Keeping it a plain trait (not async) matches the blocking socket transport and
//! lets each adapter be unit-tested with canned JSON — no network needed.

use crate::{Message, ToolCall};
use serde_json::Value;

/// One `(name, value)` header pair sent verbatim on the request line set.
pub type Header<'a> = (&'a str, &'a str);

/// Token accounting reported by a provider for one completion.
#[derive(Debug, Clone, Default)]
pub struct Usage {
    pub prompt_tokens: usize,
    pub completion_tokens: usize,
    pub total_tokens: usize,
    pub cached_prompt_tokens: usize,
}

/// A parsed assistant turn: prose content, any native tool calls, and usage.
/// `thinking` carries reasoning text when the provider exposes it separately
/// (Anthropic `thinking` blocks, Gemini thoughts) so the UI can show it.
#[derive(Debug, Clone, Default)]
pub struct ParsedReply {
    pub content: String,
    pub thinking: Option<String>,
    pub tool_calls: Vec<ToolCall>,
    pub usage: Option<Usage>,
}

/// Everything an adapter needs to build one request. The registry supplies these
/// from the resolved provider config; adapters must not read global env.
#[derive(Debug, Clone)]
pub struct RequestCtx<'a> {
    /// Model id to send (already stripped of any `provider/` routing prefix).
    pub model: &'a str,
    /// Resolved secret (API key or OAuth token); empty ⇒ unauthenticated/local.
    pub key: &'a str,
    /// Sampling temperature.
    pub temperature: f32,
    /// Hard cap on generated tokens.
    pub max_tokens: u32,
    /// Reasoning effort ("low"|"medium"|"high"|"max"|"" = disabled).
    pub effort: &'a str,
    /// Whether to stream (SSE) rather than return one JSON body.
    pub stream: bool,
    /// OpenAI-shaped tool definitions this provider may call (empty ⇒ none).
    pub tools: &'a [Value],
    /// System prompt already rendered (catalog + protocol guidance).
    pub system: &'a str,
}

/// One provider's wire protocol. Implementations are stateless translators.
pub trait ProviderProtocol: Send + Sync {
    /// Stable identifier, e.g. "openai", "anthropic", "gemini".
    fn name(&self) -> &'static str;

    /// Absolute path (starting with `/`) for the chat-completions call, given the
    /// provider base URL. Adapters own their endpoint shape.
    fn request_path(&self, base_url: &str) -> String;

    /// Build the JSON request body for a full (non-streaming) call.
    fn build_body(&self, ctx: &RequestCtx, history: &[Message]) -> Value;

    /// Extra headers beyond `Authorization: Bearer` (auth scheme differences).
    /// Return `(name, value)` pairs; an `authorization` here overrides bearer.
    fn headers(&self, ctx: &RequestCtx) -> Vec<(String, String)> {
        let _ = ctx;
        Vec::new()
    }

    /// Parse a complete non-streaming response body.
    fn parse_reply(&self, raw: &str) -> Result<ParsedReply, String>;

    /// Parse one SSE `data:` payload incrementally, accumulating into `acc`.
    /// Returns false when the stream signals completion (e.g. `[DONE]`).
    fn parse_stream_event(&self, data: &str, acc: &mut StreamAcc) -> bool;

    /// True if this protocol uses SSE framing for streaming responses.
    fn streams_via_sse(&self) -> bool {
        true
    }
}

/// Accumulator a streaming adapter stitches deltas into. Generic enough for all
/// three protocols; each fills the fields it has and finalizes via `into_parsed`.
#[derive(Debug, Default)]
pub struct StreamAcc {
    pub content: String,
    pub thinking: String,
    /// index → (name, arguments-so-far) for OpenAI-style streamed tool calls.
    pub calls: std::collections::BTreeMap<usize, (String, String)>,
    /// Anthropic/Gemini accumulate whole tool-use blocks keyed by id/index.
    pub block_tools: Vec<ToolCall>,
    pub usage: Option<Usage>,
    pub done: bool,
}

impl StreamAcc {
    /// Collapse accumulated deltas into a final `ParsedReply`.
    pub fn into_parsed(mut self) -> ParsedReply {
        // Prefer explicitly-built whole blocks; else stitch indexed fragments.
        let mut tool_calls = std::mem::take(&mut self.block_tools);
        if tool_calls.is_empty() {
            tool_calls = self
                .calls
                .iter_mut()
                .filter(|(_, (n, _))| !n.is_empty())
                .map(|(_, (n, args))| {
                    let args = serde_json::from_str::<Value>(args).unwrap_or_else(|_| Value::Null);
                    ToolCall { name: n.clone(), args }
                })
                .collect();
        }
        ParsedReply {
            content: self.content,
            thinking: if self.thinking.is_empty() { None } else { Some(self.thinking) },
            tool_calls,
            usage: self.usage.take(),
        }
    }
}

/// Strip a leading `provider/` routing prefix some catalogs use (e.g.
/// `groq/llama-3.3-70b` → `llama-3.3-70b`) while leaving genuine slashes alone.
pub fn strip_model_prefix(model: &str, provider_id: &str) -> String {
    let prefix = format!("{provider_id}/");
    if let Some(rest) = model.strip_prefix(&prefix) {
        rest.to_string()
    } else {
        model.to_string()
    }
}
