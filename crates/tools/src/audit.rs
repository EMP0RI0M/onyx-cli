//! Audit log + the Gateway: the single entry point that decides, records, then runs.
//! Mirrors OpenBot's rule that no action happens without an audit row existing first.

use crate::policy::{Decision, Policy};
use crate::{Args, Output, ToolCollection, ToolError};
use std::cell::RefCell;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone)]
pub struct AuditEntry {
    pub ts_ms: u64,
    pub tool: String,
    pub args: String,
    pub outcome: String,
}

#[derive(Default)]
pub struct AuditLog {
    entries: Mutex<Vec<AuditEntry>>,
}

impl AuditLog {
    pub fn record(&self, tool: &str, args: &str, outcome: &str) {
        let ts_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        self.entries.lock().unwrap().push(AuditEntry {
            ts_ms,
            tool: tool.to_string(),
            args: args.to_string(),
            outcome: outcome.to_string(),
        });
    }
    pub fn snapshot(&self) -> Vec<AuditEntry> {
        self.entries.lock().unwrap().clone()
    }
}

pub struct Gateway<P: Policy + ?Sized> {
    pub tools: ToolCollection,
    /// Boxed so `Gateway<dyn Policy>` is itself Sized and can be handed out as
    /// a plain reference (the TUI worker does exactly that).
    pub policy: Box<P>,
    pub audit: AuditLog,
    /// Resolves `Decision::Ask`: given (tool, args, reason), returns whether the
    /// human/judge approves. None means nobody can approve → Ask fails closed
    /// to Deny. The CLI installs an interactive y/n closure here.
    approver: RefCell<Option<Box<dyn Fn(&str, &str, &str) -> bool + Send + Sync>>>,
    /// UI-level approval mode (agy's shift+tab cycle). `Auto` bypasses both the
    /// policy and the approver — the caller opted in explicitly (--allow-all or
    /// the TUI toggle); the binary prints a warning when it is on. Interior-
    /// mutable so the TUI can cycle it while a worker holds a shared reference.
    pub mode: Mutex<ApprovalMode>,
}

/// The approver RefCell breaks auto-Sync. Sound by discipline: it is borrowed
/// mutably only on the main thread (construction and set_approver, which the
/// TUI performs once at startup); the worker thread's `call` takes a short
/// immutable borrow to resolve the closure, then blocks inside it while the
/// main thread waits on AskCtl — so the two threads never overlap on this cell.
unsafe impl<P: Policy + ?Sized + Sync> Sync for Gateway<P> {}

/// Interactive approval modes, mirroring agy's shift+tab cycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalMode {
    /// Policy decides; `Ask` goes to the approver (default).
    Normal,
    /// Read-only tools auto-allowed by policy; edits run without asking.
    AcceptEdits,
    /// Everything runs unasked (what --allow-all arms).
    Auto,
}

impl ApprovalMode {
    pub fn label(self) -> &'static str {
        match self {
            ApprovalMode::Normal => "normal",
            ApprovalMode::AcceptEdits => "accept-edits",
            ApprovalMode::Auto => "auto",
        }
    }
    /// shift+tab order: normal → accept-edits → auto → normal.
    pub fn next(self) -> ApprovalMode {
        match self {
            ApprovalMode::Normal => ApprovalMode::AcceptEdits,
            ApprovalMode::AcceptEdits => ApprovalMode::Auto,
            ApprovalMode::Auto => ApprovalMode::Normal,
        }
    }
}

impl<P: Policy> Gateway<P> {
    pub fn new(tools: ToolCollection, policy: P) -> Self {
        Self { tools, policy: Box::new(policy), audit: AuditLog::default(), approver: RefCell::new(None), mode: Mutex::new(ApprovalMode::Normal) }
    }

