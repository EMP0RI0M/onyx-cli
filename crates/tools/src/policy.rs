//! Governance policy layer (OpenBot's CEL "fail-closed" idea, in Rust types).

use crate::Args;
use std::sync::{Arc, Mutex};

/// A gateway decision. Deny carries the rule that fired (auditable). Ask
/// carries a prompt-worthy reason and defers to an interactive confirmation
/// (or, later, to an LLM judge) before executing.
#[derive(Debug, Clone, PartialEq)]
pub enum Decision {
    Allow,
    Deny(String),
    Ask(String),
}

impl Decision {
    pub fn is_allow(&self) -> bool {
        matches!(self, Decision::Allow)
    }
}

/// Run several policies in order: any Deny short-circuits, the first Ask wins
/// (unless an earlier policy already denied), otherwise Allow. Lets a fast
/// static guard (BashGuard) sit in front of slower judgement layers.
pub struct Chained<A: Policy, B: Policy> {
    pub first: A,
    pub second: B,
}

impl<A: Policy, B: Policy> Chained<A, B> {
    pub fn new(first: A, second: B) -> Self {
        Self { first, second }
    }
}

/// Same chain semantics with both legs pre-erased. Kept as its own type (not
/// `Chained<Box<dyn Policy>, Box<dyn Policy>>`) so `into_dyn` can return it
/// without nested unsizing through generic parameters.
pub struct DynChained {
    pub first: Box<dyn Policy>,
    pub second: Box<dyn Policy>,
}

impl DynChained {
    /// Erase both legs into one object-safe chain node. The TUI needs a single
    /// concrete gateway type across all policy arms (plain BashGuard vs
    /// BashGuard∘Guardian).
    pub fn new<A: Policy + 'static, B: Policy + 'static>(chain: Chained<A, B>) -> Self {
        Self { first: Box::new(chain.first), second: Box::new(chain.second) }
    }
    /// Same chain as the `dyn Policy` the gateway stores — no extra box layer.
    pub fn as_dyn(self) -> Box<dyn Policy> {
        Box::new(self)
    }
}

impl Policy for DynChained {
    fn decide(&self, tool: &str, args: &Args) -> Decision {
        let d = self.first.decide(tool, args);
        if d != Decision::Allow {
            return d;
        }
        self.second.decide(tool, args)
    }
    /// Forward an approval to whichever leg raised the `Ask`. The gateway calls
    /// this after a single decide(); we re-run decide to identify the origin so
    /// only the asking policy persists (a deny from `first` never reaches `second`).
    fn on_approval(&self, tool: &str, args: &Args, reply: crate::audit::Approval) {
        match self.first.decide(tool, args) {
            Decision::Ask(_) => self.first.on_approval(tool, args, reply),
            Decision::Allow => self.second.on_approval(tool, args, reply),
            Decision::Deny(_) => {}
        }
    }
}

impl<A: Policy, B: Policy> Policy for Chained<A, B> {
    fn decide(&self, tool: &str, args: &Args) -> Decision {
        let d = self.first.decide(tool, args);
        if d != Decision::Allow {
            return d;
        }
        self.second.decide(tool, args)
    }
    fn on_approval(&self, tool: &str, args: &Args, reply: crate::audit::Approval) {
        match self.first.decide(tool, args) {
            Decision::Ask(_) => self.first.on_approval(tool, args, reply),
            Decision::Allow => self.second.on_approval(tool, args, reply),
            Decision::Deny(_) => {}
        }
    }
}

/// Inspects an intended tool call before it runs. Implementors must fail closed:
/// an unknown/unparseable request should Deny, not Allow.
pub trait Policy: Send + Sync {
    fn decide(&self, tool: &str, args: &Args) -> Decision;

    /// Called when a human resolves one of this policy's `Ask` decisions. The
    /// gateway forwards the reply here so a stateful policy (e.g. `Ruleset`) can
    /// persist an `Always`. Default: ignore — most policies are stateless.
    fn on_approval(&self, _tool: &str, _args: &Args, _reply: crate::audit::Approval) {}
}

/// Admits everything. Only safe for tests / fully-trusted local use.
pub struct AllowAll;
impl Policy for AllowAll {
    fn decide(&self, _tool: &str, _args: &Args) -> Decision {
        Decision::Allow
    }
}

