//! onyx — interactive agentic coding CLI (clean-room, Rust).
//!
//! One binary, three modes:
//!   onyx                 agy-style inline TUI (streaming, spinner, ctrl+o, shift+tab)
//!   onyx --legacy        plain blocking REPL (cooked stdin)
//!   onyx --print "..."   one-shot turn for scripting and tests
//!
//! Every tool call routes through the Gateway (policy → audit → execute), the
//! same chain the offline ScriptedModel tests exercise. Model backend is an
//! OpenAI-compatible gateway (OmniRoute on loopback by default; ONYX_BASE_URL).

mod markdown;
mod theme;
mod tui;

use agent::model::{omniroute_config, HttpModel};
use agent::{AgentError, Message, ToolCallAgent};
use std::env;
use std::io::{self, BufRead, Write};
use std::path::PathBuf;
use std::process::exit;
use tools::audit::{Approval, Gateway};
use guardian::HttpJudge;
use tools::policy::{AllowAll, BashGuard, Chained, DynChained, Guardian, Policy};

fn catalog_for(gw: &Gateway<dyn Policy>) -> String {
    let mut s = String::new();
    for name in gw.tools.names() {
        if let Some(t) = gw.tools.get(&name) {
            s.push_str(&format!("  - {name}: {}\n", t.description()));
        }
    }
    s
}

struct Opts {
    model: Option<String>,
    max_steps: usize,
    allow_all: bool,
    guardian: bool,
    sandbox: bool,
    skill: Option<String>,
    print: Option<String>,
    legacy: bool,
}

fn parse_args(args: Vec<String>) -> Result<Opts, String> {
    let mut o = Opts { model: None, max_steps: 8, allow_all: false, guardian: false, sandbox: false, skill: None, print: None, legacy: false };
    let mut i = 0;
    let mut positional: Vec<String> = Vec::new();
    while i < args.len() {
        match args[i].as_str() {
            "--model" => {
                i += 1;
                o.model = Some(args.get(i).ok_or("--model needs a value")?.clone());
            }
            "--max-steps" => {
                i += 1;
                o.max_steps = args.get(i).and_then(|v| v.parse().ok()).ok_or("--max-steps needs N")?;
            }
            "--allow-all" => o.allow_all = true,
            "--guardian" => o.guardian = true,
            "--sandbox" => o.sandbox = true,
            "--legacy" => o.legacy = true,
            "--skill" => {
                i += 1;
                o.skill = Some(args.get(i).ok_or("--skill needs a name")?.clone());
            }
            "--print" | "-p" => {
                i += 1;
                o.print = Some(args.get(i).ok_or("--print needs a prompt")?.clone());
            }
            "-h" | "--help" => return Err("help".into()),
            other => positional.push(other.to_string()),
        }
        i += 1;
    }
    if o.print.is_none() && !positional.is_empty() {
        o.print = Some(positional.join(" "));
    }
    Ok(o)
}

fn usage() {
    eprintln!(
        "usage: onyx [--model auto] [--max-steps N] [--allow-all] [--guardian] [--sandbox] [--skill NAME]\n\
         \t  [--print PROMPT | --legacy]\n\
         env: ONYX_BASE_URL (default http://127.0.0.1:20128/v1), ONYX_API_KEY, ONYX_MODEL,\n\
              ONYX_MAX_TOKENS, ONYX_WORKDIR, ONYX_GUARDIAN=1 (LLM risk judge before actions),\n\
              ONYX_SANDBOX=1 (kernel-enforced sandbox for bash)"
    );
}

