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
use std::net::{TcpStream, ToSocketAddrs};
use std::path::PathBuf;
use std::process::exit;
use std::time::Duration;
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

/// Build the per-turn capability retriever from the live gateway + skill catalog,
/// or `None` when retrieval is disabled.
///
/// Deterministic BM25 retrieval is activated by default:
/// - Unset: BM25 enabled.
/// - "1", "true", "bm25": BM25 enabled.
/// - "0", "false", "off": explicitly disabled.
/// - "hybrid": hybrid retrieval (only when embeddings & models are available).
/// `ONYX_USAGE_RANKING=1` additionally wires the adaptive usage arm.
fn build_selector(gw: &Gateway<dyn Policy>) -> Option<agent::retrieval::CapabilitySelector> {
    let mode = env::var("ONYX_RETRIEVAL").unwrap_or_else(|_| "bm25".to_string()).to_lowercase();
    let enabled = match mode.trim() {
        "0" | "false" | "off" => false,
        _ => true,
    };
    if !enabled {
        return None;
    }
    let tool_corpus: Vec<onyx_retrieval::Tool> = gw
        .tools
        .names()
        .iter()
        .filter_map(|name| gw.tools.get(name))
        .map(|t| onyx_retrieval::Tool {
            id: t.name().to_string(),
            name: t.name().to_string(),
            description: t.description().to_string(),
            experimental_searchable_description: None,
            input_schema: t.schema(),
            output_schema: serde_json::json!({}),
        })
        .collect();

    let mut catalog = skills::SkillCatalog::new();
    catalog.scan_all_default();
    let skill_corpus: Vec<onyx_retrieval::Skill> = catalog
        .all()
        .into_iter()
        .map(|sk| onyx_retrieval::Skill {
            id: sk.name.clone(),
            name: sk.name.clone(),
            description: sk.description.clone(),
            experimental_searchable_description: None,
            tags: vec![],
            tools: sk.tools_allowed.clone(),
            metadata: Default::default(),
            body: sk.instructions.clone(),
        })
        .collect();

    let usage = env::var("ONYX_USAGE_RANKING").map(|v| v == "1" || v == "true").unwrap_or(false);
    let mut sel = agent::retrieval::CapabilitySelector::new()
        .with_tools(tool_corpus)
        .with_skills(skill_corpus);
    if usage {
        sel.enable_usage_ranking();
    }
    Some(sel)
}

struct Opts {
    doctor: bool,
    model: Option<String>,
    max_steps: usize,
    allow_all: bool,
    guardian: bool,
    sandbox: bool,
    skill: Option<String>,
    resume: Option<String>,
    fresh: bool,
    print: Option<String>,
    legacy: bool,
}