/// Rejects everything — the fail-closed default.
pub struct DenyAll;
impl Policy for DenyAll {
    fn decide(&self, tool: &str, _args: &Args) -> Decision {
        Decision::Deny(format!("default-deny policy refused `{tool}`"))
    }
}

/// Blocks `bash` calls whose command matches any deny substring (e.g. `rm -rf /`).
/// Other tools are allowed. A real port would compile CEL, not substrings.
pub struct BashGuard {
    pub deny_substrings: Vec<String>,
}

impl Default for BashGuard {
    fn default() -> Self {
        Self {
            deny_substrings: vec![
                "rm -rf /".into(),
                ":(){ :|:& };:".into(),
                "mkfs".into(),
                "dd if=/dev/zero of=/dev/".into(),
            ],
        }
    }
}

impl BashGuard {
    /// The static deny substrings this guard refuses regardless of approval
    /// mode — surfaced for `/permissions` so operators can see the hard rules.
    pub fn dangerous_patterns(&self) -> &[String] {
        &self.deny_substrings
    }
}

impl Policy for BashGuard {
    fn decide(&self, tool: &str, args: &Args) -> Decision {
        if tool != "bash" {
            return Decision::Allow;
        }
        let cmd = args.get("command").and_then(|v| v.as_str()).unwrap_or("");
        for pat in &self.deny_substrings {
            if cmd.contains(pat.as_str()) {
                return Decision::Deny(format!("bash command matched deny rule `{pat}`"));
            }
        }
        Decision::Allow
    }
}

/// Verdict from a risk classifier consulted before execution.
#[derive(Debug, Clone, PartialEq)]
pub enum Risk {
    Low,
    High(String),
}

/// The seam a model-backed judge implements: given the tool and its rendered
/// arguments, classify the risk of executing it NOW.
pub trait RiskJudge: Send + Sync {
    fn judge(&self, tool: &str, args: &Args) -> Risk;
}

/// A `Policy` that consults a `RiskJudge`: low risk passes, high risk becomes
/// `Ask` so a human confirms (or an unattended gateway fails closed by denying).
/// Inspired by Goose's guardian idea — a second model reviewing the first one's
/// intended actions — implemented clean-room with our own prompt and verdict
/// vocabulary.
pub struct Guardian<J: RiskJudge> {
    pub judge: J,
    /// Tools exempt from judgement (read-only or loop-control tools).
    pub trust: Vec<String>,
}

impl<J: RiskJudge> Guardian<J> {
    pub fn new(judge: J) -> Self {
        Self { judge, trust: vec!["terminate".into(), "ask_human".into(), "planning".into()] }
    }
}

impl<J: RiskJudge> Policy for Guardian<J> {
    fn decide(&self, tool: &str, args: &Args) -> Decision {
        if self.trust.iter().any(|t| t == tool) {
            return Decision::Allow;
        }
        match self.judge.judge(tool, args) {
            Risk::Low => Decision::Allow,
            Risk::High(reason) => Decision::Ask(if reason.is_empty() { format!("guardian flagged `{tool}`") } else { reason }),
        }
    }
}

/// What the operator chose when a rule said `Ask`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reply {
    /// Allow this one call only; no lasting change.
    Once,
    /// Allow now AND promote an `Allow` rule for the matched pattern into the
    /// live session so identical future calls are not re-asked.
    Always,
    /// Deny this call.
    Reject,
}

/// One entry in a ruleset. `permission` is the tool name (or a glob like
/// `"bash"`); `pattern` is matched against the call's *subject* — the command
/// for bash/exec tools, the target path for filesystem tools, else the raw args.
/// `action` is the verdict when both the permission and the pattern match.
#[derive(Debug, Clone)]
pub struct Rule {
    pub permission: String,
    pub pattern: String,
    pub action: Action,
}

/// The verdict a matching rule produces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Allow,
    Ask,
    Deny,
}

