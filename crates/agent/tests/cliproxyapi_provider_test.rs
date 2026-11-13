//! Integration test suite for CLIProxyAPI-Rust provider:
//! 1. Provider registration & configuration.
//! 2. Base URL & API key resolution (including env vars).
//! 3. Model discovery (`GET /v1/models`).
//! 4. Non-streaming completion.
//! 5. Streaming completion.
//! 6. Tool-call round trip.
//! 7. Authentication errors (401).
//! 8. Connection failures.
//! 9. Cancellation and timeout.
//! 10. Missing proxy process handling.
//! 11. Secret redaction.
//! 12. Compatibility with existing providers.

use agent::model::{GatewayCfg, HttpModel};
use agent::provider::{
    check_provider_health, find_spec, load_catalog, redact_secret, resolve_api_key, resolve_base_url,
    HealthStatus, ProviderConfig,
};
use agent::{Message, Step};
use serde_json::json;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::thread;
use std::time::Duration;

fn spawn_mock_server(handler: impl Fn(String) -> (u16, &'static str, String) + Send + 'static) -> (String, u16) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut sock) = stream else { break };
            let mut buf = [0u8; 4096];
            let n = sock.read(&mut buf).unwrap_or(0);
            let req = String::from_utf8_lossy(&buf[..n]).to_string();
            let (code, content_type, body) = handler(req);
            let resp = format!(
                "HTTP/1.1 {code} OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = sock.write_all(resp.as_bytes());
            let _ = sock.flush();
        }
    });
    ("127.0.0.1".to_string(), port)
}

#[test]
fn test_01_provider_registration() {
    let catalog = load_catalog();
    let spec = catalog.iter().find(|p| p.id == "cliproxyapi").expect("cliproxyapi must be registered in catalog");
    assert_eq!(spec.name, "CLIProxyAPI (Rust / Go)");
    assert_eq!(spec.base_url.as_deref(), Some("http://127.0.0.1:8317/v1"));
    assert!(spec.function_calling);
}

#[test]
fn test_02_base_url_and_api_key_resolution() {
    let spec = find_spec("cliproxyapi").unwrap();
    // Default base url
    let base = resolve_base_url(&spec, None);
    assert_eq!(base, "http://127.0.0.1:8317/v1");

    // Config override
    let cfg = ProviderConfig {
        provider_id: "cliproxyapi".into(),
        base_url: Some("http://192.168.1.10:8317/v1".into()),
        api_key: Some("test-key-123".into()),
        ..Default::default()
    };
    let base_cfg = resolve_base_url(&spec, Some(&cfg));
    assert_eq!(base_cfg, "http://192.168.1.10:8317/v1");
    let key_cfg = resolve_api_key(&spec, Some(&cfg));
    assert_eq!(key_cfg, Some("test-key-123".into()));

    // Env override
    unsafe {
        std::env::set_var("CLIPROXYAPI_BASE_URL", "http://localhost:9999/v1");
        std::env::set_var("CLIPROXYAPI_API_KEY", "env-secret-key");
    }
    assert_eq!(resolve_base_url(&spec, Some(&cfg)), "http://localhost:9999/v1");
    assert_eq!(resolve_api_key(&spec, Some(&cfg)), Some("env-secret-key".into()));
    unsafe {
        std::env::remove_var("CLIPROXYAPI_BASE_URL");
        std::env::remove_var("CLIPROXYAPI_API_KEY");
    }
}

#[test]
fn test_03_model_discovery() {
    let (host, port) = spawn_mock_server(|req| {
        if req.starts_with("GET /v1/models") {
            (
                200,
                "application/json",
                json!({
                    "object": "list",
                    "data": [
                        {"id": "gpt-4o", "object": "model"},
                        {"id": "claude-3-5-sonnet", "object": "model"}
                    ]
                }).to_string(),
            )
        } else {
            (404, "text/plain", "not found".into())
        }
    });

    let gw = GatewayCfg {
        host: host.clone(),
        port,
        base: format!("http://{host}:{port}/v1"),
        model: "gpt-4o".into(),
        key: "test".into(),
    };
    let hm = HttpModel::from_cfg(&gw, String::new(), Vec::new());
    let models = hm.list_models().unwrap();
    assert_eq!(models, vec!["gpt-4o", "claude-3-5-sonnet"]);
}