    pub fn with_approver(
        tools: ToolCollection,
        policy: P,
        approver: impl Fn(&str, &str, &str) -> bool + Send + Sync + 'static,
    ) -> Self {
        Self { tools, policy: Box::new(policy), audit: AuditLog::default(), approver: RefCell::new(Some(Box::new(approver))), mode: Mutex::new(ApprovalMode::Normal) }
    }
}

impl<P: Policy + ?Sized> Gateway<P> {
    /// Build a gateway whose policy is already type-erased (`Box<dyn Policy>`),
    /// so `Gateway::<dyn Policy>::new(...)` is callable — the by-value `new`
    /// above requires `P: Sized` and can't serve the unsized target type.
    pub fn from_boxed(tools: ToolCollection, policy: Box<P>) -> Self {
        Self { tools, policy, audit: AuditLog::default(), approver: RefCell::new(None), mode: Mutex::new(ApprovalMode::Normal) }
    }

    pub fn from_boxed_with_approver(
        tools: ToolCollection,
        policy: Box<P>,
        approver: impl Fn(&str, &str, &str) -> bool + Send + Sync + 'static,
    ) -> Self {
        Self { tools, policy, audit: AuditLog::default(), approver: RefCell::new(Some(Box::new(approver))), mode: Mutex::new(ApprovalMode::Normal) }
    }

    /// Swap the approver closure at runtime (the TUI installs its own so
    /// confirmations render inside the transcript instead of raw stdin).
    /// Takes `&self`: the approver is an interior-mutable UI hook, and the
    /// worker thread holds only a shared reference to the gateway. The
    /// RefCell is touched mutably only on the main thread; the worker reads
    /// it while frozen in AskCtl — see the Sync impl below.
    pub fn set_approver(
        &self,
        approver: impl Fn(&str, &str, &str) -> bool + Send + Sync + 'static,
    ) {
        *self.approver.borrow_mut() = Some(Box::new(approver));
    }

    /// The only way to invoke a tool: resolve → policy → audit → execute.
    /// Denied calls are still audited (an attempted action is a recordable event).
    pub fn call(&self, name: &str, args: &Args) -> Result<Output, ToolError> {
        let args_str = serde_json::to_string(args).unwrap_or_else(|_| "<unserializable>".into());
        let tool = self
            .tools
            .get(name)
            .ok_or_else(|| ToolError::Other(format!("unknown tool `{name}`")))?;

        if *self.mode.lock().unwrap() == ApprovalMode::Auto {
            self.audit.record(name, &args_str, "AUTO");
            return self.execute(name, &args_str, tool, args);
        }
        match self.policy.decide(name, args) {
            Decision::Deny(rule) => {
                self.audit.record(name, &args_str, &format!("DENY: {rule}"));
                Err(ToolError::Denied(rule))
            }
            Decision::Ask(reason) => {
                let approved = self
                    .approver
                    .borrow()
                    .as_ref()
                    .map(|f| f(name, &args_str, &reason))
                    .unwrap_or(false);
                if approved {
                    self.audit.record(name, &args_str, &format!("ASK-OK: {reason}"));
                    self.execute(name, &args_str, tool, args)
                } else {
                    let rule = format!("confirmation refused ({reason})");
                    self.audit.record(name, &args_str, &format!("DENY: {rule}"));
                    Err(ToolError::Denied(rule))
                }
            }
            Decision::Allow => self.execute(name, &args_str, tool, args),
        }
    }

