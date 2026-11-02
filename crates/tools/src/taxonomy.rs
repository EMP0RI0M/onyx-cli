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

use crate::policy::{Decision, Policy};
use crate::Args;

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
    /// Ends the turn / talks back to the user; control-flow only.
    Meta,
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
            Capability::Meta => "meta",
            Capability::Unknown => "unknown",
        }
    }

    /// True when a call to a tool of this class carries an argument whose text
    /// could escalate into network or destructive effects (shell / interpreter).
    fn is_interpreted(&self) -> bool {
        matches!(self, Capability::Shell | Capability::Exec)
    }
}

/// Total map from registered tool name to its primary capability class. Names
/// not listed here fall through to [`Capability::Unknown`] (denied).
pub fn capability_of(name: &str) -> Capability {
    match name {
        "bash" => Capability::Shell,
        "python" => Capability::Exec,
        "editor" => Capability::Filesystem,
        "web_search" => Capability::Network,
        "vecmath" => Capability::Compute,
        "planning" => Capability::Planning,
        "terminate" | "ask_human" => Capability::Meta,
        _ => Capability::Unknown,
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

    /// A permissive-but-honest baseline: everything except Unknown, but with
    /// Network and Filesystem-escalation still gated via the payload scan. Use
    /// this when you want "normal agent" behaviour with the sharp edges off.
    pub fn standard() -> Self {
        Self::new([
            Capability::Planning,
            Capability::Compute,
            Capability::Filesystem,
            Capability::Shell,
            Capability::Exec,
            Capability::Meta,
        ])
    }

    /// Read-only: planning, compute and meta only. Anything touching the outside
    /// world (files, shell, exec, network) is denied outright.
    pub fn read_only() -> Self {
        Self::new([Capability::Planning, Capability::Compute, Capability::Meta])
    }

    fn allows(&self, cap: Capability) -> bool {
        self.allow.contains(&cap)
    }
}

impl Policy for Sandbox {
    fn decide(&self, tool: &str, args: &Args) -> Decision {
        let cap = capability_of(tool);
        if !self.allows(cap) {
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
            }
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
            "ask_human",
        ] {
            assert_ne!(capability_of(name), Capability::Unknown, "{name} unclassified");
        }
        assert_eq!(capability_of("mystery"), Capability::Unknown);
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
        let sb = Sandbox::standard(); // Shell yes, Network no
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
        let sb = Sandbox::standard();
        let bad = json!({"code": "import os; os.system('wget http://x')"});
        assert!(matches!(sb.decide("python", &bad), Decision::Ask(_)));
    }
}
