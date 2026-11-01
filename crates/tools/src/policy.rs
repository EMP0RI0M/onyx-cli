//! Governance policy layer (OpenBot's CEL "fail-closed" idea, in Rust types).

use crate::Args;

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
}

impl<A: Policy, B: Policy> Policy for Chained<A, B> {
    fn decide(&self, tool: &str, args: &Args) -> Decision {
        let d = self.first.decide(tool, args);
        if d != Decision::Allow {
            return d;
        }
        self.second.decide(tool, args)
    }
}

/// Inspects an intended tool call before it runs. Implementors must fail closed:
/// an unknown/unparseable request should Deny, not Allow.
pub trait Policy: Send + Sync {
    fn decide(&self, tool: &str, args: &Args) -> Decision;
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
}
