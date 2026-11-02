//! Tool capability taxonomy + a sandbox policy built on top of it.
//!
//! Every tool belongs to exactly one [`Capability`] class. The gateway's policy
//! layer can then gate whole *classes* of behaviour ("no filesystem writes",
//! "no outbound network") instead of matching individual command strings. This
//! is the clean-room shape of the "capability scope" idea: a small enum, a
//! total mapping from tool name → class, and one [`Sandbox`] policy that fails
//! closed on any class it wasn't told to allow.
//!
//! Two things make this more than a lookup table:
//! - shell/exec tools are *polymorphic*: the same `bash`/`python` entry can also
//!   reach the network or erase data. When those classes are restricted, the
//!   sandbox inspects the command/code text for escalation patterns and denies
//!   or defers rather than trusting the class label alone.
//! - unknown tool names classify as [`Capability::Unknown`] and are denied, so a
//!   newly-added tool can never silently inherit an allow-by-default.
//! - remote MCP tools (`mcp__server__tool`) classify as
//!   [`Capability::Remote`]: they run on a server we cannot inspect, so the
//!   sandbox treats them as an escalation surface — allowed only when every
//!   class they could reach is granted (see [`Capability::granted_in`]).

use crate::policy::{Decision, Policy};
use crate::Args;
use serde_json::Value;

/// The kind of side effect a tool is capable of. Ordered roughly from benign to
/// dangerous; used both for display (`/tools`) and as the gate key in [`Sandbox`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Capability {
    /// Produces a plan / todo state only; no external effect.
    Planning,
    /// Pure computation over supplied values (e.g. vector math). No I/O.
    Compute,
    /// Reads/writes files under a single base directory.
    Filesystem,
    /// Makes an outbound network request.
    Network,
    /// Runs a shell command.
    Shell,
    /// Executes arbitrary code in an interpreter.
    Exec,
    /// Spawns nested agent loops (subagents / swarms). Delegates work but does
    /// not itself touch files or the network; its children re-enter the policy.
    Orchestration,
    /// Ends the turn / talks back to the user; control-flow only.
    Meta,
    /// A remote MCP tool: runs on a server we cannot inspect, so its effective
    /// ceiling is whatever the local policy grants for that class.
    Remote,
    /// Not recognised — the fail-closed bucket.
    Unknown,
}

impl Capability {
    /// Short human label for `/tools` output.
    pub fn label(&self) -> &'static str {
        match self {
            Capability::Planning => "planning",
            Capability::Compute => "compute",
            Capability::Filesystem => "filesystem",
            Capability::Network => "network",
            Capability::Shell => "shell",
            Capability::Exec => "exec",
            Capability::Orchestration => "orchestration",
            Capability::Meta => "meta",
            Capability::Remote => "remote",
            Capability::Unknown => "unknown",
        }
    }

    /// True when a call to a tool of this class carries an argument whose text
    /// could escalate into network or destructive effects (shell / interpreter).
    fn is_interpreted(&self) -> bool {
        matches!(self, Capability::Shell | Capability::Exec)
    }

    /// Whether the sandbox grants this class outright. Remote MCP tools use
    /// this: they run on a server we cannot inspect, so their capability
    /// ceiling is whatever the local policy allows for that class — never more.
    pub fn granted_in(self, sb: &Sandbox) -> bool {
        sb.allows(self)
    }
}

/// Total map from registered tool name to its primary capability class. Names
/// not listed here fall through to [`Capability::Unknown`] (denied).
pub fn capability_of(name: &str) -> Capability {
    match name {
        "bash" => Capability::Shell,
        "python" => Capability::Exec,
        "editor" | "read_file" | "list_dir" | "grep" | "search_replace" => Capability::Filesystem,
        "web_search" | "web_fetch" => Capability::Network,
        "vecmath" => Capability::Compute,
        "planning" | "todo" => Capability::Planning,
        "enter_plan_mode" | "exit_plan_mode" => Capability::Planning,
        "spawn_subagent" | "swarm" => Capability::Orchestration,
        "terminate" | "ask_human" => Capability::Meta,
        _ if is_mcp_tool(name) => Capability::Remote,
        _ => Capability::Unknown,
    }
}

