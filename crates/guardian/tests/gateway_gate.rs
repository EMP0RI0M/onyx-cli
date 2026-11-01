//! End-to-end governance: a High-verdict judge chained behind BashGuard,
//! exercised through the real Gateway (no network, no LLM).

use serde_json::json;
use tools::audit::Gateway;
use tools::policy::{BashGuard, Chained, Decision, Guardian, Policy, Risk, RiskJudge};
use tools::ToolCollection;

struct AlwaysHigh;
impl RiskJudge for AlwaysHigh {
    fn judge(&self, _tool: &str, _args: &serde_json::Value) -> Risk {
        Risk::High("test verdict: destructive".into())
    }
}

fn chain() -> Chained<BashGuard, Guardian<AlwaysHigh>> {
    Chained::new(BashGuard::default(), Guardian::new(AlwaysHigh))
}

/// A minimal fake side-effecting tool so we can observe execution vs denial.
mod echo {
    use tools::{Args, Output, Tool, ToolError};
    pub struct Echo;
    impl Tool for Echo {
        fn name(&self) -> &str {
            "echo"
        }
        fn description(&self) -> &str {
            "echo back"
        }
        fn arg_keys(&self) -> Vec<String> {
            vec!["text".into()]
        }
        fn execute(&self, args: &Args) -> Result<Output, ToolError> {
            Ok(Output::ok(args.get("text").and_then(|v| v.as_str()).unwrap_or("")))
        }
    }
}

fn collection() -> ToolCollection {
    let mut c = ToolCollection::new();
    c.add(echo::Echo);
    c
}

#[test]
fn high_risk_fails_closed_without_approver() {
    let gw = Gateway::new(collection(), chain());
    let err = gw.call("echo", &json!({"text": "hi"})).unwrap_err();
    assert!(err.to_string().contains("confirmation refused"), "{err}");
    let audit = gw.audit.snapshot();
    assert!(audit.last().unwrap().outcome.starts_with("DENY: confirmation refused"));
}

#[test]
fn high_risk_executes_when_approved() {
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let seen2 = seen.clone();
    let gw = Gateway::with_approver(collection(), chain(), move |_, _, reason| {
        // the guardian's reason reaches the approver
        seen2.lock().unwrap().push(reason.to_string());
        true
    });
    let out = gw.call("echo", &json!({"text": "hi"})).unwrap();
    assert_eq!(out.content, "hi");
    assert_eq!(seen.lock().unwrap().as_slice(), ["test verdict: destructive"]);
    let audit = gw.audit.snapshot();
    let outcomes: Vec<&str> = audit.iter().map(|e| e.outcome.as_str()).collect();
    assert!(
        outcomes.iter().any(|o| o.starts_with("ASK-OK")) && outcomes.last().unwrap().starts_with("OK"),
        "{outcomes:?}"
    );
}

#[test]
fn static_deny_short_circuits_the_guardian() {
    // The judge would only Ask; BashGuard Denies and must win, and the tool
    // must not run.
    let gw = Gateway::with_approver(collection(), chain(), |_, _, _| true);
    match gw.policy.decide("bash", &json!({"command": "rm -rf /"})) {
        Decision::Deny(_) => {}
        other => panic!("expected Deny, got {other:?}"),
    }
}
