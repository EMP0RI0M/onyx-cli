//! End-to-end proof that retrieval narrows what a turn is offered — through the
//! real agent loop, not just `apply_turn` in isolation.
//!
//! The unit tests in `retrieval.rs` check the projection; this checks the wiring:
//! that `run_turn_observed` actually reads `turn_tools` back and hands the model a
//! filtered tool list, and emits a `CapabilitySelection` event saying so. A scripted
//! model stands in for the network (no live gateway needed on this box).

use std::any::Any;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread;

use serde_json::{json, Value};

use agent::model::HttpModel;
use agent::retrieval::CapabilitySelector;
use agent::{AgentEvent, Message, Model, Step, ToolCall, ToolCallAgent};
use tools::policy::AllowAll;
use tools::ToolCollection;

/// A one-shot loopback gateway that records the exact JSON request body it is sent,
/// then replies with a native `terminate` tool call. Pointing a real [`HttpModel`] at
/// it lets us run the whole agent loop offline and observe precisely which tools were
/// put on the wire — the strongest possible proof that retrieval narrowed the surface.
fn spawn_recording_gateway() -> (u16, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    let bodies: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = bodies.clone();
    thread::spawn(move || {
        // One connection per step; terminate ends the loop after the first, so this
        // serves exactly once in practice. Loop keeps it alive if reused.
        for stream in listener.incoming() {
            let mut sock: TcpStream = stream.unwrap();
            let body = read_body(&mut sock);
            sink.lock().unwrap().push(body);
            let reply = "{\"choices\":[{\"message\":{\"content\":\"\",\"tool_calls\":\
                [{\"function\":{\"name\":\"terminate\",\
                \"arguments\":\"{\\\"status\\\":\\\"success\\\",\\\"content\\\":\\\"done\\\"}\"}}]}}]}";
            let raw = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}",
                reply.len()
            );
            let _ = sock.write_all(raw.as_bytes());
            let _ = sock.flush();
            drop(sock);
        }
    });
    (port, bodies)
}

/// Read an HTTP request off the socket up to and including its Content-Length body.
fn read_body(sock: &mut TcpStream) -> String {
    let mut buf = vec![0u8; 8192];
    let mut raw = Vec::new();
    loop {
        let n = match sock.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        raw.extend_from_slice(&buf[..n]);
        if let Some(pos) = find(&raw, b"\r\n\r\n") {
            let head = String::from_utf8_lossy(&raw[..pos]).to_ascii_lowercase();
            let content_len = match find(head.as_bytes(), b"content-length: ") {
                Some(i) => head[i + 16..]
                    .find("\r\n")
                    .map(|e| head[i + 16..][..e].trim().parse().unwrap_or(0))
                    .unwrap_or(0),
                None => 0,
            };
            if raw.len() >= pos + 4 + content_len {
                break;
            }
        }
        if raw.len() > 1 << 20 {
            break;
        }
    }
    String::from_utf8_lossy(&raw).to_string()
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if hay.len() < needle.len() {
        return None;
    }
    hay.windows(needle.len()).position(|w| w == needle)
}