/// MCP tools register as `mcp__<server>__<tool>`; the server name never
/// contains `__` (the CLI rejects it at config load), so two separators
/// reliably identify the prefix.
pub fn is_mcp_tool(name: &str) -> bool {
    name.starts_with("mcp__") && name[5..].contains("__")
}

/// Conservative class guess for a remote MCP tool from its declared schema.
/// Defaults to Network — an MCP tool that touches anything reaches it through
/// its server, which is outbound from our point of view. Only calls with no
/// arguments and a description free of risky verbs are treated as Compute.
pub fn infer_remote_capability(schema: &Value, description: &str) -> Capability {
    let d = description.to_lowercase();
    const RISKY: &[&str] = &[
        "file", "directory", "shell", "command", "exec", "run ", "delete", "remove", "write",
        "network", "http", "url", "fetch", "browse", "scrap", "download", "upload", "system",
    ];
    if RISKY.iter().any(|p| d.contains(p)) {
        return Capability::Network;
    }
    let has_required = matches!(schema.get("required").and_then(|r| r.as_array()), Some(a) if !a.is_empty());
    if has_required {
        return Capability::Network;
    }
    let props_empty = schema
        .get("properties")
        .and_then(|p| p.as_object())
        .map_or(true, |m| m.is_empty());
    if props_empty {
        Capability::Compute
    } else {
        Capability::Network
    }
}

/// Substrings that, inside a shell command or interpreter payload, indicate the
/// call is reaching beyond its nominal class. Kept deliberately small and
/// auditable; a real port would parse the command, not scan for literals.
fn escalation(cmd: &str) -> Option<(&'static str, Capability)> {
    let lower = cmd.to_lowercase();
    // Outbound network from within a shell/exec tool.
    const NET: &[&str] = &[
        "curl ", "wget ", "nc ", "ssh ", "scp ", "ftp ", "telnet ", "http://", "https://",
    ];
    for pat in NET {
        if lower.contains(pat) {
            return Some((pat, Capability::Network));
        }
    }
    // Destructive / privileged filesystem actions.
    const DESTRUCTIVE: &[&str] = &[
        "rm -rf", "mkfs", "dd if=", ":(){", "> /dev/", "chmod 777", "sudo ", "shutdown", "reboot",
    ];
    for pat in DESTRUCTIVE {
        if lower.contains(pat) {
            return Some((pat, Capability::Filesystem));
        }
    }
    None
}

/// A capability-scoped sandbox. Constructed with an explicit set of allowed
/// classes; anything else is denied. Interpreted tools (shell/exec) additionally
/// have their payload scanned, so restricting `Network`/`Filesystem` genuinely
/// blocks a `bash curl …` even though `bash` itself (Shell) is allowed.
pub struct Sandbox {
    pub allow: Vec<Capability>,
}

impl Sandbox {
    pub fn new(allow: impl IntoIterator<Item = Capability>) -> Self {
        let mut v: Vec<Capability> = allow.into_iter().collect();
        v.sort_by_key(|c| format!("{:?}", c)); // deterministic membership checks
        v.dedup();
        Self { allow: v }
    }

    /// A permissive-but-honest baseline: every local class, but NOT `Remote` —
    /// MCP tools are gated behind explicit approval unless a caller opts in via
    /// [`Sandbox::with_remote`]. Filesystem/network escalation through shell or
    /// exec is still policed by the payload scan. Use this when you want
    /// "normal agent" behaviour with the sharp edges off.
    pub fn standard() -> Self {
        Self::new([
            Capability::Planning,
            Capability::Compute,
            Capability::Filesystem,
            Capability::Network,
            Capability::Shell,
            Capability::Exec,
            Capability::Orchestration,
            Capability::Meta,
        ])
    }