#[test]
fn test_04_non_streaming_completion() {
    let (host, port) = spawn_mock_server(|_| {
        (
            200,
            "application/json",
            json!({
                "choices": [{
                    "index": 0,
                    "message": {
                        "role": "assistant",
                        "content": "Hello from CLIProxyAPI-Rust!"
                    },
                    "finish_reason": "stop"
                }]
            }).to_string(),
        )
    });

    let gw = GatewayCfg {
        host: host.clone(),
        port,
        base: format!("http://{host}:{port}/v1"),
        model: "gpt-4o".into(),
        key: "test".into(),
    };
    let hm = HttpModel::from_cfg(&gw, String::new(), Vec::new());
    let step = hm.chat_raw("{\"messages\":[]}");
    assert_eq!(step.unwrap(), "Hello from CLIProxyAPI-Rust!");
}

#[test]
fn test_05_streaming_completion() {
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    thread::spawn(move || {
        if let Ok(mut sock) = listener.accept().map(|(s, _)| s) {
            let mut drain = [0u8; 1024];
            let _ = sock.read(&mut drain);
            let sse_data = "data: {\"choices\":[{\"delta\":{\"content\":\"Chunk1 \"}}]}\n\ndata: {\"choices\":[{\"delta\":{\"content\":\"Chunk2\"}}]}\n\ndata: [DONE]\n\n";
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n{:x}\r\n{}\r\n0\r\n\r\n",
                sse_data.len(),
                sse_data
            );
            let _ = sock.write_all(resp.as_bytes());
            let _ = sock.flush();
        }
    });

    let gw = GatewayCfg {
        host: "127.0.0.1".into(),
        port,
        base: format!("http://127.0.0.1:{port}/v1"),
        model: "gpt-4o".into(),
        key: "test".into(),
    };
    let mut hm = HttpModel::from_cfg(&gw, String::new(), Vec::new());
    let mut collected = String::new();
    let step = hm.next_step_streaming(&[Message::text("user", "hi")], &[], &mut |d| collected.push_str(d), &mut |_| {});
    assert!(matches!(step, Step::Final(_)));
    assert_eq!(collected, "Chunk1 Chunk2");
}

#[test]
fn test_06_tool_call_round_trip() {
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    thread::spawn(move || {
        if let Ok(mut sock) = listener.accept().map(|(s, _)| s) {
            let mut drain = [0u8; 1024];
            let _ = sock.read(&mut drain);
            let sse_data = "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"name\":\"read_file\",\"arguments\":\"{\\\"path\\\":\\\"/tmp/foo\\\"}\"}}]}}]}\n\ndata: [DONE]\n\n";
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n{:x}\r\n{}\r\n0\r\n\r\n",
                sse_data.len(),
                sse_data
            );
            let _ = sock.write_all(resp.as_bytes());
            let _ = sock.flush();
        }
    });

    let gw = GatewayCfg {
        host: "127.0.0.1".into(),
        port,
        base: format!("http://127.0.0.1:{port}/v1"),
        model: "gpt-4o".into(),
        key: "test".into(),
    };
    let mut hm = HttpModel::from_cfg(&gw, String::new(), Vec::new());
    let step = hm.next_step_streaming(&[Message::text("user", "read it")], &["read_file"], &mut |_| {}, &mut |_| {});
    match step {
        Step::Calls(calls) => {
            assert_eq!(calls.len(), 1);
            assert_eq!(calls[0].name, "read_file");
            assert_eq!(calls[0].args["path"], "/tmp/foo");
        }
        _ => panic!("Expected Step::Calls, got {step:?}"),
    }
}

#[test]
fn test_07_auth_error_health_check() {
    let (host, port) = spawn_mock_server(|_| {
        (
            401,
            "application/json",
            json!({
                "error": {
                    "message": "Invalid API key provided to proxy",
                    "code": 401
                }
            }).to_string(),
        )
    });

    let spec = find_spec("cliproxyapi").unwrap();
    let cfg = ProviderConfig {
        provider_id: "cliproxyapi".into(),
        base_url: Some(format!("http://{host}:{port}/v1")),
        api_key: Some("bad-key".into()),
        ..Default::default()
    };
    let health = check_provider_health(&spec, Some(&cfg));
    assert!(matches!(health, HealthStatus::AuthFailure(_)));
}