/// Tool names the fake gateway saw on the wire in the given request body.
fn offered_on_wire(body: &str) -> Vec<String> {
    let json_start = find(body.as_bytes(), b"\r\n\r\n").map(|p| &body[p + 4..]).unwrap_or(body);
    let v: Value = serde_json::from_str(json_start).expect("gateway received a JSON request");
    v["tools"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|d| d["function"]["name"].as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default()
}

fn reg_tool(name: &str, desc: &str) -> onyx_retrieval::Tool {
    onyx_retrieval::Tool {
        id: name.into(),
        name: name.into(),
        description: desc.into(),
        experimental_searchable_description: None,
        input_schema: json!({"type": "object", "properties": {}}),
        output_schema: json!({}),
    }
}

/// Build a gateway whose collection holds exactly `names`, plus an HttpModel whose
/// session catalog advertises all of them (so the unfiltered baseline is "everything").
fn harness(names: &[&str]) -> (tools::audit::Gateway<AllowAll>, HttpModel) {
    let mut col = ToolCollection::new();
    for n in names {
        col.add(Stub(n.to_string()));
    }
    let gw = tools::audit::Gateway::new(col, AllowAll);

    let defs: Vec<Value> = names
        .iter()
        .map(|n| {
            json!({"type":"function","function":{"name": n, "description": n, "parameters": {"type":"object"}}})
        })
        .collect();
    let catalog = names.iter().map(|n| format!("  - {n}: stub\n")).collect::<String>();
    let mut hm = HttpModel::new("127.0.0.1".into(), 1, "test-model".into(), String::new(), catalog);
    hm.tool_defs = defs;
    (gw, hm)
}

// Minimal control tool so terminate exists at the gateway.
struct Stub(String);
impl tools::Tool for Stub {
    fn name(&self) -> &str {
        &self.0
    }
    fn description(&self) -> &str {
        "a stub capability"
    }
    fn execute(&self, _args: &Value) -> Result<tools::Output, tools::ToolError> {
        Ok(tools::Output::ok("ok"))
    }
}

#[test]
fn run_turn_offers_only_retrieved_tools_not_the_whole_catalog() {
    // Fifteen irrelevant fillers + one matching tool + terminate. BM25 should put the
    // matching tool and terminate on the wire, not all seventeen. Driven through the
    // real agent loop with a real HttpModel pointed at a recording loopback gateway,
    // so we observe exactly what was sent — not just what apply_turn wrote internally.
    let mut corpus_names: Vec<String> = (0..15).map(|i| format!("filler_{i}")).collect();
    corpus_names.push("read_file".into());
    corpus_names.push("terminate".into());
    let refs: Vec<&str> = corpus_names.iter().map(|s| s.as_str()).collect();

    // Selector corpus mirrors the gateway catalog exactly — fillers included — so
    // filtering has something to rank against. read_file matches the query; the
    // fillers ("An unrelated placeholder capability") must not.
    let mut tool_corpus = vec![
        reg_tool("read_file", "Read a file from disk and return its contents"),
        reg_tool("terminate", "End the turn with the final answer"),
    ];
    for i in 0..15 {
        tool_corpus.push(reg_tool(&format!("filler_{i}"), "An unrelated placeholder capability"));
    }
    let sel = CapabilitySelector::new()
        .with_top_k(4)
        .with_min_catalog(8)
        .with_tools(tool_corpus);

    let (port, bodies) = spawn_recording_gateway();
    let (gw, mut hm) = harness(&refs);
    hm.host = "127.0.0.1".into();
    hm.port = port;

    let mut events: Vec<AgentEvent> = Vec::new();
    let mut history = vec![Message::text("system", "you are onyx")];
    {
        let agent = ToolCallAgent::new(&gw).with_selector(&sel);
        agent
            .run_turn_observed(&mut hm, "read the config file from disk", &mut history, &mut |ev| {
                events.push(ev)
            })
            .unwrap();
    }

    let bodies = bodies.lock().unwrap();
    assert_eq!(bodies.len(), 1, "expected exactly one request on the wire");
    let offered = offered_on_wire(&bodies[0]);
    assert!(offered.contains(&"read_file".to_string()), "matching tool missing: {offered:?}");
    assert!(offered.contains(&"terminate".to_string()), "terminate must survive filtering");
    assert!(
        !offered.iter().any(|n| n.starts_with("filler_")),
        "irrelevant tools leaked into the offered set: {offered:?}"
    );
    assert!(offered.len() < refs.len(), "nothing was filtered: {offered:?}");

    // The loop announced the compression via a CapabilitySelection event.
    assert!(
        events.iter().any(|e| matches!(e, AgentEvent::CapabilitySelection(_))),
        "no CapabilitySelection event emitted"
    );
}

#[test]
fn no_selector_offers_the_full_gateway_catalog() {
    // Baseline: without a selector the loop ships every registered tool, unchanged.
    // A scripted model keeps this pure-loop behavior (no projection is attempted for
    // non-HttpModel models by design), so it exercises the full-inventory fallback.
    let refs: Vec<&str> = vec!["alpha", "beta", "gamma", "delta", "epsilon", "zeta", "terminate"];
    let (gw, _hm) = harness(&refs);
    let mut model = ScriptedRecorder { seen: Vec::new() };
    let mut history = vec![Message::text("system", "you are onyx")];
    let agent = ToolCallAgent::new(&gw);
    agent
        .run_turn_observed(&mut model, "anything", &mut history, &mut |_| {})
        .unwrap();
    assert_eq!(model.seen[0].len(), refs.len(), "unfiltered turn dropped tools");
}

/// Scripted model that records the tool-name slice the loop handed it, then
/// terminates. Used only for the no-selector baseline (a non-HttpModel, so the loop
/// correctly ships the full gateway inventory).
struct ScriptedRecorder {
    seen: Vec<Vec<String>>,
}

impl Model for ScriptedRecorder {
    fn next_step(&mut self, _history: &[Message], tools: &[&str]) -> Step {
        self.seen.push(tools.iter().map(|s| s.to_string()).collect());
        Step::Calls(vec![ToolCall {
            name: "terminate".into(),
            args: json!({ "status": "success", "content": "done" }),
        }])
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}
