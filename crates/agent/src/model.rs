//! A live `Model` backed by an OpenAI-compatible HTTP endpoint (OmniRoute on
//! loopback :20128), with zero external crates: just `std::net` + `serde_json`.
//! Loopback is plain HTTP (no TLS), which keeps the whole agent offline-capable
//! and card-free.
//!
//! Tool-use is negotiated natively via the OpenAI `tools` array when the backend
//! supports function-calling, with a TEXT protocol fallback (the model replies
//! with a bare or fenced JSON object `{"tool":"name","args":{...}}`) for weaker
//! auto-fallback models. Both paths feed the same Gateway/policy/audit chain.

use crate::{Message, Step, ToolCall};
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

/// Where the model gateway lives. Loaded from ONYX_* env by `omniroute_config`.
#[derive(Debug, Clone)]
pub struct GatewayCfg {
    pub host: String,
    pub port: u16,
    pub base: String, // e.g. http://127.0.0.1:20128/v1
    pub key: String,
    pub model: String,
}

/// Minimal HTTP/1.1 request over a blocking socket. Returns the response body.
/// Handles `Transfer-Encoding: chunked` and surfaces non-2xx as an error string.
pub fn http_request(
    method: &str,
    host: &str,
    port: u16,
    path: &str,
    auth: Option<&str>,
    body: Option<&str>,
) -> Result<String, String> {
    if port == 443 {
        return http_request_tls(method, host, port, path, auth, body);
    }
    let addr = (host, port)
        .to_socket_addrs()
        .map_err(|e| format!("resolve {host}:{port}: {e}"))?
        .next()
        .ok_or_else(|| "no address".to_string())?;
    let mut sock = TcpStream::connect_timeout(&addr, Duration::from_secs(15))
        .map_err(|e| format!("connect {host}:{port}: {e}"))?;
    sock.set_write_timeout(Some(Duration::from_secs(30))).ok();
    sock.set_read_timeout(Some(Duration::from_secs(120))).ok();

    let mut req = String::new();
    req.push_str(&format!("{method} {path} HTTP/1.1\r\nHost: {host}\r\nAccept: application/json\r\n"));
    if let Some(a) = auth {
        req.push_str(&format!("Authorization: Bearer {a}\r\n"));
    }
    match body {
        Some(b) => {
            req.push_str("Content-Type: application/json\r\n");
            req.push_str(&format!("Content-Length: {}\r\n", b.as_bytes().len()));
        }
        None => {
            req.push_str("Content-Length: 0\r\n");
        }
    }
    req.push_str("Connection: close\r\n\r\n");

    sock.write_all(req.as_bytes()).map_err(|e| format!("write hdr: {e}"))?;
    if let Some(b) = body {
        sock.write_all(b.as_bytes()).map_err(|e| format!("write body: {e}"))?;
    }
    sock.flush().ok();

    let mut raw = Vec::new();
    let mut tmp = [0u8; 8192];
    loop {
        match sock.read(&mut tmp) {
            Ok(0) => break,
            Ok(n) => raw.extend_from_slice(&tmp[..n]),
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => break, // timeout: parse what arrived
        }
    }
    parse_response(&raw)
}

/// Fallback / TLS request runner via curl
pub fn http_request_tls(
    method: &str,
    host: &str,
    port: u16,
    path: &str,
    auth: Option<&str>,
    body: Option<&str>,
) -> Result<String, String> {
    let scheme = if port == 443 { "https" } else { "http" };
    let url = if (port == 443 && scheme == "https") || (port == 80 && scheme == "http") {
        format!("{scheme}://{host}{path}")
    } else {
        format!("{scheme}://{host}:{port}{path}")
    };
    let mut cmd = std::process::Command::new("curl");
    cmd.args(["-s", "-S", "-X", method, &url]);
    cmd.args(["-H", "accept: application/json"]);
    cmd.args(["-H", "content-type: application/json"]);
    if let Some(a) = auth {
        cmd.args(["-H", &format!("authorization: Bearer {a}")]);
    }
    if let Some(b) = body {
        cmd.args(["-d", b]);
    }
    let output = cmd.output().map_err(|e| format!("curl execution failed: {e}"))?;
    if !output.status.success() {
        return Err(format!("curl error: {}", String::from_utf8_lossy(&output.stderr)));
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

fn parse_response(raw: &[u8]) -> Result<String, String> {
    let sep = b"\r\n\r\n";
    let idx = find_subslice(raw, sep).ok_or_else(|| "malformed response (no header/body split)".to_string())?;
    let head = String::from_utf8_lossy(&raw[..idx]).to_string();
    let body = &raw[idx + sep.len()..];

    let status_line = head.lines().next().unwrap_or("");
    let code: u16 = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|c| c.parse().ok())
        .unwrap_or(0);
    let lower = head.to_ascii_lowercase();
    let decoded_bytes = if lower.contains("transfer-encoding: chunked") {
        decode_chunked(body)
    } else {
        body.to_vec()
    };
    let decoded = String::from_utf8_lossy(&decoded_bytes).into_owned();
    if !(200..300).contains(&code) {
        return Err(format!("http {code}: {}", decoded.chars().take(200).collect::<String>()));
    }
    Ok(decoded)
}

fn find_subslice(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || hay.len() < needle.len() {
        return None;
    }
    hay.windows(needle.len()).position(|w| w == needle)
}

/// Byte-safe chunked transfer decoding (chunk sizes are byte counts).
fn decode_chunked(mut body: &[u8]) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::new();
    loop {
        let nl = match find_subslice(body, b"\r\n") {
            Some(i) => i,
            None => break,
        };
        let line = String::from_utf8_lossy(&body[..nl]);
        let size = match usize::from_str_radix(line.trim(), 16) {
            Ok(s) => s,
            Err(_) => break,
        };
        if size == 0 {
            break;
        }
        let start = nl + 2;
        let end = start + size;
        if body.len() < end {
            out.extend_from_slice(&body[start..]);
            break;
        }
        out.extend_from_slice(&body[start..end]);
        // advance past chunk data + its trailing CRLF
        body = &body[end..];
        if let Some(stripped) = strip_prefix_bytes(body, b"\r\n") {
            body = stripped;
        }
    }
    out
}

fn strip_prefix_bytes<'a>(hay: &'a [u8], pre: &[u8]) -> Option<&'a [u8]> {
    if hay.len() >= pre.len() && &hay[..pre.len()] == pre {
        Some(&hay[pre.len()..])
    } else {
        None
    }
}