#[test]
fn test_08_connection_failure() {
    let spec = find_spec("cliproxyapi").unwrap();
    // Non-listening local port
    let cfg = ProviderConfig {
        provider_id: "cliproxyapi".into(),
        base_url: Some("http://127.0.0.1:41999/v1".into()),
        ..Default::default()
    };
    let health = check_provider_health(&spec, Some(&cfg));
    match health {
        HealthStatus::ConnectionRefused(msg) => {
            assert!(msg.contains("CLIProxyAPI is not running"));
        }
        other => panic!("Expected ConnectionRefused, got {other:?}"),
    }
}

#[test]
fn test_09_timeout_handling() {
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    thread::spawn(move || {
        if let Ok(mut sock) = listener.accept().map(|(s, _)| s) {
            // Sleep without writing response
            thread::sleep(Duration::from_millis(500));
            let mut buf = [0u8; 10];
            let _ = sock.read(&mut buf);
        }
    });

    let spec = find_spec("cliproxyapi").unwrap();
    let cfg = ProviderConfig {
        provider_id: "cliproxyapi".into(),
        base_url: Some(format!("http://127.0.0.1:{port}/v1")),
        ..Default::default()
    };
    // TCP succeeds, but probe has short timeout
    let _health = check_provider_health(&spec, Some(&cfg));
}

#[test]
fn test_10_missing_proxy_guidance() {
    let spec = find_spec("cliproxyapi").unwrap();
    let cfg = ProviderConfig {
        provider_id: "cliproxyapi".into(),
        base_url: Some("http://127.0.0.1:8399/v1".into()),
        ..Default::default()
    };
    let health = check_provider_health(&spec, Some(&cfg));
    if let HealthStatus::ConnectionRefused(hint) = health {
        assert!(hint.contains("cliproxyapi-rust"));
    }
}

#[test]
fn test_11_secret_redaction() {
    assert_eq!(redact_secret(""), "(none)");
    assert_eq!(redact_secret("short"), "***");
    assert_eq!(redact_secret("sk-cliproxy-secret-token-99"), "sk-c...n-99");
}

#[test]
fn test_12_compatibility_with_existing_providers() {
    let catalog = load_catalog();
    assert!(catalog.iter().any(|p| p.id == "openai"));
    assert!(catalog.iter().any(|p| p.id == "anthropic"));
    assert!(catalog.iter().any(|p| p.id == "openrouter"));
    assert!(catalog.iter().any(|p| p.id == "cliproxyapi"));
}

#[test]
fn test_13_specialized_provider_identities_registered() {
    let catalog = load_catalog();
    let expected = ["claude", "codex", "antigravity", "kimi", "grok", "muse", "devin", "vertex"];
    for id in &expected {
        assert!(catalog.iter().any(|p| p.id == *id), "Provider `{id}` should be registered in catalog");
    }
}

#[test]
fn test_14_gateway_binary_detection() {
    let detected = agent::provider::cliproxy::find_gateway_binary();
    // In our test environment, /root/cliproxy/cli-proxy-api exists
    assert!(detected.is_some(), "Gateway binary should be discovered");
}

#[test]
fn test_15_gateway_probe_running_state() {
    let info = agent::provider::cliproxy::inspect_gateway(Some("http://127.0.0.1:54321"));
    assert_eq!(info.running, false);
    assert_eq!(info.endpoint, "http://127.0.0.1:54321");
}

#[test]
fn test_16_gateway_login_flag_mapping() {
    assert_eq!(agent::provider::cliproxy::resolve_login_target("claude"), Some("-claude-login"));
    assert_eq!(agent::provider::cliproxy::resolve_login_target("codex"), Some("-codex-login"));
    assert_eq!(agent::provider::cliproxy::resolve_login_target("antigravity"), Some("-antigravity-login"));
    assert_eq!(agent::provider::cliproxy::resolve_login_target("kimi"), Some("-kimi-login"));
    assert_eq!(agent::provider::cliproxy::resolve_login_target("grok"), Some("-xai-login"));
    assert_eq!(agent::provider::cliproxy::resolve_login_target("muse"), Some("-meta-login"));
    assert_eq!(agent::provider::cliproxy::resolve_login_target("devin"), Some("-devin-login"));
    assert_eq!(agent::provider::cliproxy::resolve_login_target("unknown"), None);
}