fn parse_args(args: Vec<String>) -> Result<Opts, String> {
    let mut o = Opts { doctor: false, model: None, max_steps: 8, allow_all: false, guardian: false, sandbox: false, skill: None, resume: None, fresh: false, print: None, legacy: false };
    let mut i = 0;
    let mut positional: Vec<String> = Vec::new();
    while i < args.len() {
        match args[i].as_str() {
            "doctor" | "--doctor" => o.doctor = true,
            "--fresh" | "-f" => o.fresh = true,
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
            "--resume" | "-r" => {
                i += 1;
                o.resume = Some(args.get(i).ok_or("--resume needs a task ID or session ID")?.clone());
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
        "usage: onyx [--model auto] [--max-steps N] [--allow-all] [--guardian] [--sandbox] [--skill NAME] [--resume TASK_ID]\n\
         \t  [--fresh] [--print PROMPT | --legacy]\n\
         \t  onyx doctor\n\
         env: ONYX_BASE_URL (default http://127.0.0.1:20128/v1), ONYX_API_KEY, ONYX_MODEL,\n\
              ONYX_MAX_TOKENS, ONYX_WORKDIR, ONYX_GUARDIAN=1 (LLM risk judge before actions),\n\
              ONYX_SANDBOX=1 (kernel-enforced sandbox for bash)"
    );
}

fn doctor_result(label: &str, result: Result<String, String>, failures: &mut usize) {
    match result {
        Ok(detail) => println!("[OK]   {label}: {detail}"),
        Err(detail) => {
            *failures += 1;
            println!("[FAIL] {label}: {detail}");
        }
    }
}

fn base_path(base: &str) -> String {
    let after_scheme = base.split_once("://").map(|(_, rest)| rest).unwrap_or(base);
    let path = after_scheme
        .find('/')
        .map(|idx| &after_scheme[idx..])
        .unwrap_or("");
    path.trim_end_matches('/').to_string()
}

fn doctor_mcp(failures: &mut usize) {
    let Some(home) = env::var_os("HOME").map(PathBuf::from) else {
        println!("[WARN] HOME: not set; skipped MCP configuration");
        return;
    };
    let path = home.join(".onyx/mcp.json");
    let content = match std::fs::read_to_string(&path) {
        Ok(content) => content,
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            println!("[WARN] MCP: no configuration at {}", path.display());
            return;
        }
        Err(err) => {
            *failures += 1;
            println!("[FAIL] MCP: cannot read {}: {err}", path.display());
            return;
        }
    };
    let value: serde_json::Value = match serde_json::from_str(&content) {
        Ok(value) => value,
        Err(err) => {
            *failures += 1;
            println!("[FAIL] MCP: invalid JSON in {}: {err}", path.display());
            return;
        }
    };
    let Some(servers) = value.get("mcpServers").and_then(|v| v.as_object()) else {
        *failures += 1;
        println!("[FAIL] MCP: expected an object at `mcpServers`");
        return;
    };
    if servers.is_empty() {
        println!("[WARN] MCP: configuration contains no servers");
        return;
    }
    for (name, server) in servers {
        let host = server.get("host").and_then(|v| v.as_str());
        let port = server.get("port").and_then(|v| v.as_u64()).and_then(|p| u16::try_from(p).ok());
        let (Some(host), Some(port)) = (host, port) else {
            *failures += 1;
            println!("[FAIL] MCP {name}: expected string `host` and u16 `port`");
            continue;
        };
        let address = match (host, port).to_socket_addrs().ok().and_then(|mut addrs| addrs.next()) {
            Some(address) => address,
            None => {
                *failures += 1;
                println!("[FAIL] MCP {name}: cannot resolve {host}:{port}");
                continue;
            }
        };
        match TcpStream::connect_timeout(&address, Duration::from_secs(2)) {
            Ok(_) => println!("[OK]   MCP {name}: reachable at {host}:{port}"),
            Err(err) => {
                *failures += 1;
                println!("[FAIL] MCP {name}: {host}:{port} unreachable: {err}");
            }
        }
    }
}

fn run_doctor() -> i32 {
    println!("onyx doctor");
    println!("-----------");
    let cfg = omniroute_config();
    println!("Gateway: {}:{} ({})", cfg.host, cfg.port, cfg.base);
    println!("Model: {}", cfg.model);
    println!("API key: {}", if cfg.key.is_empty() { "not set (may be valid for local gateways)" } else { "set (value hidden)" });

    let mut failures = 0;
    doctor_result(
        "Configuration",
        if cfg.base.is_empty() || cfg.host.is_empty() {
            Err("gateway base URL is empty".into())
        } else {
            Ok("loaded".into())
        },
        &mut failures,
    );

    let workdir = env::var("ONYX_WORKDIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| env::current_dir().unwrap_or_else(|_| env::temp_dir()));
    doctor_result(
        "Workdir",
        match std::fs::metadata(&workdir) {
            Ok(meta) if meta.is_dir() => Ok(workdir.display().to_string()),
            Ok(_) => Err(format!("{} is not a directory", workdir.display())),
            Err(err) => Err(format!("{}: {err}", workdir.display())),
        },
        &mut failures,
    );

    let health_base = base_path(&cfg.base);
    let health_paths = if health_base.is_empty() {
        vec!["/health".to_string()]
    } else {
        vec![format!("{health_base}/health"), "/health".to_string()]
    };
    let mut health = Err("no health endpoint responded".to_string());
    for path in health_paths {
        let auth = (!cfg.key.is_empty()).then_some(cfg.key.as_str());
        match agent::model::http_request("GET", &cfg.host, cfg.port, &path, auth, None) {
            Ok(body) => {
                health = Ok(format!("{path} responded ({})", body.trim().chars().take(80).collect::<String>()));
                break;
            }
            Err(err) => health = Err(err),
        }
    }
    doctor_result("Gateway health", health, &mut failures);

    let model_probe = agent::model::HttpModel::from_cfg(&cfg, String::new(), Vec::new()).list_models();
    doctor_result(
        "Model discovery",
        model_probe.map(|models| {
            if models.is_empty() {
                "endpoint responded but returned no models".to_string()
            } else {
                format!("{} model(s) available", models.len())
            }
        }),
        &mut failures,
    );

    doctor_mcp(&mut failures);
    println!();
    if failures == 0 {
        println!("Doctor found no blocking issues.");
        0
    } else {
        println!("Doctor found {failures} blocking issue(s).");
        1
    }
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
    sel: Option<&agent::retrieval::CapabilitySelector>,
) {
    let mut agent = ToolCallAgent::new(gw);
    if let Some(sel) = sel {
        agent = agent.with_selector(sel);
    }
    agent.max_steps = opts.max_steps;

    let workdir = std::env::var("ONYX_WORKDIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::current_dir().unwrap_or_else(|_| std::env::temp_dir()));

    let assembler = agent::context_assembler::ContextAssembler::new(
        &workdir,
        agent::context_assembler::ContextBudgetConfig::default(),
    );
    agent = agent.with_context_assembler(&assembler);

    // If genuine engineering task, create or load checkpoint
    if agent::autonomous::is_engineering_task(prompt) {
        let task_id = format!("task-{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0));
        let arch = assembler.get_arch_ref();
        let engine = agent::autonomous::AutonomousEngine::new(&arch, &workdir);
        let plan = engine.plan_task(&task_id, prompt);
        let mut cp = agent::autonomous::TaskCheckpoint::new(&task_id, prompt, plan);
        let _ = cp.save_atomic(&workdir);
        let cp_arc = std::sync::Arc::new(std::sync::Mutex::new(cp));
        agent = agent.with_checkpoint(cp_arc);
    }

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
                let (clean_m, _) = agent::repetition::strip_think_tags(&m.content);
                let one = clean_m.replace('\n', " ");
                let one = if one.len() > 200 { format!("{}…", one.chars().take(200).collect::<String>()) } else { one };
                println!("{tag}│ {one}");
            }
            let (clean_ans, _) = agent::repetition::strip_think_tags(&rep.answer);
            println!("\nANSWER: {}", clean_ans);
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
                let mut catalog = skills::SkillCatalog::new();
                catalog.scan_all_default();
                if let Some(sk) = catalog.get(name) {
                    hm.system = sk.augment_system_prompt(&hm.system);
                    println!("loaded skill: {}", sk.name);
                } else {
                    println!("skill `{name}` not found in any skill directory (try /skills)");
                }
            } else {
                println!("usage: /skill <name> (or /skills to list all)");
            }
        }
        "/skills" => {
            let mut catalog = skills::SkillCatalog::new();
            catalog.scan_all_default();
            println!("Available skills ({} total):", catalog.count());
            let mut sorted: Vec<_> = catalog.skills.values().collect();
            sorted.sort_by(|a, b| a.name.cmp(&b.name));
            for sk in sorted {
                let desc = if sk.description.is_empty() {
                    "".to_string()
                } else {
                    format!(" — {}", sk.description)
                };
                println!("  • /skill {}{}", sk.name, desc);
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
fn repl<P: Policy + ?Sized>(gw: &Gateway<P>, hm: &mut HttpModel, opts: &mut Opts, sel: Option<&agent::retrieval::CapabilitySelector>) {
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
        do_turn(gw, hm, &mut history, line, opts, sel);
    }
    println!("\naudit ({} call(s) this session)", gw.audit.snapshot().len());
}

fn build_model(cfg: &agent::model::GatewayCfg, opts: &Opts, catalog: String, defs: Vec<serde_json::Value>) -> HttpModel {
    let mut hm = HttpModel::from_cfg(cfg, catalog, defs);
    if let Some(m) = &opts.model {
        hm.model = m.clone();
    }
    let mut cat = skills::SkillCatalog::new();
    cat.scan_all_default();
    let mut skill_names: Vec<_> = cat.skills.keys().cloned().collect();
    skill_names.sort();
    let skill_summary = format!(
        "You are onyx, a precise agentic coding assistant. You have {} installed skills available in your workspace (loadable via /skill <name>): {}. Use tools to act; when done, call terminate with the final answer.",
        cat.count(),
        skill_names.join(", ")
    );
    hm.system = skill_summary;
    hm
}

fn run_once<P: Policy + ?Sized>(gw: &Gateway<P>, hm: &mut HttpModel, opts: &Opts, prompt: &str, sel: Option<&agent::retrieval::CapabilitySelector>) {
    let mut history = Vec::new();
    do_turn(gw, hm, &mut history, prompt, opts, sel);
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

    if opts.doctor {
        exit(run_doctor());
    }

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
        let mut catalog = skills::SkillCatalog::new();
        catalog.scan_all_default();
        if let Some(sk) = catalog.get(skill_name) {
            sk.apply_to_tools(&mut tool_col);
            skill_instructions = Some(sk.instructions.clone());
        } else {
            eprintln!("onyx: skill `{skill_name}` not found in any skill directory");
            exit(1);
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
    let sel = build_selector(&gw);
    // Handle --resume <task-id>
    if let Some(target_id) = &opts.resume {
        println!("Checking checkpoint recovery for task: {target_id}...");
        if let Some(cp) = agent::autonomous::TaskCheckpoint::load_checkpoint(&workdir, target_id) {
            println!("Loaded checkpoint for task `{}`:", cp.task_id);
            println!("  Objective: {}", cp.objective);
            println!("  Completed steps: {:?}", cp.completed_steps);
            println!("  Remaining steps: {:?}", cp.remaining_steps);
            println!("  Modified files: {:?}", cp.modified_files);

            let (unchanged, modified_or_missing) = cp.validate_file_hashes(&workdir);
            if !modified_or_missing.is_empty() {
                println!("  ⚠️ WARNING: Files modified outside checkpoint: {:?}", modified_or_missing);
            } else {
                println!("  ✓ File integrity verified: {} file(s) intact", unchanged.len());
            }

            if !cp.verification_results.is_empty() {
                println!("  Previous verifications: {:?}", cp.verification_results);
            }

            // Restore conversation history if transcript exists
            let mut resumed_history = Vec::new();
            if let Some(sess_id) = &cp.session_id {
                let sess_path = agent::session::sessions_dir().join(format!("{sess_id}.jsonl"));
                resumed_history = agent::session::load_messages(&sess_path);
            }
            if resumed_history.is_empty() {
                resumed_history.push(Message::text("system", hm.system.clone()));
                resumed_history.push(Message::text("user", format!("Resume task {}: {}", cp.task_id, cp.objective)));
            }

            println!("Resuming task execution with {} history message(s)...", resumed_history.len());
            let cp_arc = std::sync::Arc::new(std::sync::Mutex::new(cp));
            let assembler = agent::context_assembler::ContextAssembler::new(
                &workdir,
                agent::context_assembler::ContextBudgetConfig::default(),
            );
            let mut agent = ToolCallAgent::new(&gw)
                .with_context_assembler(&assembler)
                .with_checkpoint(cp_arc);
            if let Some(s) = sel.as_ref() {
                agent = agent.with_selector(s);
            }
            agent.max_steps = opts.max_steps;

            match agent.run_turn(&mut hm, &format!("Continue task {}", target_id), &mut resumed_history) {
                Ok(rep) => {
                    println!("\nANSWER: {}", rep.answer);
                }
                Err(e) => {
                    eprintln!("Execution error during resumed turn: {:?}", e);
                }
            }
            return;
        } else {
            eprintln!("onyx: no checkpoint found for task `{target_id}` in .onyx/checkpoints/");
            exit(1);
        }
    }

    match &print {
        Some(p) => run_once(&gw, &mut hm, &opts, p, sel.as_ref()),
        None if opts.legacy => repl(&mut gw, &mut hm, &mut opts, sel.as_ref()),
        None => tui::tui_session(&mut gw, &mut hm, &mut opts, sel.as_ref()),
    }
}