/// Discover the model gateway coordinates from ONYX_* env or active Provider in ~/.onyx/providers.json.
pub fn omniroute_config() -> GatewayCfg {
    // 1. If explicit environment variables are set, prioritize them
    if let Ok(base) = std::env::var("ONYX_BASE_URL").or_else(|_| std::env::var("TEK_BASE_URL")) {
        let key = std::env::var("ONYX_API_KEY")
            .or_else(|_| std::env::var("TEK_API_KEY"))
            .unwrap_or_default();
        let model = std::env::var("ONYX_MODEL")
            .or_else(|_| std::env::var("TEK_MODEL"))
            .unwrap_or_else(|_| "auto".to_string());
        let (host, port) = parse_host_port(&base);
        return GatewayCfg { host, port, base, key, model };
    }

    // 2. Read from active provider in ~/.onyx/providers.json
    let storage = crate::provider::load_storage();
    let active_id = &storage.active_provider;
    let cfg = storage.providers.get(active_id);
    let spec = crate::provider::find_spec(active_id).unwrap_or_else(|| {
        crate::provider::ProviderSpec {
            id: active_id.clone(),
            name: active_id.clone(),
            base_url: Some("http://127.0.0.1:20128/v1".to_string()),
            env_var: Some("OPENAI_API_KEY".to_string()),
            auth_type: "bearer".to_string(),
            streaming_format: "sse".to_string(),
            model_prefixes: vec![],
            model_count: 0,
            function_calling: true,
            vision: true,
            reasoning: true,
        }
    });

    let base = crate::provider::resolve_base_url(&spec, cfg);
    let key = crate::provider::resolve_api_key(&spec, cfg).unwrap_or_default();
    let model = cfg
        .and_then(|c| c.active_model.clone())
        .or_else(|| std::env::var("ONYX_MODEL").ok())
        .unwrap_or_else(|| "gpt-oss-120b-medium".to_string());

    let (host, port) = parse_host_port(&base);
    GatewayCfg { host, port, base, key, model }
}

impl GatewayCfg {
    pub fn host_port_from(&self, base: &str) -> (String, u16) {
        parse_host_port(base)
    }
}

pub fn parse_host_port(base: &str) -> (String, u16) {
    let is_https = base.starts_with("https://");
    let default_port = if is_https { 443 } else if base.starts_with("http://") { 80 } else { 20128 };
    let after = base.split("://").nth(1).unwrap_or(base);
    let authority = after.split('/').next().unwrap_or(after);
    let mut parts = authority.rsplitn(2, ':');
    match (parts.next(), parts.next()) {
        (Some(p), Some(h)) if !p.is_empty() => (h.to_string(), p.parse().unwrap_or(default_port)),
        _ => (authority.to_string(), default_port),
    }
}

/// Determine the model's native context window limit.
/// Explicit ONYX_CONTEXT_TOKENS overrides; otherwise detects based on model name
/// and known model architectures (from 12k up to 1M+ tokens).
pub fn context_window_for_model(model_name: &str) -> usize {
    if let Ok(v) = std::env::var("ONYX_CONTEXT_TOKENS").or_else(|_| std::env::var("TEK_CONTEXT_TOKENS")) {
        if let Ok(n) = v.parse::<usize>() {
            return n;
        }
    }
    let m = model_name.to_lowercase();
    if m.contains("1m") || m.contains("1000k") || m.contains("1250k") {
        1_000_000
    } else if m.contains("nemotron-3.5-lightning:free") {
        1_000_000
    } else if m.contains("nemotron-3.5-lightning") || m.contains("nemotron-3-nano") {
        256_000
    } else if m.contains("gemini") {
        if m.contains("flash") || m.contains("pro") || m.contains("1.5") || m.contains("2.0") {
            1_000_000
        } else {
            128_000
        }
    } else if m.contains("claude") {
        200_000
    } else if m.contains("qwen") || m.contains("qwq") {
        131_072
    } else if m.contains("deepseek") {
        64_000
    } else if m.contains("gpt-4") || m.contains("gpt-4o") || m.contains("o1") || m.contains("o3") {
        128_000
    } else if m.contains("llama-3") {
        128_000
    } else if m.contains("mistral") || m.contains("mixtral") {
        32_768
    } else {
        // Safe conservative default for unspecified models
        64_000
    }
}

pub fn default_context_budget() -> usize {
    let model = std::env::var("ONYX_MODEL").unwrap_or_default();
    context_window_for_model(&model)
}

/// The tool surface for one turn, projected from the full catalog by retrieval.
/// Both fields describe the same set: `catalog` is what the model reads, `defs` is
/// what it may call, and a mismatch means a tool the model was told about but
/// cannot invoke (or vice versa).
#[derive(Clone, Debug)]
pub struct TurnTools {
    /// `  - name: description\n` lines for the retrieved subset.
    pub catalog: String,
    /// OpenAI-shaped definitions for exactly the tools in `catalog`.
    pub defs: Vec<Value>,
    /// Retrieved skill bodies to append after the tool catalog — instructions, so
    /// they belong with the system prompt rather than as a synthetic user message.
    pub skills: String,
    /// Project context string dynamically added to this turn.
    pub project_context: String,
}

impl TurnTools {
    /// Add a tool definition dynamically to this turn.
    pub fn add_tool(&mut self, name: &str, description: &str, schema: Value) {
        if self.offered_names().iter().any(|n| n == name) {
            return;
        }
        self.catalog.push_str(&format!("  - {name}: {description}\n"));
        self.defs.push(json!({
            "type": "function",
            "function": {
                "name": name,
                "description": description,
                "parameters": schema,
            }
        }));
    }

    /// Add a skill instruction dynamically to this turn.
    pub fn add_skill(&mut self, name: &str, description: &str, body: &str) {
        let tag = format!("[Retrieved skill: {name}]");
        if !self.skills.contains(&tag) {
            self.skills.push_str(&format!("\n\n[Retrieved skill: {name}]\n{description}\n{body}"));
        }
    }
    /// The names of exactly the tools this turn offers, in ranked order. The agent
    /// loop uses this so its own tool list agrees with what the model was shown.
    pub fn offered_names(&self) -> Vec<String> {
        self.defs
            .iter()
            .filter_map(|d| d["function"]["name"].as_str().map(|s| s.to_string()))
            .collect()
    }

