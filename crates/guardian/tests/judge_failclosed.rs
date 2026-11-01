//! The HTTP judge must fail closed: no gateway, no verdict — High, never Low.

use agent::model::HttpModel;
use guardian::HttpJudge;
use serde_json::json;
use tools::policy::{Risk, RiskJudge};

#[test]
fn unreachable_gateway_yields_high() {
    let mut hm = HttpModel::new("127.0.0.1".into(), 1, "auto".into(), String::new(), String::new());
    hm.path = "/v1/chat/completions".into();
    let judge = HttpJudge::new(hm);
    match judge.judge("bash", &json!({"command": "ls"})) {
        Risk::High(reason) => assert!(reason.contains("guardian unavailable"), "{reason}"),
        Risk::Low => panic!("dead gateway must not produce Low"),
    }
}