/// Ask-before-execute approver wired into the BashGuard policy's `Ask` arm.
/// Refusal (or EOF) fails closed.
fn interactive_approve(tool: &str, args: &str, reason: &str) -> Approval {
    println!("\n⚠ approval needed — {tool} {args}\n  reason: {reason}");
    print!("allow? [y] once / [a] always / [N] no: ");
    io::stdout().flush().ok();
    let mut line = String::new();
    if io::stdin().lock().read_line(&mut line).unwrap_or(0) == 0 {
        return Approval::Reject;
    }
    match line.trim().to_lowercase().as_str() {
        "y" | "yes" => Approval::Once,
        "a" | "always" => Approval::Always,
        _ => Approval::Reject,
    }
}

/// Run one turn against a live model, printing this turn's trace + answer.
/// `history` persists across calls so the REPL stays conversational.
fn do_turn<P: Policy + ?Sized>(
    gw: &Gateway<P>,
    hm: &mut HttpModel,
    history: &mut Vec<Message>,
    prompt: &str,
    opts: &Opts,
) {
    let mut agent = ToolCallAgent::new(gw);
    agent.max_steps = opts.max_steps;
    match agent.run_turn(hm, prompt, history) {
        Ok(rep) => {
            let start = history.iter().rposition(|m| m.role == "user").unwrap_or(0);
            for m in &history[start..] {
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
            println!("\nANSWER: {}", rep.answer);
        }
        Err(AgentError::MaxSteps) => {
            eprintln!("error: reached max_steps ({}) without terminating", opts.max_steps);
        }
        Err(AgentError::DoomLoop { tool, repeats, reason }) => {
            eprintln!("error: agent stuck — {reason} ({tool} ×{repeats})");
        }
        Err(AgentError::Tool(t)) => {
            eprintln!("error: tool call failed at the gateway: {t}");
        }
    }
}

/// Slash commands handled by the REPL itself rather than the model.
fn slash_command(line: &str, hm: &mut HttpModel, opts: &mut Opts, history: &mut Vec<Message>) -> bool {
    let mut it = line.split_whitespace();
    match it.next().unwrap_or("") {
        "/quit" | "/exit" => return false,
        "/clear" => {
            *history = vec![Message::text("system", hm.system.clone())];
            println!("history cleared");
        }
        "/model" => {
            if let Some(m) = it.next() {
                hm.model = m.to_string();
                opts.model = Some(m.to_string());
            }
            println!("model: {}", hm.model);
        }
        "/steps" => {
            if let Some(n) = it.next().and_then(|v| v.parse().ok()) {
                opts.max_steps = n;
            }
            println!("max steps: {}", opts.max_steps);
        }
        "/skill" => {
            if let Some(name) = it.next() {
                if let Some(home) = env::var_os("HOME").map(PathBuf::from) {
                    let mut catalog = skills::SkillCatalog::new();
                    catalog.scan_dir(&home.join(".onyx/skills"));
                    if let Some(sk) = catalog.get(name) {
                        hm.system = sk.augment_system_prompt(&hm.system);
                        println!("loaded skill: {}", sk.name);
                    } else {
                        println!("skill `{name}` not found in ~/.onyx/skills");
                    }
                }
            } else {
                println!("usage: /skill <name>");
            }
        }
        "/sessions" => {
            let list = agent::session::list_sessions(12);
            if list.is_empty() {
                println!("no saved sessions yet");
            }
            for s in list {
                let pv = if s.preview.is_empty() { "(no prompt)".to_string() } else { s.preview };
                println!("  {} · {} msg · {}", s.id, s.messages, pv);
            }
            println!("/resume <id|prefix|suffix> to replay one");
        }
        "/res" | "/resume" => {
            if let Some(q) = it.next() {
                match agent::session::resolve_session(q) {
                    None => println!("no session matches `{q}` (try /sessions)"),
                    Some((path, msgs)) => {
                        *history = msgs.clone();
                        println!(
                            "resumed {} message(s) from {}",
                            msgs.len(),
                            path.file_stem().and_then(|s| s.to_str()).unwrap_or("?")
                        );
                    }
                }
            } else {
                println!("usage: /resume <id|prefix|suffix> (see /sessions)");
            }
        }
        "/help" => println!("commands: /model [name] /steps N /skill <name> /sessions /resume <id> /clear /quit"),
        other => println!("unknown command `{other}` (try /help)"),
    }
    true
}

/// Read stdin as a session: every non-empty line is a turn; slash commands are local.
fn repl<P: Policy + ?Sized>(gw: &Gateway<P>, hm: &mut HttpModel, opts: &mut Opts) {
    let mut history: Vec<Message> = Vec::new();
    let stdin = io::stdin();
    loop {
        print!("onyx> ");
        io::stdout().flush().ok();
        let mut line = String::new();
        if stdin.lock().read_line(&mut line).unwrap_or(0) == 0 {
            break;
        }
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if line.starts_with('/') {
            if !slash_command(line, hm, opts, &mut history) {
                break;
            }
            continue;
        }
        do_turn(gw, hm, &mut history, line, opts);
    }
    println!("\naudit ({} call(s) this session)", gw.audit.snapshot().len());
}

fn build_model(cfg: &agent::model::GatewayCfg, opts: &Opts, catalog: String, defs: Vec<serde_json::Value>) -> HttpModel {
    let mut hm = HttpModel::from_cfg(cfg, catalog, defs);
    if let Some(m) = &opts.model {
        hm.model = m.clone();
    }
    hm.system = "You are onyx, a precise agentic coding assistant. Use tools to act; when done, call terminate with the final answer.".to_string();
    hm
}

fn run_once<P: Policy + ?Sized>(gw: &Gateway<P>, hm: &mut HttpModel, opts: &Opts, prompt: &str) {
    let mut history = Vec::new();
    do_turn(gw, hm, &mut history, prompt, opts);
    let audit = gw.audit.snapshot();
    println!("\naudit ({} call(s)):", audit.len());
    for a in &audit {
        let oc = if a.outcome.len() > 120 { format!("{}…", a.outcome.chars().take(120).collect::<String>()) } else { a.outcome.clone() };
        println!("  {:<12} → {}", a.tool, oc);
    }
}

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    let mut opts = match parse_args(args) {
        Ok(o) => o,
        Err(e) if e == "help" => { usage(); exit(0); }
        Err(e) => { eprintln!("onyx: {e}"); usage(); exit(2); }
    };

    let cfg = omniroute_config();
    let workdir = env::var("ONYX_WORKDIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| env::current_dir().unwrap_or_else(|_| env::temp_dir()));

    // Policy monomorphs; pick one per invocation. Each arm owns its gateway for
    // the whole session because Gateway is not object-safe over P. The guardian
    // judge costs one extra model call per non-trusted action, so it is opt-in
    // (--guardian or ONYX_GUARDIAN=1) and chains behind BashGuard's static deny.
    let guardian_on = opts.guardian
        || env::var("ONYX_GUARDIAN").map(|v| v == "1" || v == "true").unwrap_or(false);
    let sandbox_on = opts.sandbox
        || env::var("ONYX_SANDBOX").map(|v| v == "1" || v == "true").unwrap_or(false);

    let mut tool_col = tools::default_collection(workdir.clone());
    if sandbox_on {
        let caps = sandbox::probe();
        let lvl = sandbox::select_level(&caps);
        tool_col.add(sandbox::SandboxedBash::new(lvl, Some(workdir.display().to_string())));
    }

    let subagent_model = build_model(&cfg, &opts, String::new(), Vec::new());
    tool_col.add(subagent::SpawnSubagentTool::new(subagent_model.clone(), tool_col.clone(), opts.max_steps));
    tool_col.add(subagent::SwarmTool::new(subagent_model, tool_col.clone(), opts.max_steps, 2));

    // Discover MCP tools configured in ~/.onyx/mcp.json (if present)
    if let Some(home) = env::var_os("HOME").map(PathBuf::from) {
        let mcp_cfg = home.join(".onyx/mcp.json");
        if let Ok(content) = std::fs::read_to_string(mcp_cfg) {
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(&content) {
                if let Some(servers) = val.get("mcpServers").and_then(|s| s.as_object()) {
                    for (srv_name, srv_obj) in servers {
                        if let (Some(host), Some(port)) = (
                            srv_obj.get("host").and_then(|h| h.as_str()),
                            srv_obj.get("port").and_then(|p| p.as_u64()),
                        ) {
                            let path = srv_obj.get("path").and_then(|p| p.as_str()).unwrap_or("/");
                            let client = mcp::McpClient::new(srv_name, host, port as u16, path);
                            if let Ok(tools) = client.list_tools() {
                                for t in tools {
                                    tool_col.add(mcp::McpTool::new(client.clone(), t));
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // Load active skill if requested via --skill
    let mut skill_instructions: Option<String> = None;
    if let Some(skill_name) = &opts.skill {
        if let Some(home) = env::var_os("HOME").map(PathBuf::from) {
            let mut catalog = skills::SkillCatalog::new();
            catalog.scan_dir(&home.join(".onyx/skills"));
            if let Some(sk) = catalog.get(skill_name) {
                sk.apply_to_tools(&mut tool_col);
                skill_instructions = Some(sk.instructions.clone());
            } else {
                eprintln!("onyx: skill `{skill_name}` not found in ~/.onyx/skills");
                exit(1);
            }
        }
    }

    // One type-erased gateway for every policy arm: `Gateway<dyn Policy>` is
    // Sized (the policy field is boxed), so all three arms converge on the same
    // session dispatch. --allow-all starts in Auto mode (agy's auto-approve);
    // the TUI's hint bar shows it, and we warn once here. The interactive TUI
    // replaces the approver at startup; --print/--legacy keep this one.
    let print = opts.print.clone();
    // All three arms converge on `Gateway<dyn Policy>` (Sized, because the
    // policy field is boxed). The `_boxed` constructors take the erased box
    // directly, so P is dyn at every call site.
    let mut gw = if opts.allow_all {
        Gateway::<dyn Policy>::from_boxed(tool_col, Box::new(AllowAll))
    } else if guardian_on {
        Gateway::<dyn Policy>::from_boxed_with_approver(
            tool_col,
            DynChained::new(Chained::new(
                BashGuard::default(),
                Guardian::new(HttpJudge::new(build_model(&cfg, &opts, String::new(), Vec::new()))),
            ))
            .as_dyn(),
            interactive_approve,
        )
    } else {
        Gateway::<dyn Policy>::from_boxed_with_approver(
            tool_col,
            DynChained::new(Chained::new(
                tools::taxonomy::Sandbox::standard(),
                BashGuard::default(),
            ))
            .as_dyn(),
            interactive_approve,
        )
    };
    let catalog = catalog_for(&gw);
    let defs = tools::tool_schemas(&gw.tools);
    let mut hm = build_model(&cfg, &opts, catalog, defs);
    // Inject Hermes persistent memory snapshot into system prompt
    let hermes_mem = tools::implementations::grok_build::HermesMemoryStore::default_paths();
    let mem_snapshot = hermes_mem.compile_snapshot();
    if !mem_snapshot.is_empty() {
        hm.system.push_str(&mem_snapshot);
    }
    if let Some(ins) = skill_instructions.take() {
        hm.system.push_str("\n\n");
        hm.system.push_str(&ins);
    }
    if opts.allow_all {
        *gw.mode.lock().unwrap() = tools::audit::ApprovalMode::Auto;
        eprintln!("onyx: --allow-all → AUTO approval mode (every action runs unattended)");
    }
    match &print {
        Some(p) => run_once(&gw, &mut hm, &opts, p),
        None if opts.legacy => repl(&mut gw, &mut hm, &mut opts),
        None => tui::tui_session(&mut gw, &mut hm, &mut opts),
    }
}