    /// One-line summary for the TUI status row: how many offered, and any skills
    /// unfolded into the prompt.
    pub fn summary(&self) -> String {
        let n = self.defs.len();
        if self.skills.trim().is_empty() {
            format!("retrieval: offering {n} tools")
        } else {
            format!("retrieval: offering {n} tools + skill instructions")
        }
    }
}

/// The live model. Constructed with the gateway coordinates + a rendered tool
/// catalog (name: description lines) used to teach the model what it may call.
#[derive(Clone)]
pub struct HttpModel {
    pub host: String,
    pub port: u16,
    pub path: String,
    pub model: String,
    pub key: String,
    pub temperature: f32,
    pub system: String,
    pub catalog: String,
    /// Token budget for the whole request; histories estimated above it are
    /// compacted (see crate::context) before sending. 0 disables compaction.
    pub context_budget: usize,
    /// OpenAI-style `tools` array (from tools::tool_schemas); empty = text protocol only.
    pub tool_defs: Vec<Value>,
    /// Reasoning effort ("low", "medium", "high", or "" for disabled)
    pub effort: String,
    /// Last reported token usage from the server
    pub last_usage: Option<crate::TokenUsage>,
    /// Per-turn tool projection (see crate::retrieval). When set, this turn's
    /// system prompt and `tools` array come from here instead of from `catalog` /
    /// `tool_defs`, which stay the full session catalog. Keeping the two separate is
    /// what lets retrieval shrink one turn without permanently narrowing the
    /// session — the next turn can retrieve a different subset, and `/tools` can
    /// still report what the whole catalog holds.
    pub turn_tools: Option<TurnTools>,
    /// Resolved provider endpoint (protocol + auth plan). When present, requests
    /// route through the provider's own wire protocol instead of the hardcoded
    /// OpenAI-compat path. `None` keeps the legacy OpenAI behavior intact.
    pub endpoint: Option<crate::provider::Resolved>,
}

impl HttpModel {
    #[allow(clippy::too_many_arguments)]
    pub fn new(host: String, port: u16, model: String, key: String, catalog: String) -> Self {
        Self {
            host,
            port,
            path: "/v1/chat/completions".to_string(),
            model,
            key,
            temperature: 0.1,
            system: "You are onyx, a precise agent that acts through tools.".to_string(),
            catalog,
            context_budget: default_context_budget(),
            tool_defs: Vec::new(),
            turn_tools: None,
            effort: "medium".to_string(),
            last_usage: None,
            endpoint: None,
        }
    }

    /// Resolve the active provider's wire protocol for a base URL + key. Falls
    /// back to `None` (legacy OpenAI-compat) if the spec can't be determined, so
    /// a bad catalog entry never blocks sending a request.
    fn resolve_endpoint(base_url: &str, key: &str) -> Option<crate::provider::Resolved> {
        let active_id = crate::provider::load_storage().active_provider;
        let spec = crate::provider::find_spec(&active_id)?;
        crate::provider::resolve(&spec, base_url, key).ok()
    }

    /// Re-resolve the wire protocol after the host/key changed (e.g. `/provider`
    /// switch or hot-swap). Keeps `host`/`port` in sync with the resolved endpoint
    /// so TLS routing (`use_tls`) matches where requests actually go. A failed
    /// resolution clears the endpoint, falling back to legacy OpenAI-compat.
    pub fn set_endpoint_for(&mut self, base_url: &str, key: &str) {
        match Self::resolve_endpoint(base_url, key) {
            Some(ep) => {
                self.host = ep.host.clone();
                self.port = ep.port;
                self.endpoint = Some(ep);
            }
            None => self.endpoint = None,
        }
    }

    /// Build from GatewayCfg; the chat path is derived from the base URL's path.
    pub fn from_cfg(cfg: &GatewayCfg, catalog: String, tool_defs: Vec<Value>) -> Self {
        let after = cfg.base.split("://").nth(1).unwrap_or(&cfg.base);
        let path_part = after.splitn(2, '/').nth(1).unwrap_or("v1");
        let path = format!("/{}/chat/completions", path_part.trim_end_matches('/'));
        let endpoint = Self::resolve_endpoint(&cfg.base, &cfg.key);
        Self {
            host: cfg.host.clone(),
            port: cfg.port,
            path,
            model: cfg.model.clone(),
            key: cfg.key.clone(),
            temperature: 0.1,
            system: "You are onyx, a precise agent that acts through tools.".to_string(),
            catalog,
            context_budget: default_context_budget(),
            tool_defs,
            turn_tools: None,
            effort: "medium".to_string(),
            last_usage: None,
            endpoint,
        }
    }

    /// One-shot completion for non-agent callers (e.g. the guardian judge):
    /// post a ready-made chat-completions body, return `choices[0].message.content`.
    pub fn chat_raw(&self, body_json: &str) -> Result<String, String> {
        let auth = if self.key.is_empty() { None } else { Some(self.key.as_str()) };
        let raw = http_request("POST", &self.host, self.port, &self.path, auth, Some(body_json))?;
        let v: Value = serde_json::from_str(&raw).map_err(|e| format!("parse reply: {e}"))?;
        Ok(v
            .get("choices")
            .and_then(|c| c.get(0))
            .and_then(|c| c.get("message"))
            .and_then(|m| m.get("content"))
            .and_then(|c| c.as_str())
            .unwrap_or("")
            .to_string())
    }

    /// List models available on the gateway (`/v1/models`).
    pub fn list_models(&self) -> Result<Vec<String>, String> {
        let auth = if self.key.is_empty() { None } else { Some(self.key.as_str()) };
        let models_path = if self.path.contains("/chat/completions") {
            self.path.replace("/chat/completions", "/models")
        } else {
            "/v1/models".to_string()
        };
        let raw = http_request("GET", &self.host, self.port, &models_path, auth, None)?;
        let v: Value = serde_json::from_str(&raw).map_err(|e| format!("parse models: {e}"))?;
        let mut ids = Vec::new();
        if let Some(arr) = v.get("data").and_then(|d| d.as_array()) {
            for m in arr {
                if let Some(id) = m.get("id").and_then(|i| i.as_str()) {
                    ids.push(id.to_string());
                }
            }
        }
        Ok(ids)
    }