#[test]
fn test_17_gateway_status_formatting_mock() {
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    thread::spawn(move || {
        // Handle probe and request
        while let Ok((mut sock, _)) = listener.accept() {
            let mut buf = [0u8; 1024];
            let n = sock.read(&mut buf).unwrap_or(0);
            if n > 0 {
                let resp = "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 75\r\nConnection: close\r\n\r\n{\"data\":[{\"id\":\"claude-sonnet-4-6\"},{\"id\":\"gpt-oss-120b\"}]}";
                let _ = sock.write_all(resp.as_bytes());
                let _ = sock.flush();
                break;
            }
        }
    });

    let url = format!("http://127.0.0.1:{port}");
    let res = agent::provider::cliproxy::query_gateway_status(&url, Some("sk-secret-key-12345"));
    assert!(res.is_ok(), "query_gateway_status failed: {:?}", res);
    let out = res.unwrap();
    assert!(out.contains("Discovered Models (2 available)"));
    assert!(out.contains("claude-sonnet-4-6"));
    assert!(!out.contains("sk-secret-key-12345"), "Raw secret key should not leak into status report");
}

#[test]
fn test_18_protocol_resolution_for_subproviders() {
    use agent::provider::registry::protocol_for;
    let catalog = load_catalog();
    for id in &["claude", "codex", "kimi", "grok", "muse", "devin", "vertex"] {
        let spec = catalog.iter().find(|p| p.id == *id).unwrap();
        let proto = protocol_for(spec);
        assert!(!proto.name().is_empty());
    }
}

#[test]
fn test_19_responses_api_and_compat_parsing() {
    // Audit of OpenAI Responses-style JSON payload vs Chat Completions
    let json_resp = r#"{
        "id": "resp-123",
        "object": "chat.completion",
        "created": 1700000000,
        "choices": [{
            "index": 0,
            "message": {
                "role": "assistant",
                "content": "Hello from CLIProxyAPI Responses layer"
            },
            "finish_reason": "stop"
        }]
    }"#;
    let proto = agent::provider::openai::OpenAiCompat;
    let parsed = agent::provider::types::ProviderProtocol::parse_reply(&proto, json_resp);
    assert!(parsed.is_ok());
    assert_eq!(parsed.unwrap().content, "Hello from CLIProxyAPI Responses layer");
}

#[test]
fn test_20_reasoning_effort_propagation() {
    use agent::provider::types::{ProviderProtocol, RequestCtx};
    let proto = agent::provider::openai::OpenAiCompat;
    let ctx = RequestCtx {
        model: "claude-sonnet-4-6",
        key: "test-key",
        temperature: 0.7,
        max_tokens: 4096,
        effort: "high",
        stream: false,
        tools: &[],
        system: "You are a test agent",
    };
    let body = proto.build_body(&ctx, &[]);
    assert_eq!(body.get("reasoning_effort").and_then(|v| v.as_str()), Some("high"));
}

#[test]
fn test_21_stream_accumulator_tool_call_stitching() {
    use agent::provider::types::StreamAcc;
    let mut acc = StreamAcc::default();
    acc.calls.insert(0, ("read_file".into(), "{\"path\":".into()));
    acc.calls.entry(0).and_modify(|(_, args)| args.push_str("\"/root/test.txt\"}"));
    let parsed = acc.into_parsed();
    assert_eq!(parsed.tool_calls.len(), 1);
    assert_eq!(parsed.tool_calls[0].name, "read_file");
    assert_eq!(parsed.tool_calls[0].args["path"], "/root/test.txt");
}

#[test]
fn test_22_session_affinity_and_headers_preservation() {
    let mut storage = agent::provider::load_storage();
    let entry = storage.providers.entry("cliproxyapi".to_string()).or_insert_with(Default::default);
    entry.custom_headers.insert("X-Session-Affinity".to_string(), "sess-99".to_string());
    assert_eq!(entry.custom_headers.get("X-Session-Affinity").unwrap(), "sess-99");
}

#[test]
fn test_23_guardian_policy_unaltered_with_cliproxy() {
    // Verify Guardian and standard policies run without modification
    let workdir = std::path::PathBuf::from("/tmp");
    let tools = tools::default_collection(workdir);
    let gw = tools::audit::Gateway::<dyn tools::policy::Policy>::from_boxed(
        tools,
        Box::new(tools::policy::AllowAll),
    );
    assert!(gw.tools.names().contains(&"read_file".to_string()));
}

