//! tektile-agent — the on-device agentic loop wired to a live LLM gateway
//! (OmniRoute on loopback :20128 by default; override with ONYX_BASE_URL).
//!
//! It drives the SAME `ToolCallAgent` + `Gateway` (policy + audit) used by the
//! offline `ScriptedModel` tests, but behind the `Model` trait sits `HttpModel`.
//! Nothing here needs a payment card or an outbound cloud key: OmniRoute holds
//! upstream credentials and exposes quota-aware auto-fallback via model "auto".
//!
//! Usage:
//!   tektile-agent [--model auto] [--max-steps 8] [--allow-all] <prompt...>

use agent::model::{omniroute_config, HttpModel};
use agent::{AgentError, ToolCallAgent};
use std::env;
use std::process::exit;
use tools::audit::Gateway;
use tools::policy::{AllowAll, BashGuard, Policy};

fn catalog_for(gw: &Gateway<impl Policy>) -> String {
    let mut s = String::new();
    for name in gw.tools.names() {
        if let Some(t) = gw.tools.get(&name) {
            s.push_str(&format!("  - {name}: {}\n", t.description()));
        }
    }
    s
}

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    let cfg = omniroute_config();
    let mut model = cfg.model.clone();
    let mut max_steps: usize = 8;
    let mut allow_all = false;
    let mut prompt_parts: Vec<String> = Vec::new();

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--model" => {
                if i + 1 < args.len() { model = args[i + 1].clone(); i += 2; } else { i += 1; }
            }
            "--max-steps" => {
                if i + 1 < args.len() { max_steps = args[i + 1].parse().unwrap_or(8); i += 2; } else { i += 1; }
            }
            "--allow-all" => { allow_all = true; i += 1; }
            "-h" | "--help" => { usage(); exit(0); }
            other => { prompt_parts.push(other.to_string()); i += 1; }
        }
    }
    let prompt = prompt_parts.join(" ").trim().to_string();
    if prompt.is_empty() {
        eprintln!("error: no prompt given\n");
        usage();
        exit(2);
    }

    println!("tektile-agent → model={model} gateway={}:{} (key length {})", cfg.host, cfg.port, cfg.key.chars().count());
    println!("prompt: {prompt}\n");

    let base = env::temp_dir().join("tektile-agent-editor");

    // Two policy monomorphs; pick one per invocation. Each arm owns its gateway
    // for the whole run and returns an owned (report, audit) pair.
    let outcome = if allow_all {
        let gw = Gateway::new(tools::default_collection(base), AllowAll);
        let mut hm = HttpModel::from_cfg(&cfg, catalog_for(&gw), Vec::new());
        hm.model = model.clone();
        let mut agent = ToolCallAgent::new(&gw);
        agent.max_steps = max_steps;
        agent.run(&mut hm, &prompt).map(|r| (r, gw.audit.snapshot()))
    } else {
        let gw = Gateway::new(tools::default_collection(base), BashGuard::default());
        let mut hm = HttpModel::from_cfg(&cfg, catalog_for(&gw), Vec::new());
        hm.model = model.clone();
        let mut agent = ToolCallAgent::new(&gw);
        agent.max_steps = max_steps;
        agent.run(&mut hm, &prompt).map(|r| (r, gw.audit.snapshot()))
    };

    match outcome {
        Ok((report, audit)) => {
            println!("---- trace ----");
            for m in &report.history {
                if m.role == "system" { continue; }
                let tag = match m.role.as_str() {
                    "user" => "USER",
                    "assistant" => "ASSIST",
                    "tool" => "TOOL ",
                    r => r,
                };
                let one = m.content.replace('\n', " ");
                let one = if one.len() > 200 { format!("{}…", one.chars().take(200).collect::<String>()) } else { one };
                println!("{tag}│ {one}");
            }
            println!("\n---- audit ({} call(s)) ----", audit.len());
            for a in &audit {
                let oc = if a.outcome.len() > 120 { format!("{}…", a.outcome.chars().take(120).collect::<String>()) } else { a.outcome.clone() };
                println!("  {:<12} → {}", a.tool, oc);
            }
            println!("\nANSWER: {}", report.answer);
            println!("steps: {}", report.steps);
        }
        Err(AgentError::MaxSteps) => {
            eprintln!("\nerror: reached max_steps ({max_steps}) without terminating");
            exit(3);
        }
        Err(AgentError::DoomLoop { tool, repeats, reason }) => {
            eprintln!("\nerror: agent stuck — {reason} ({tool} ×{repeats})");
            exit(5);
        }
        Err(AgentError::Tool(t)) => {
            eprintln!("\nerror: a tool call failed at the gateway: {t}");
            exit(4);
        }
    }
}

fn usage() {
    eprintln!("usage: tektile-agent [--model NAME] [--max-steps N] [--allow-all] <prompt...>");
}