    /// The tool-calling instructions, chosen by whether this turn offers function
    /// definitions at all. With none, the model has to emit the JSON object form and
    /// needs the terminate contract spelled out; with definitions, function calling is
    /// primary and the JSON form is only a fallback.
    fn protocol_for(has_tools: bool) -> &'static str {
        if has_tools {
            "\n\nCall tools via function calling. Batch independent calls in one turn when possible. When finished, call `terminate` with status and the final answer. If a backend ignores function calling, fall back to the JSON object form {\"tool\":\"<name>\",\"args\":{...}}."
        } else {
            "\n\nTo call a tool, reply with ONLY a JSON object, no prose:\n  {\"tool\":\"<name>\",\"args\":{...}}\nUse args exactly as the tool needs. When you are finished, call the terminate tool:\n  {\"tool\":\"terminate\",\"args\":{\"status\":\"success\",\"content\":\"<final answer for the user>\"}}\nDo not invent tools. Do not emit more than one JSON object per turn.\nIf the request needs SEVERAL tools, call them one per turn and keep going until you have run EVERY tool the request asks for and gathered each result. Do NOT call terminate until all requested tool work is done; the terminate content must report every result."
        }
    }

    /// The effective (catalog, defs) pair for the request about to be built: the
    /// retrieval projection when one is set for this turn, otherwise the full session
    /// catalog. One accessor so `build_messages`, `rendered_system` and the body
    /// builders can never disagree about which tools the model was offered.
    fn active_tools(&self) -> (&str, &[Value]) {
        match &self.turn_tools {
            Some(t) => (t.catalog.as_str(), t.defs.as_slice()),
            None => (self.catalog.as_str(), self.tool_defs.as_slice()),
        }
    }

    fn build_messages(&self, history: &[Message]) -> Vec<Value> {
        // Compact the history first when it's estimated to exceed our budget;
        // compaction is a no-op (returns None) for normal conversations.
        let compacted = if self.context_budget > 0 {
            crate::context::compact_history(history, self.context_budget)
        } else {
            None
        };
        let history: &[Message] = match &compacted {
            Some(c) => c.as_slice(),
            None => history,
        };
        let (catalog, defs) = self.active_tools();
        let protocol = Self::protocol_for(!defs.is_empty());
        let sys = format!(
            "{base}\n\nAvailable tools:\n{catalog}{protocol}{skills}",
            base = self.system,
            skills = self.turn_tools.as_ref().map(|t| t.skills.as_str()).unwrap_or("")
        );
        let mut msgs = vec![json!({ "role": "system", "content": sys })];
        for m in history {
            match m.role.as_str() {
                "system" => {} // we inject our own system
                "user" => msgs.push(json!({ "role": "user", "content": m.content })),
                "assistant" => {
                    let c = if m.content.is_empty() { "(calling a tool)" } else { &m.content };
                    msgs.push(json!({ "role": "assistant", "content": c }));
                }
                "tool" => msgs.push(json!({ "role": "user", "content": format!("OBSERVATION (tool result): {}", m.content) })),
                _ => msgs.push(json!({ "role": "user", "content": m.content })),
            }
        }
        msgs
    }

    /// The system prompt with the tool catalog + protocol guidance appended. The
    /// adapters take this verbatim (OpenAI as a `system` message, Anthropic as the
    /// top-level `system`, Gemini as `systemInstruction`).
    fn rendered_system(&self) -> String {
        let (catalog, defs) = self.active_tools();
        format!(
            "{base}\n\nAvailable tools:\n{catalog}{protocol}{skills}",
            base = self.system,
            protocol = Self::protocol_for(!defs.is_empty()),
            skills = self.turn_tools.as_ref().map(|t| t.skills.as_str()).unwrap_or("")
        )
    }

    fn max_tokens_cfg() -> u32 {
        std::env::var("ONYX_MAX_TOKENS")
            .or_else(|_| std::env::var("TEK_MAX_TOKENS"))
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(4096)
    }

    /// Build the request context the resolved adapter needs for one call.
    fn make_ctx<'a>(&'a self, stream: bool) -> crate::provider::RequestCtx<'a> {
        crate::provider::RequestCtx {
            model: &self.model,
            key: &self.key,
            temperature: self.temperature,
            max_tokens: Self::max_tokens_cfg(),
            effort: &self.effort,
            stream,
            tools: self.active_tools().1,
            system: "", // filled below via owned string
        }
    }

    /// Record usage reported by an adapter into `last_usage`.
    fn apply_usage(&mut self, u: Option<crate::provider::Usage>) {
        if let Some(u) = u {
            self.last_usage = Some(crate::TokenUsage {
                prompt_tokens: u.prompt_tokens,
                completion_tokens: u.completion_tokens,
                total_tokens: u.total_tokens,
                cached_prompt_tokens: u.cached_prompt_tokens,
            });
        }
    }

    /// Turn a parsed reply (from either protocol path) into a Step.
    fn step_from_parsed(&mut self, p: crate::provider::ParsedReply) -> Step {
        self.apply_usage(p.usage);
        if p.tool_calls.is_empty() && p.content.trim().is_empty() {
            return Step::Final("[model returned empty content]".to_string());
        }
        self.to_step(&p.content, p.tool_calls)
    }

    /// Non-streaming call routed through the resolved provider protocol.
    fn next_step_protocol(&mut self, history: &[Message]) -> Step {
        let ep = match &self.endpoint {
            Some(ep) => ep,
            None => return Step::Final("[no endpoint]".to_string()),
        };
        let sys = self.rendered_system();
        let mut ctx = self.make_ctx(false);
        ctx.system = &sys;
        let body = ep.protocol.build_body(&ctx, history);
        let body_str = body.to_string();
        let (bearer, hdrs) = ep.headers_for(&ctx);
        let extra: Vec<crate::provider::Header> =
            hdrs.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        match crate::provider::transport::request(
            "POST",
            &ep.host,
            ep.port,
            &ep.path,
            bearer.as_deref(),
            &extra,
            Some(&body_str),
        ) {
            Ok(raw) => match ep.protocol.parse_reply(&raw) {
                Ok(p) => self.step_from_parsed(p),
                Err(e) => Step::Final(format!("[model parse error: {e}]")),
            },
            Err(e) => Step::Final(format!("[model http error: {e}]")),
        }
    }

    /// Streaming call routed through the resolved provider protocol. Answer-text
    /// deltas go to `on_delta`; reasoning/thinking deltas go to `on_reasoning` as
    /// they arrive, so a UI can show them live instead of dumping them at the end.
    /// Tool calls + usage are stitched by the adapter's `parse_stream_event`.
    fn next_step_streaming_protocol(
        &mut self,
        history: &[Message],
        on_delta: &mut dyn FnMut(&str),
        on_reasoning: &mut dyn FnMut(&str),
    ) -> Step {
        let ep = match &self.endpoint {
            Some(ep) => ep,
            None => return Step::Final("[no endpoint]".to_string()),
        };
        if !ep.protocol.streams_via_sse() {
            // Protocol without SSE framing (e.g. ndjson local): fall back to one
            // complete body and emit its content as a single delta.
            let step = self.next_step_protocol(history);
            if let Step::Final(text) = &step {
                on_delta(text);
            }
            return step;
        }
        let sys = self.rendered_system();
        let mut ctx = self.make_ctx(true);
        ctx.system = &sys;
        let body = ep.protocol.build_body(&ctx, history);
        let body_str = body.to_string();
        let (bearer, hdrs) = ep.headers_for(&ctx);
        let extra: Vec<crate::provider::Header> =
            hdrs.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();

        let mut acc = crate::provider::StreamAcc::default();
        let proto = ep.protocol.clone();
        let result = crate::provider::transport::sse(
            "POST",
            &ep.host,
            ep.port,
            &ep.path,
            bearer.as_deref(),
            &extra,
            Some(&body_str),
            "text/event-stream",
            &mut |data| proto.parse_stream_event(data, &mut acc, on_reasoning),
        );
        if let Err(e) = result {
            return Step::Final(format!("[model stream error: {e}]"));
        }
        let parsed = acc.into_parsed();
        // Reasoning already streamed live via on_reasoning; do NOT re-emit it into
        // the answer tail. Only the visible answer text goes to on_delta.
        on_delta(&parsed.content);
        self.step_from_parsed(parsed)
    }

    // Pull the FIRST well-formed JSON object out of a possibly chatty or
    // multi-object reply. Brace-matched and string/escape aware, so a model that
    // emits several `{"tool":...}` objects (or wraps one in prose) still yields
    // one parseable call — the loop runs it, feeds the result back, and the model
    // emits the next. The old first-`{`-to-last-`}` slice broke on this.
    fn extract_json(text: &str) -> Option<Value> {
        let trimmed = text.trim();
        if let Ok(v) = serde_json::from_str::<Value>(trimmed) {
            return Some(v);
        }
        let bytes = trimmed.as_bytes();
        let mut depth = 0usize;
        let mut in_str = false;
        let mut esc = false;
        let mut start: Option<usize> = None;
        for i in 0..bytes.len() {
            let c = bytes[i];
            if in_str {
                if esc {
                    esc = false;
                } else if c == b'\\' {
                    esc = true;
                } else if c == b'"' {
                    in_str = false;
                }
                continue;
            }
            match c {
                b'"' => in_str = true,
                b'{' => {
                    if depth == 0 {
                        start = Some(i);
                    }
                    depth += 1;
                }
                b'}' if depth > 0 => {
                    depth -= 1;
                    if depth == 0 {
                        if let Some(s) = start {
                            // s..i are ASCII structural boundaries → byte slice is safe
                            if let Ok(v) = serde_json::from_str::<Value>(&trimmed[s..=i]) {
                                return Some(v);
                            }
                            start = None;
                        }
                    }
                }
                _ => {}
            }
        }
        None
    }

    /// Helper to validate tool name conforms to identifier rules `^[a-zA-Z0-9_-]+$`.
    pub fn is_valid_tool_name(name: &str) -> bool {
        !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    }

    fn to_step(&self, content: &str, native_tools: Vec<ToolCall>) -> Step {
        if !native_tools.is_empty() {
            let mut valid_tools = Vec::new();
            for tc in native_tools {
                if Self::is_valid_tool_name(&tc.name) && tc.args.is_object() {
                    valid_tools.push(tc);
                } else {
                    return Step::Final(format!("[model error: malformed tool call `{}` with invalid name or arguments]", tc.name));
                }
            }
            if !valid_tools.is_empty() {
                return Step::Calls(valid_tools);
            }
        }
        match Self::extract_json(content) {
            Some(v) => {
                let name = v.get("tool").and_then(|s| s.as_str()).unwrap_or("");
                if !name.is_empty() {
                    if !Self::is_valid_tool_name(name) {
                        return Step::Final(format!("[model error: malformed fallback tool call with invalid tool name `{name}`]"));
                    }
                    let args = v.get("args").cloned().unwrap_or_else(|| json!({}));
                    if !args.is_object() {
                        return Step::Final(format!("[model error: malformed fallback tool call `{name}` arguments must be a JSON object]"));
                    }
                    return Step::Calls(vec![ToolCall { name: name.to_string(), args }]);
                }
                if let Some(f) = v.get("final").and_then(|s| s.as_str()) {
                    return Step::Final(f.to_string());
                }
                // JSON without a recognizable shape → treat whole text as answer
                Step::Final(content.trim().to_string())
            }
            None => {
                let trimmed = content.trim();
                // Check if the model emitted malformed pseudo-call like `→ list_dir(...)`, `→ ask_human(...)` or `→ ask_humanquestion":"...`
                if let Some(rest) = trimmed.strip_prefix("→ ") {
                    let rest = rest.trim();
                    // Pattern 1: Standard function syntax `tool_name(...)`
                    if let Some(pp) = rest.find('(') {
                        let cand_name = rest[..pp].trim();
                        let after_paren = &rest[pp + 1..];
                        if let Some(cp) = after_paren.rfind(')') {
                            let inside = after_paren[..cp].trim();
                            if Self::is_valid_tool_name(cand_name) {
                                if let Ok(args) = serde_json::from_str::<Value>(inside) {
                                    if args.is_object() {
                                        return Step::Calls(vec![ToolCall { name: cand_name.to_string(), args }]);
                                    }
                                }
                            }
                        }
                    }

                    // Pattern 2: Stream-corrupted syntax like `ask_humanquestion":"...`
                    for key in ["question", "content", "command", "query"] {
                        let marker = format!("{key}\":\"");
                        if let Some(idx) = rest.find(&marker) {
                            let prefix = rest[..idx].trim_end_matches(|c: char| {
                                c == '(' || c == '{' || c == '[' || c == '"' || c == '\'' || c.is_whitespace()
                            });
                            if !prefix.is_empty() && Self::is_valid_tool_name(prefix) {
                                let val_start = idx + marker.len();
                                if let Some(val_end) = rest[val_start..].rfind('\"') {
                                    let extracted_val = &rest[val_start..val_start + val_end];
                                    let args = json!({ key: extracted_val });
                                    return Step::Calls(vec![ToolCall { name: prefix.to_string(), args }]);
                                }
                            }
                        }
                    }
                }
                Step::Final(trimmed.to_string())
            }
        }
    }

    /// Parse a full (non-streaming) chat-completions reply into a Step.
    fn step_from_reply(&mut self, raw: &str) -> Step {
        let v: Value = match serde_json::from_str(raw) {
            Ok(v) => v,
            Err(e) => return Step::Final(format!("[model parse error: {e}]")),
        };
        self.step_from_value(&v)
    }

    fn step_from_value(&mut self, v: &Value) -> Step {
        let choice = v.get("choices").and_then(|c| c.get(0));
        let msg = choice.and_then(|c| c.get("message"));

        // ALL native tool_calls this turn, not just the first
        let native: Vec<ToolCall> = msg
            .and_then(|m| m.get("tool_calls"))
            .and_then(|t| t.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|tc| tc.get("function"))
                    .filter_map(|f| {
                        let name = f.get("name").and_then(|n| n.as_str())?.to_string();
                        let argstr = f.get("arguments").and_then(|a| a.as_str()).unwrap_or("{}");
                        let args = serde_json::from_str::<Value>(argstr).unwrap_or_else(|_| json!({}));
                        Some(ToolCall { name, args })
                    })
                    .collect()
            })
            .unwrap_or_default();

        let content = msg
            .and_then(|m| m.get("content"))
            .and_then(|c| c.as_str())
            .unwrap_or("")
            .to_string();

        let (content, _extracted_think) = crate::repetition::strip_think_tags(&content);

        if let Some(u) = v.get("usage") {
            let prompt_tokens = u.get("prompt_tokens").and_then(|p| p.as_u64()).unwrap_or(0) as usize;
            let completion_tokens = u.get("completion_tokens").and_then(|p| p.as_u64()).unwrap_or(0) as usize;
            let total_tokens = u.get("total_tokens").and_then(|p| p.as_u64()).unwrap_or(0) as usize;
            let cached_prompt_tokens = u.get("prompt_tokens_details")
                .and_then(|d| d.get("cached_tokens"))
                .and_then(|c| c.as_u64())
                .unwrap_or(0) as usize;
            self.last_usage = Some(crate::TokenUsage {
                prompt_tokens,
                completion_tokens,
                total_tokens,
                cached_prompt_tokens,
            });
        }
        if native.is_empty() && content.trim().is_empty() {
            return Step::Final("[model returned empty content]".to_string());
        }
        self.to_step(&content, native)
    }
}