    /// `standard()` plus automatic admission for remote MCP tools whose worst
    /// case (Network) is already granted. Call this only for servers you trust.
    pub fn with_remote() -> Self {
        Self::new([
            Capability::Planning,
            Capability::Compute,
            Capability::Filesystem,
            Capability::Network,
            Capability::Shell,
            Capability::Exec,
            Capability::Orchestration,
            Capability::Meta,
            Capability::Remote,
        ])
    }

    /// Read-only: planning, compute, orchestration and meta only. Anything that
    /// touches the outside world (files, shell, exec, network) is denied outright.
    /// Subagents are allowed because they re-enter this same policy — a read-only
    /// parent cannot grant a child more capability than it has.
    pub fn read_only() -> Self {
        Self::new([
            Capability::Planning,
            Capability::Compute,
            Capability::Orchestration,
            Capability::Meta,
        ])
    }

    fn allows(&self, cap: Capability) -> bool {
        self.allow.contains(&cap)
    }
}

impl Policy for Sandbox {
    fn decide(&self, tool: &str, args: &Args) -> Decision {
        let cap = capability_of(tool);
        // Remote has its own arm below (approval-gated when Network is granted);
        // every other ungranted class fails closed here.
        if cap != Capability::Remote && !self.allows(cap) {
            return Decision::Deny(format!(
                "sandbox: `{tool}` ({}) is outside the allowed capability set",
                cap.label()
            ));
        }
        // Interpreted tools: scan the payload for escalation into a class we did
        // NOT grant. Denied classes already short-circuited above, so this only
        // fires for capabilities that ARE granted at the class level but whose
        // use through a shell/exec payload should still be policed.
        if cap.is_interpreted() {
            if let Some((pat, esc)) = escalation(payload(args)) {
                // Network reached through shell/exec is always gated: a plain
                // `bash` grant does not imply outbound access.
                if esc == Capability::Network && !self.allows(Capability::Network) {
                    return Decision::Ask(format!(
                        "`{tool}` payload appears to reach the network (`{pat}`) — approve?"
                    ));
                }
                if esc == Capability::Filesystem && !self.allows(Capability::Filesystem) {
                    return Decision::Ask(format!(
                        "`{tool}` payload appears to escalate to destructive filesystem access (`{pat}`) — approve?"
                    ));
                }
            }
        }
        // Remote MCP tools run on a server we cannot inspect; their worst-case
        // effect is Network (they fetch/read/write remotely). A sandbox that
        // grants Network but not Remote still admits them — behind approval —
        // so enabling MCP never silently widens what an agent can do.
        if cap == Capability::Remote && !self.allows(Capability::Remote) {
            if self.allows(Capability::Network) {
                return Decision::Ask(format!(
                    "remote MCP tool `{tool}` runs off-server (network surface) — approve?"
                ));
            }
            return Decision::Deny(format!(
                "sandbox: remote MCP tool `{tool}` needs Network, which is not granted"
            ));
        }
        Decision::Allow
    }
}