/// Glob match supporting `*` (any run of chars) and `?` (exactly one char).
/// Iterative backtracking — no recursion blowup on pathological patterns.
pub fn wildcard_match(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    let (mut pi, mut ti) = (0usize, 0usize);
    let (mut star, mut star_ti) = (None::<usize>, 0usize);
    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            star_ti = ti;
            pi += 1;
        } else if let Some(sp) = star {
            // Last `*` can absorb one more char; rewind to just after it.
            pi = sp + 1;
            star_ti += 1;
            ti = star_ti;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

/// A small table mapping interpreters whose "meaningful prefix" spans more than
/// one token, so `git checkout main` normalises to `git checkout` and matches a
/// `"git checkout *"` allow-rule rather than the naive first word alone. Values
/// are leading-token counts. This is deliberately tiny and local — a real port
/// would derive it from a parser, but the substring-prefix failure mode (a bare
/// `git` rule silently covering `git push --force`) is exactly what this guards.
fn command_arity(cmd: &str) -> usize {
    const TABLE: &[(&str, usize)] = &[
        ("git", 2),
        ("npm run", 3),
        ("pnpm run", 3),
        ("yarn run", 3),
        ("cargo", 2),
        ("docker compose", 3),
        ("docker", 2),
        ("systemctl", 2),
        ("apt-get", 2),
        ("brew", 2),
    ];
    // Collapse internal runs of whitespace so `npm   run` still matches "npm run".
    let lower = cmd.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase();
    for (prefix, n) in TABLE {
        if lower == *prefix || lower.starts_with(&format!("{prefix} ")) {
            return *n;
        }
    }
    1
}

/// Reduce a shell command to its human-meaningful prefix: the first `arity`
/// whitespace-separated tokens. Used as the subject for bash pattern matching.
pub fn normalize_command(cmd: &str) -> String {
    let arity = command_arity(cmd);
    cmd.split_whitespace()
        .take(arity)
        .collect::<Vec<_>>()
        .join(" ")
}

/// Extract the string a rule's `pattern` should be matched against for a call.
/// Bash/exec-family → the normalised command; filesystem/editor family with a
/// `path`/`file_path` arg → that path; everything else → the whole args object.
fn subject_for(_tool: &str, args: &Args) -> String {
    if let Some(cmd) = args.get("command").and_then(|v| v.as_str()) {
        return normalize_command(cmd);
    }
    for key in ["path", "file_path", "target", "url"] {
        if let Some(s) = args.get(key).and_then(|v| v.as_str()) {
            return s.to_string();
        }
    }
    args.to_string()
}

/// Ordered, last-match-wins ruleset — opencode's permission core, reimplemented
/// clean-room. Evaluation finds the LAST rule whose permission glob matches the
/// tool AND whose pattern glob matches the call subject; that rule's action wins.
/// When nothing matches, `default` applies. Live `always` approvals append
/// in-memory `Allow` rules (shared via `Arc<Mutex<_>>` so a gateway cloned into
/// subagent threads sees them) so a once-approved pattern stops asking.
pub struct Ruleset {
    /// Static rules from config; never mutated at runtime.
    base: Vec<Rule>,
    /// Session-scoped rules promoted by `Reply::Always`.
    live: Arc<Mutex<Vec<Rule>>>,
    pub default: Action,
}

impl Ruleset {
    pub fn new(base: Vec<Rule>) -> Self {
        Self { base, live: Arc::new(Mutex::new(Vec::new())), default: Action::Ask }
    }
    pub fn with_default(mut self, default: Action) -> Self {
        self.default = default;
        self
    }

    /// Find the winning action for a call, searching live rules after base so a
    /// freshly-promoted `always` outranks an older ask at equal specificity.
    /// Returns `(action, matched_subject)`; live rules are copied out so no
    /// reference escapes the mutex guard.
    fn find_action(&self, tool: &str, subject: &str) -> Option<Action> {
        let live = self.live.lock().unwrap();
        live.iter()
            .rev()
            .map(|r| (r.permission.as_str(), r.pattern.as_str(), r.action))
            .chain(self.base.iter().rev().map(|r| (r.permission.as_str(), r.pattern.as_str(), r.action)))
            .find(|(perm, pat, _)| wildcard_match(perm, tool) && wildcard_match(pat, subject))
            .map(|(_, _, act)| act)
    }

    /// Record an operator reply to an `Ask`. `Always` promotes a session rule
    /// for the exact (tool, subject) so identical calls stop prompting.
    pub fn record_reply(&self, tool: &str, args: &Args, reply: Reply) {
        if reply == Reply::Always {
            let subject = subject_for(tool, args);
            let mut live = self.live.lock().unwrap();
            live.push(Rule {
                permission: tool.to_string(),
                pattern: format!("{subject}*"),
                action: Action::Allow,
            });
        }
    }

    /// Number of live (session-promoted) rules — for tests and status display.
    pub fn live_len(&self) -> usize {
        self.live.lock().unwrap().len()
    }
}

impl Policy for Ruleset {
    fn decide(&self, tool: &str, args: &Args) -> Decision {
        let subject = subject_for(tool, args);
        let action = self.find_action(tool, &subject).unwrap_or(self.default);
        match action {
            Action::Allow => Decision::Allow,
            Action::Deny => Decision::Deny(format!("ruleset denied `{tool}`")),
            Action::Ask => Decision::Ask(format!("ruleset: approve `{tool}` ({subject})?")),
        }
    }

    /// Map the gateway's three-way approval onto rule persistence: only
    /// `Always` leaves a lasting session rule; `Once`/`Reject` change nothing.
    fn on_approval(&self, tool: &str, args: &Args, reply: crate::audit::Approval) {
        let r = match reply {
            crate::audit::Approval::Always => Reply::Always,
            crate::audit::Approval::Once => Reply::Once,
            crate::audit::Approval::Reject => Reply::Reject,
        };
        self.record_reply(tool, args, r);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    struct ScriptedJudge(Risk);
    impl RiskJudge for ScriptedJudge {
        fn judge(&self, _tool: &str, _args: &Args) -> Risk {
            self.0.clone()
        }
    }

    #[test]
    fn guardian_allows_low_risk() {
        let g = Guardian::new(ScriptedJudge(Risk::Low));
        assert_eq!(g.decide("bash", &json!({"command": "ls"})), Decision::Allow);
    }

    #[test]
    fn guardian_maps_high_to_ask_with_reason() {
        let g = Guardian::new(ScriptedJudge(Risk::High("erases data".into())));
        assert_eq!(g.decide("editor", &json!({"command": "delete"})), Decision::Ask("erases data".into()));
    }

    #[test]
    fn guardian_skips_judging_trusted_tools() {
        // High-risk judge, but loop-control tools never reach it.
        let g = Guardian::new(ScriptedJudge(Risk::High("should not matter".into())));
        for tool in ["terminate", "ask_human", "planning"] {
            assert_eq!(g.decide(tool, &json!({})), Decision::Allow, "{tool} is trusted");
        }
    }

    #[test]
    fn chained_denies_before_asking() {
        // BashGuard denies the fork bomb statically; the guardian (which would
        // only Ask) must never get a chance to soften that.
        let chain = Chained::new(BashGuard::default(), Guardian::new(ScriptedJudge(Risk::High("meh".into()))));
        match chain.decide("bash", &json!({"command": ":(){ :|:& };:"})) {
            Decision::Deny(_) => {}
            other => panic!("expected Deny, got {other:?}"),
        }
        // A benign command passes BashGuard, then the guardian asks.
        assert_eq!(
            chain.decide("bash", &json!({"command": "rm ./tmp.txt"})),
            Decision::Ask("meh".into())
        );
    }

    #[test]
    fn wildcard_matcher_basics() {
        assert!(wildcard_match("git checkout *", "git checkout main"));
        assert!(wildcard_match("*", "anything at all"));
        assert!(wildcard_match("src/*.rs", "src/lib.rs"));
        // `*` is a general wildcard (crosses `/`), matching opencode's semantics.
        assert!(wildcard_match("src/*.rs", "src/nested/lib.rs"));
        assert!(!wildcard_match("src/?lib.rs", "src/a/b/lib.rs"));
        assert!(wildcard_match("a?c", "abc"));
        assert!(!wildcard_match("a?c", "ac"));
        assert!(wildcard_match("npm run *", "npm run build"));
    }

    #[test]
    fn command_normalization_respects_arity() {
        assert_eq!(normalize_command("git checkout main -b foo"), "git checkout");
        assert_eq!(normalize_command("ls -la /tmp"), "ls");
        assert_eq!(normalize_command("cargo build --release"), "cargo build");
        assert_eq!(normalize_command("  npm   run   dev  "), "npm run dev");
    }

    #[test]
    fn ruleset_last_match_wins_and_defaults() {
        let rs = Ruleset::new(vec![
            Rule { permission: "bash".into(), pattern: "*".into(), action: Action::Ask },
            // Later rule wins for read-only git subcommands.
            Rule { permission: "bash".into(), pattern: "git status*".into(), action: Action::Allow },
            // Even later: but force-push is denied outright.
            Rule { permission: "bash".into(), pattern: "git push*".into(), action: Action::Deny },
        ])
        .with_default(Action::Deny);

        assert!(rs.decide("bash", &json!({"command": "git status"})).is_allow());
        assert!(matches!(rs.decide("bash", &json!({"command": "git push origin main"})), Decision::Deny(_)));
        assert!(matches!(rs.decide("bash", &json!({"command": "rm file"})), Decision::Ask(_)));
        // Unknown tool with no matching rule → default deny (fail closed).
        assert!(matches!(rs.decide("mcp__x__y", &json!({})), Decision::Deny(_)));
    }

    #[test]
    fn always_reply_promotes_session_rule() {
        let rs = Ruleset::new(vec![Rule {
            permission: "bash".into(),
            pattern: "*".into(),
            action: Action::Ask,
        }]);
        // First call asks.
        let d1 = rs.decide("bash", &json!({"command": "echo hi"}));
        assert!(matches!(d1, Decision::Ask(_)));
        assert_eq!(rs.live_len(), 0);

        // Operator says "always" → a session Allow rule is recorded.
        rs.record_reply("bash", &json!({"command": "echo hi"}), Reply::Always);
        assert_eq!(rs.live_len(), 1);

        // Same prefix now auto-allows without asking.
        let d2 = rs.decide("bash", &json!({"command": "echo hi there"}));
        assert!(d2.is_allow(), "promoted rule should allow, got {d2:?}");
    }

    #[test]
    fn once_and_reject_do_not_persist() {
        let rs = Ruleset::new(vec![Rule {
            permission: "editor".into(),
            pattern: "*".into(),
            action: Action::Ask,
        }]);
        rs.record_reply("editor", &json!({"path": "/tmp/a"}), Reply::Once);
        rs.record_reply("editor", &json!({"path": "/tmp/b"}), Reply::Reject);
        assert_eq!(rs.live_len(), 0, "only Always promotes a rule");
    }

    #[test]
    fn gateway_always_reply_stops_the_next_prompt() {
        use crate::audit::{Approval, Gateway};
        use crate::{Tool, ToolCollection};
        struct FakeBash;
        impl Tool for FakeBash {
            fn name(&self) -> &str { "bash" }
            fn description(&self) -> &str { "x" }
            fn execute(&self, a: &Args) -> Result<crate::Output, crate::ToolError> {
                Ok(crate::Output::ok(
                    a.get("command").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                ))
            }
        }
        let mut c = ToolCollection::new();
        c.add(FakeBash);
        let rs = Ruleset::new(vec![Rule {
            permission: "bash".into(),
            pattern: "*".into(),
            action: Action::Ask,
        }]);
        // Count how many times the operator is actually prompted.
        let prompts = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let p2 = prompts.clone();
        let gw = Gateway::with_approver(c, rs, move |_, _, _| {
            p2.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Approval::Always
        });
        // First call asks → approver says always → executes and promotes a rule.
        assert_eq!(gw.call("bash", &json!({"command": "echo hi"})).unwrap().content, "echo hi");
        assert_eq!(gw.policy.live_len(), 1);
        assert_eq!(prompts.load(std::sync::atomic::Ordering::SeqCst), 1);
        // A same-prefix call now auto-allows: the approver is never consulted.
        assert!(gw.call("bash", &json!({"command": "echo hi again"})).is_ok());
        assert_eq!(prompts.load(std::sync::atomic::Ordering::SeqCst), 1, "second call must not prompt");
    }
}