impl crate::Model for HttpModel {
    fn next_step(&mut self, history: &[Message], _tools: &[&str]) -> Step {
        if self.endpoint.is_some() {
            return self.next_step_protocol(history);
        }
        let msgs = self.build_messages(history);
        let defs = self.active_tools().1;
        // Thinking models spend most of the budget on reasoning_content and, left
        // uncapped, can return an empty `content`, which ends the tool loop before
        // any tool runs. Give every step room for reasoning + a tool call.
        let max_tokens: u32 = std::env::var("ONYX_MAX_TOKENS")
            .or_else(|_| std::env::var("TEK_MAX_TOKENS"))
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(4096);
        let mut body = json!({
            "model": self.model,
            "messages": msgs,
            "temperature": self.temperature,
            "max_tokens": max_tokens,
            "stream": false,
        });
        if !defs.is_empty() {
            body["tools"] = json!(defs);
            body["tool_choice"] = json!("auto");
        }
        let body_str = body.to_string();
        let auth = if self.key.is_empty() { None } else { Some(self.key.as_str()) };
        match http_request("POST", &self.host, self.port, &self.path, auth, Some(&body_str)) {
            Ok(raw) => self.step_from_reply(&raw),
            Err(e) => Step::Final(format!("[model http error: {e}]")),
        }
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }
}