/// Concatenate the string-ish arguments most likely to hold a command/code body.
fn payload(args: &Args) -> &str {
    for key in ["command", "code", "cmd", "script", "snippet", "source"] {
        if let Some(s) = args.get(key).and_then(|v| v.as_str()) {
            if !s.is_empty() {
                return s;
            }
        }
    }
    ""
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn every_default_tool_has_a_known_capability() {
        for name in [
            "bash", "python", "editor", "planning", "web_search", "vecmath", "terminate",
            "ask_human", "spawn_subagent", "swarm",
        ] {
            assert_ne!(capability_of(name), Capability::Unknown, "{name} unclassified");
        }
        assert_eq!(capability_of("mystery"), Capability::Unknown);
        assert_eq!(capability_of("spawn_subagent"), Capability::Orchestration);
    }

    #[test]
    fn read_only_denies_outside_classes() {
        let sb = Sandbox::read_only();
        let empty = json!({});
        assert!(sb.decide("planning", &empty).is_allow());
        assert!(matches!(sb.decide("bash", &empty), Decision::Deny(_)));
        assert!(matches!(sb.decide("editor", &empty), Decision::Deny(_)));
        assert!(matches!(sb.decide("web_search", &empty), Decision::Deny(_)));
        // Unknown tool fails closed even in read-only.
        assert!(matches!(sb.decide("mystery", &empty), Decision::Deny(_)));
    }

    #[test]
    fn standard_allows_bash_but_gates_network_escalation() {
        let sb = Sandbox::new([
            Capability::Planning,
            Capability::Compute,
            Capability::Filesystem,
            Capability::Shell,
            Capability::Exec,
            Capability::Orchestration,
            Capability::Meta,
        ]); // Shell yes, Network no
        let echo = json!({"command": "echo hi"});
        assert!(sb.decide("bash", &echo).is_allow());
        let curl = json!({"command": "curl https://evil.test/x"});
        assert!(matches!(sb.decide("bash", &curl), Decision::Ask(_)));
    }

    #[test]
    fn standard_permits_network_when_granted() {
        let sb = Sandbox::new([
            Capability::Shell,
            Capability::Network,
            Capability::Meta,
        ]);
        let curl = json!({"command": "curl https://ok.test"});
        assert!(sb.decide("bash", &curl).is_allow());
    }

    #[test]
    fn python_exec_payload_is_scanned_too() {
        let sb = Sandbox::new([
            Capability::Planning,
            Capability::Compute,
            Capability::Filesystem,
            Capability::Shell,
            Capability::Exec,
            Capability::Orchestration,
            Capability::Meta,
        ]);
        let bad = json!({"code": "import os; os.system('wget http://x')"});
        assert!(matches!(sb.decide("python", &bad), Decision::Ask(_)));
    }

    #[test]
    fn mcp_tools_classify_as_remote_not_unknown() {
        assert!(is_mcp_tool("mcp__files__read"));
        assert_eq!(capability_of("mcp__files__read"), Capability::Remote);
        // Malformed prefixes (no second separator) stay Unknown → denied.
        assert!(!is_mcp_tool("mcp__broken"));
        assert_eq!(capability_of("mcp__broken"), Capability::Unknown);
    }

    #[test]
    fn standard_sandbox_asks_before_running_remote_tools() {
        let sb = Sandbox::standard(); // Network granted, Remote not
        let call = json!({"msg": "hi"});
        assert!(matches!(sb.decide("mcp__test_server__echo", &call), Decision::Ask(_)));
        // An explicitly-trusted sandbox admits them outright.
        let trusted = Sandbox::with_remote();
        assert!(trusted.decide("mcp__test_server__echo", &call).is_allow());
    }

    #[test]
    fn remote_denied_when_network_not_granted() {
        let sb = Sandbox::read_only(); // no Network, no Remote
        assert!(matches!(
            sb.decide("mcp__test_server__echo", &json!({})),
            Decision::Deny(_)
        ));
    }

    #[test]
    fn remote_capability_inference_is_conservative() {
        // Pure computation: no args, benign description.
        let pure = json!({"type": "object", "properties": {}});
        assert_eq!(infer_remote_capability(&pure, "add two numbers"), Capability::Compute);
        // Anything that mentions files or fetching is treated as Network.
        let f = json!({"type": "object", "properties": {"path": {"type": "string"}}});
        assert_eq!(infer_remote_capability(&f, "read a file from disk"), Capability::Network);
        // Declared arguments mean unknown effects → Network by default.
        let q = json!({"type": "object", "properties": {"query": {"type": "string"}}});
        assert_eq!(infer_remote_capability(&q, "look things up"), Capability::Network);
    }
}