    fn execute(
        &self,
        name: &str,
        args_str: &str,
        tool: &dyn crate::Tool,
        args: &Args,
    ) -> Result<Output, ToolError> {
        match tool.execute(args) {
            Ok(out) => {
                let tag = if out.is_error { "ERR" } else { "OK" };
                self.audit.record(name, args_str, tag);
                Ok(out)
            }
            Err(e) => {
                self.audit.record(name, args_str, &format!("FAIL: {e}"));
                Err(e)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::BashGuard;
    use crate::{Tool, ToolCollection};
    use serde_json::json;

    struct FakeBash;
    impl Tool for FakeBash {
        fn name(&self) -> &str { "bash" }
        fn description(&self) -> &str { "x" }
        fn execute(&self, a: &Args) -> Result<Output, ToolError> {
            Ok(Output::ok(a.get("command").and_then(|v| v.as_str()).unwrap_or("").to_string()))
        }
    }

    #[test]
    fn gateway_denies_and_audits() {
        let mut c = ToolCollection::new();
        c.add(FakeBash);
        let gw = Gateway::new(c, BashGuard::default());

        let ok = gw.call("bash", &json!({"command": "ls"})).unwrap();
        assert!(!ok.is_error);

        let denied = gw.call("bash", &json!({"command": "rm -rf /"}));
        assert!(matches!(denied, Err(ToolError::Denied(_))));

        let log = gw.audit.snapshot();
        assert_eq!(log.len(), 2);
        assert!(log[0].outcome == "OK");
        assert!(log[1].outcome.starts_with("DENY:"));
    }

    struct AskOnce;
    impl Policy for AskOnce {
        fn decide(&self, _tool: &str, _args: &Args) -> Decision {
            Decision::Ask("destructive-looking".into())
        }
    }

    #[test]
    fn ask_without_approver_fails_closed() {
        let mut c = ToolCollection::new();
        c.add(FakeBash);
        let gw = Gateway::new(c, AskOnce);
        let res = gw.call("bash", &json!({"command": "ls"}));
        assert!(matches!(res, Err(ToolError::Denied(_))));
        assert!(gw.audit.snapshot()[0].outcome.starts_with("DENY:"));
    }

    #[test]
    fn ask_with_approver_executes_and_audits() {
        let mut c = ToolCollection::new();
        c.add(FakeBash);
        let gw = Gateway::with_approver(c, AskOnce, |_n, _a, _r| true);
        let out = gw.call("bash", &json!({"command": "ls"})).unwrap();
        assert_eq!(out.content, "ls");
        assert!(gw.audit.snapshot()[0].outcome.starts_with("ASK-OK:"));
    }

    #[test]
    fn chained_short_circuits_on_deny() {
        use crate::policy::{AllowAll, Chained};
        let mut c = ToolCollection::new();
        c.add(FakeBash);
        // BashGuard denies rm -rf /; second policy would allow it.
        let gw = Gateway::new(c, Chained::new(BashGuard::default(), AllowAll));
        assert!(matches!(gw.call("bash", &json!({"command": "rm -rf /"})), Err(ToolError::Denied(_))));
        assert!(gw.call("bash", &json!({"command": "ls"})).is_ok());
    }

    #[test]
    fn auto_mode_runs_even_when_policy_asks() {
        let mut c = ToolCollection::new();
        c.add(FakeBash);
        // AskOnce would deny without an approver; Auto bypasses the policy.
        let mut gw = Gateway::new(c, AskOnce);
        *gw.mode.lock().unwrap() = ApprovalMode::Auto;
        assert_eq!(gw.call("bash", &json!({"command": "ls"})).unwrap().content, "ls");
        assert!(gw.audit.snapshot()[0].outcome == "AUTO");
    }

    #[test]
    fn mode_cycle_normal_edits_auto() {
        assert_eq!(ApprovalMode::Normal.next(), ApprovalMode::AcceptEdits);
        assert_eq!(ApprovalMode::AcceptEdits.next(), ApprovalMode::Auto);
        assert_eq!(ApprovalMode::Auto.next(), ApprovalMode::Normal);
    }

    #[test]
    fn set_approver_replaces_the_resolver() {
        let mut c = ToolCollection::new();
        c.add(FakeBash);
        let gw = Gateway::with_approver(c, AskOnce, |_n, _a, _r| false);
        assert!(matches!(gw.call("bash", &json!({"command": "ls"})), Err(ToolError::Denied(_))));
        gw.set_approver(|_n, _a, _r| true);
        assert!(gw.call("bash", &json!({"command": "ls"})).is_ok());
    }
}