/// Streaming variant: yields text deltas to `on_delta` while accumulating
/// native tool-call fragments (stitched by index), then returns the final Step.
impl HttpModel {
    pub fn next_step_streaming(
        &mut self,
        history: &[Message],
        _tools: &[&str],
        on_delta: &mut dyn FnMut(&str),
        on_reasoning: &mut dyn FnMut(&str),
    ) -> Step {
        if self.endpoint.is_some() {
            return self.next_step_streaming_protocol(history, on_delta, on_reasoning);
        }
        let msgs = self.build_messages(history);
        let defs = self.active_tools().1.to_vec();
        let max_tokens: u32 = std::env::var("ONYX_MAX_TOKENS")
            .or_else(|_| std::env::var("TEK_MAX_TOKENS"))
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(4096);
        let mut body = json!({
            "model": self.model,
            "messages": msgs,
            "temperature": self.temperature,
            "max_tokens": max_tokens,
            "stream": true,
            "stream_options": { "include_usage": true },
        });
        if !self.effort.is_empty() {
            body["reasoning_effort"] = json!(self.effort);
        }
        if !defs.is_empty() {
            body["tools"] = json!(defs);
            body["tool_choice"] = json!("auto");
        }
        let body_str = body.to_string();
        let auth = if self.key.is_empty() { None } else { Some(self.key.as_str()) };

        let mut content = String::new();
        // index → (name, arguments-so-far)
        let mut calls: std::collections::BTreeMap<usize, (String, String)> =
            std::collections::BTreeMap::new();
        let mut reported_usage: Option<crate::TokenUsage> = None;
        let mut think_filter = crate::repetition::ThinkTagFilter::new();

        let result = http_sse(
            "POST",
            &self.host,
            self.port,
            &self.path,
            auth,
            Some(&body_str),
            &mut |data| {
                if data == "[DONE]" {
                    return false;
                }
                let Ok(v) = serde_json::from_str::<Value>(data) else { return true };
                if let Some(u) = v.get("usage") {
                    let prompt_tokens = u.get("prompt_tokens").and_then(|p| p.as_u64()).unwrap_or(0) as usize;
                    let completion_tokens = u.get("completion_tokens").and_then(|p| p.as_u64()).unwrap_or(0) as usize;
                    let total_tokens = u.get("total_tokens").and_then(|p| p.as_u64()).unwrap_or(0) as usize;
                    let cached_prompt_tokens = u.get("prompt_tokens_details")
                        .and_then(|d| d.get("cached_tokens"))
                        .and_then(|c| c.as_u64())
                        .unwrap_or(0) as usize;
                    reported_usage = Some(crate::TokenUsage {
                        prompt_tokens,
                        completion_tokens,
                        total_tokens,
                        cached_prompt_tokens,
                    });
                }
                let Some(choice) = v.get("choices").and_then(|c| c.get(0)).map(|c| c.clone())
                else { return true };
                let delta = choice.get("delta").cloned().unwrap_or(json!({}));
                let r_opt = delta.get("reasoning_content")
                    .or_else(|| delta.get("reasoning"))
                    .or_else(|| delta.get("thought"))
                    .and_then(|c| c.as_str());
                if let Some(r) = r_opt {
                    if !r.is_empty() {
                        on_reasoning(r);
                    }
                }
                if let Some(txt) = delta.get("content").and_then(|c| c.as_str()) {
                    if !txt.is_empty() {
                        think_filter.push(
                            txt,
                            |clean_chunk| {
                                on_delta(clean_chunk);
                                content.push_str(clean_chunk);
                            },
                            |reasoning_chunk| {
                                on_reasoning(reasoning_chunk);
                            },
                        );
                    }
                }
                if let Some(tcs) = delta.get("tool_calls").and_then(|t| t.as_array()) {
                    for tc in tcs {
                        let idx = tc.get("index").and_then(|i| i.as_u64()).unwrap_or(0) as usize;
                        let e = calls.entry(idx).or_insert_with(|| (String::new(), String::new()));
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
            },
        );

        think_filter.finish(
            |clean_chunk| {
                on_delta(clean_chunk);
                content.push_str(clean_chunk);
            },
            |reasoning_chunk| {
                on_reasoning(reasoning_chunk);
            },
        );

        if let Err(e) = result {
            return Step::Final(format!("[model stream error: {e}]"));
        }
        let native: Vec<ToolCall> = calls
            .into_values()
            .filter(|(name, _)| !name.is_empty())
            .map(|(name, args)| {
                let args = serde_json::from_str::<Value>(&args).unwrap_or_else(|_| json!({}));
                ToolCall { name, args }
            })
            .collect();
        if let Some(u) = reported_usage {
            self.last_usage = Some(u);
        }
        if native.is_empty() && content.trim().is_empty() {
            return Step::Final("[model returned empty content]".to_string());
        }
        self.to_step(&content, native)
    }
}

/// Minimal SSE-over-HTTP: streams `data:` payload strings to `on_data` until it
/// returns false or `[DONE]`. Blocking socket, no crates. Handles both chunked
/// transfer encoding and raw (connection-close) framing.
pub fn http_sse(
    method: &str,
    host: &str,
    port: u16,
    path: &str,
    auth: Option<&str>,
    body: Option<&str>,
    on_data: &mut dyn FnMut(&str) -> bool,
) -> Result<(), String> {
    if port == 443 {
        return http_sse_curl(method, host, port, path, auth, body, on_data);
    }
    let addr = (host, port)
        .to_socket_addrs()
        .map_err(|e| format!("resolve {host}:{port}: {e}"))?
        .next()
        .ok_or_else(|| "no address".to_string())?;
    let mut sock = TcpStream::connect_timeout(&addr, Duration::from_secs(15))
        .map_err(|e| format!("connect {host}:{port}: {e}"))?;
    sock.set_write_timeout(Some(Duration::from_secs(30))).ok();
    sock.set_read_timeout(Some(Duration::from_secs(220))).ok();

    let mut req = String::new();
    req.push_str(&format!("{method} {path} HTTP/1.1\r\nHost: {host}:{port}\r\nAccept: text/event-stream\r\n"));
    if let Some(a) = auth {
        req.push_str(&format!("Authorization: Bearer {a}\r\n"));
    }
    match body {
        Some(b) => {
            req.push_str("Content-Type: application/json\r\n");
            req.push_str(&format!("Content-Length: {}\r\n", b.as_bytes().len()));
        }
        None => req.push_str("Content-Length: 0\r\n"),
    }
    req.push_str("Connection: close\r\n\r\n");
    sock.write_all(req.as_bytes()).map_err(|e| format!("write hdr: {e}"))?;
    if let Some(b) = body {
        sock.write_all(b.as_bytes()).map_err(|e| format!("write body: {e}"))?;
    }
    sock.flush().ok();

    // Read through the header block once.
    let mut buf: Vec<u8> = Vec::with_capacity(8192);
    let mut tmp = [0u8; 8192];
    loop {
        if let Some(split) = find_subslice(&buf, b"\r\n\r\n") {
            let head = String::from_utf8_lossy(&buf[..split]).to_string();
            buf.drain(..split + 4);
            let code: u16 = head
                .lines()
                .next()
                .and_then(|l| l.split_whitespace().nth(1))
                .and_then(|c| c.parse().ok())
                .unwrap_or(0);
            if !(200..300).contains(&code) {
                let err_peek = String::from_utf8_lossy(&buf[..buf.len().min(500)]);
                return Err(format!("http {code}: {}", err_peek.trim()));
            }
            let chunked = head.to_ascii_lowercase().contains("transfer-encoding: chunked");
            return sse_pump(&mut sock, &mut buf, chunked, on_data);
        }
        let n = sock.read(&mut tmp).map_err(|e| format!("read hdr: {e}"))?;
        if n == 0 {
            return Err("eof before headers".to_string());
        }
        buf.extend_from_slice(&tmp[..n]);
    }
}

/// Fallback / TLS SSE stream reader via curl -N
pub fn http_sse_curl(
    method: &str,
    host: &str,
    port: u16,
    path: &str,
    auth: Option<&str>,
    body: Option<&str>,
    on_data: &mut dyn FnMut(&str) -> bool,
) -> Result<(), String> {
    let scheme = if port == 443 { "https" } else { "http" };
    let url = if (port == 443 && scheme == "https") || (port == 80 && scheme == "http") {
        format!("{scheme}://{host}{path}")
    } else {
        format!("{scheme}://{host}:{port}{path}")
    };
    let mut cmd = std::process::Command::new("curl");
    cmd.args(["-N", "-s", "-S", "-X", method, &url]);
    cmd.args(["-H", "accept: text/event-stream"]);
    cmd.args(["-H", "content-type: application/json"]);
    if let Some(a) = auth {
        cmd.args(["-H", &format!("authorization: Bearer {a}")]);
    }
    if let Some(b) = body {
        cmd.args(["-d", b]);
    }
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());

    let mut child = cmd.spawn().map_err(|e| format!("spawn curl SSE: {e}"))?;
    let stdout = child.stdout.take().ok_or_else(|| "no stdout".to_string())?;
    use std::io::BufRead;
    let reader = std::io::BufReader::new(stdout);
    for line in reader.lines() {
        match line {
            Ok(l) => {
                let trimmed = l.trim();
                if let Some(data) = sse_data(trimmed) {
                    if data == "[DONE]" {
                        let _ = child.kill();
                        return Ok(());
                    }
                    if !on_data(&data) {
                        let _ = child.kill();
                        return Ok(());
                    }
                }
            }
            Err(_) => break,
        }
    }
    let _ = child.wait();
    Ok(())
}

fn sse_pump(
    sock: &mut TcpStream,
    buf: &mut Vec<u8>,
    chunked: bool,
    on_data: &mut dyn FnMut(&str) -> bool,
) -> Result<(), String> {
    let mut pending: Vec<u8> = Vec::new(); // partial line across reads
    let mut eof = false;

    loop {
        while !eof {
            let payload: Option<Vec<u8>> = if chunked {
                match next_chunk(buf) {
                    Chunk::Data(bytes) => Some(bytes),
                    Chunk::NeedMore => None,
                    Chunk::End => {
                        eof = true;
                        None
                    }
                }
            } else {
                if buf.is_empty() {
                    None
                } else {
                    Some(std::mem::take(buf))
                }
            };
            let Some(payload) = payload else { break };
            if !feed_lines(&payload, &mut pending, on_data) {
                return Ok(());
            }
            if eof {
                break;
            }
        }

        if eof {
            if !pending.is_empty() {
                let rest = std::mem::take(&mut pending);
                if !finish_line(&rest, on_data) {
                    return Ok(());
                }
            }
            return Ok(());
        }
        match read_into(sock, buf) {
            Ok(0) => eof = true,
            Ok(_) => {}
            Err(e) => return Err(format!("read: {e}")),
        }
    }
}

/// Split payload into complete `\n`-terminated lines (carrying `pending` across
/// socket reads so JSON is never cut mid-line). Returns false to stop the pump.
fn feed_lines(payload: &[u8], pending: &mut Vec<u8>, on_data: &mut dyn FnMut(&str) -> bool) -> bool {
    for &b in payload {
        if b == b'\n' {
            let line = std::mem::take(pending);
            if !finish_line(&line, on_data) {
                return false;
            }
        } else if b != b'\r' {
            pending.push(b);
        }
    }
    true
}

fn finish_line(bytes: &[u8], on_data: &mut dyn FnMut(&str) -> bool) -> bool {
    let s = String::from_utf8_lossy(bytes).trim().to_string();
    match sse_data(&s) {
        Some(data) => {
            if data == "[DONE]" {
                return false;
            }
            on_data(&data)
        }
        None => true,
    }
}

fn read_into(sock: &mut TcpStream, buf: &mut Vec<u8>) -> std::io::Result<usize> {
    let start = buf.len();
    buf.resize(start + 8192, 0);
    let n = sock.read(&mut buf[start..])?;
    buf.truncate(start + n);
    Ok(n)
}

enum Chunk {
    Data(Vec<u8>),
    NeedMore,
    End,
}

/// Consume one complete chunk from the front of `buf`, returning its payload.
fn next_chunk(buf: &mut Vec<u8>) -> Chunk {
    let nl = match find_subslice(buf, b"\r\n") {
        Some(i) => i,
        None => return Chunk::NeedMore,
    };
    let size_line = String::from_utf8_lossy(&buf[..nl]).trim().to_string();
    let size = match usize::from_str_radix(size_line.split(';').next().unwrap_or("").trim(), 16) {
        Ok(s) => s,
        Err(_) => return Chunk::End,
    };
    if size == 0 {
        return Chunk::End;
    }
    let start = nl + 2;
    let end = start + size;
    if buf.len() < end + 2 {
        return Chunk::NeedMore;
    }
    let data = buf[start..end].to_vec();
    buf.drain(..end + 2); // skip chunk data + trailing CRLF
    Chunk::Data(data)
}

fn sse_data(line: &str) -> Option<String> {
    if let Some(rest) = line.strip_prefix("data:") {
        let d = rest.trim();
        if !d.is_empty() {
            return Some(d.to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Message;

    fn model_with_budget(budget: usize) -> HttpModel {
        let mut m = HttpModel::new(
            "127.0.0.1".into(),
            1,
            "test".into(),
            "k".into(),
            "catalog".into(),
        );
        m.context_budget = budget;
        m
    }

    #[test]
    fn build_messages_compacts_oversized_history() {
        // A history far above a tiny budget must be folded before sending.
        let mut h = vec![Message::text("system", "sys"), Message::text("user", "go")];
        for _ in 0..6 {
            h.push(Message::text("tool", "z".repeat(30_000)));
            h.push(Message::text("user", "next"));
        }
        let raw_total: usize = h.iter().map(|m| crate::context::estimate_tokens(&m.content)).sum();
        assert!(raw_total > 4096, "precondition: history busts the budget");

        let msgs = model_with_budget(4096).build_messages(&h);
        // The system prompt is always first; every other message was compacted
        // so no single user/tool body carries the full 30k blob any more.
        let biggest = msgs
            .iter()
            .skip(1)
            .map(|m| m["content"].as_str().unwrap_or("").chars().count())
            .max()
            .unwrap_or(0);
        assert!(biggest < 30_000, "expected shrink/fold, got a {biggest}-char body");
        // Compaction ran: either stale tools were pruned (elision markers) or
        // older turns were folded into a digest. Either satisfies the budget.
        let compacted = msgs.iter().any(|m| {
            let c = m["content"].as_str().unwrap_or("");
            c.contains("[Context compacted:") || c.contains("characters elided")
        });
        assert!(compacted, "neither fold nor prune left a trace");
    }

    #[test]
    fn build_messages_leaves_small_history_intact() {
        let h = vec![
            Message::text("system", "sys"),
            Message::text("user", "hello"),
            Message::text("assistant", "hi"),
        ];
        let msgs = model_with_budget(4096).build_messages(&h);
        assert!(!msgs.iter().any(|m| {
            m["content"]
                .as_str()
                .unwrap_or("")
                .contains("[Context compacted:")
        }));
    }
}

