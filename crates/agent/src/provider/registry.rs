//! ProviderRegistry: resolve a catalog entry to a concrete protocol + endpoint.
//!
//! The catalog (`providers_catalog.json`, 174 entries) tells us each provider's
//! base URL, env var and auth type but not which wire *protocol* it speaks — most
//! speak OpenAI-compat, a few speak their native API. `protocol_for` decides that
//! from the id/base_url, so `/provider anthropic` uses the Messages adapter while
//! `/provider gemini` (whose catalog base is the `/openai` compat path) still
//! works through OpenAI-compat unless the user points at the native REST root.

use super::anthropic::Anthropic;
use super::gemini::Gemini;
use super::openai::OpenAiCompat;
use super::types::ProviderProtocol;
use super::ProviderSpec;
use std::sync::Arc;

/// Which authentication scheme a resolved request should use. Kept separate from
/// the protocol because the same protocol can be reached with bearer or api-key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthScheme {
    /// `Authorization: Bearer <key>` (default for OpenAI-compat).
    Bearer,
    /// Key only in a protocol-supplied header (Anthropic x-api-key, Gemini key).
    HeaderOnly,
    /// No auth (local Ollama / loopback gateway).
    None,
}

/// A resolved, ready-to-call endpoint: protocol + absolute path + auth plan.
#[derive(Clone)]
pub struct Resolved {
    pub protocol: Arc<dyn ProviderProtocol>,
    pub host: String,
    pub port: u16,
    pub path: String,
    pub auth: AuthScheme,
    pub key: String,
}

/// Pick the protocol implementation for a provider spec.
pub fn protocol_for(spec: &ProviderSpec) -> Arc<dyn ProviderProtocol> {
    let id = spec.id.to_ascii_lowercase();
    let base = spec.base_url.clone().unwrap_or_default().to_ascii_lowercase();
    if id == "anthropic" || id.contains("anthropic") || base.contains("api.anthropic.com") {
        return Arc::new(Anthropic);
    }
    // Native Gemini REST only when NOT using Google's own /openai compat path.
    let native_gemini = (id == "gemini" || id == "google" || id.contains("generativelanguage"))
        && !base.ends_with("/openai")
        && base.contains("generativelanguage.googleapis.com");
    if native_gemini {
        return Arc::new(Gemini);
    }
    Arc::new(OpenAiCompat)
}

/// Decide the auth scheme from the spec + protocol.
fn auth_scheme_for(spec: &ProviderSpec, proto: &dyn ProviderProtocol) -> AuthScheme {
    match proto.name() {
        "anthropic" | "gemini" => AuthScheme::HeaderOnly,
        _ => {
            if spec.auth_type == "none" {
                AuthScheme::None
            } else {
                AuthScheme::Bearer
            }
        }
    }
}

/// Resolve a spec + config into a callable endpoint description.
pub fn resolve(spec: &ProviderSpec, base_url: &str, key: &str) -> Result<Resolved, String> {
    let protocol = protocol_for(spec);
    let (host, port, _path) = split_base(base_url);
    let full_path = if protocol.name() == "gemini" {
        // Gemini embeds model+verb later; store the versioned root here.
        protocol.request_path(&format!("{base_url}"))
    } else {
        protocol.request_path(base_url)
    };
    let auth = auth_scheme_for(spec, protocol.as_ref());
    Ok(Resolved {
        protocol,
        host,
        port,
        path: full_path,
        auth,
        key: key.to_string(),
    })
}

/// Split `scheme://host[:port]/sub` into (host, port, "/sub"). Port defaults by
/// scheme (443 https, 80 http) when absent.
pub(crate) fn split_base(base: &str) -> (String, u16, String) {
    let after = base.split("://").nth(1).unwrap_or(base);
    let mut segs = after.splitn(2, '/');
    let authority = segs.next().unwrap_or("");
    let rest = segs.next().unwrap_or("");
    let (host, port) = match authority.rsplit_once(':') {
        Some((h, p)) if p.parse::<u16>().is_ok() => (h.to_string(), p.parse().unwrap()),
        _ => {
            let default = if base.starts_with("https") { 443 } else { 80 };
            (authority.to_string(), default)
        }
    };
    let sub = if rest.is_empty() { String::new() } else { format!("/{rest}") };
    (host, port, sub)
}

impl Resolved {
    /// Build the transport headers + bearer for one call given the protocol ctx.
    /// Returns `(bearer_token, owned_headers)`; the caller borrows the header names
    /// and values as `&str` when handing them to `transport::{request,sse}`.
    pub fn headers_for(&self, ctx: &super::types::RequestCtx) -> (Option<String>, Vec<(String, String)>) {
        let proto_headers = self.protocol.headers(ctx);
        let bearer = match self.auth {
            AuthScheme::Bearer if !self.key.is_empty() => Some(self.key.clone()),
            _ => None,
        };
        (bearer, proto_headers)
    }
}
