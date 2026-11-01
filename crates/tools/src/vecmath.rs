//! Vector math tool — the bridge from the Rust tool surface into the hand-written
//! AArch64 assembly dot-product kernel. This is the "assembly powers the critical
//! math kernel, Rust orchestrates" seam from the architecture brief.

use crate::{Args, Output, Tool, ToolError};
use kernel::{cosine, dot};

pub struct VecMath;

impl Default for VecMath {
    fn default() -> Self {
        Self
    }
}

fn parse_vec(v: &serde_json::Value) -> Result<Vec<f32>, ToolError> {
    let arr = v
        .as_array()
        .ok_or_else(|| ToolError::BadArgs("expected array of numbers".into()))?;
    arr.iter()
        .map(|x| {
            x.as_f64()
                .map(|f| f as f32)
                .ok_or_else(|| ToolError::BadArgs("non-numeric element".into()))
        })
        .collect()
}

impl Tool for VecMath {
    fn name(&self) -> &str {
        "vecmath"
    }
    fn arg_keys(&self) -> Vec<String> {
        vec!["op".into(), "a".into(), "b".into()]
    }
    fn description(&self) -> &str {
        "Vector math on the AArch64 NEON kernel. Args: {\"op\": \"dot|cosine|norm\", \"a\": [numbers], \"b\": [numbers]} (b optional for norm)."
    }
    fn execute(&self, args: &Args) -> Result<Output, ToolError> {
        let op = args.get("op").and_then(|v| v.as_str()).unwrap_or("dot");
        let a = args
            .get("a")
            .ok_or_else(|| ToolError::BadArgs("missing `a`".into()))
            .and_then(parse_vec)?;
        let b = args
            .get("b")
            .ok_or_else(|| ToolError::BadArgs("missing `b`".into()))
            .and_then(parse_vec)?;
        if a.len() != b.len() {
            return Ok(Output::err(format!(
                "length mismatch: a={} b={}",
                a.len(),
                b.len()
            )));
        }
        let result: f32 = match op {
            "dot" => dot(&a, &b),
            "cosine" => cosine(&a, &b),
            other => return Ok(Output::err(format!("unknown op `{other}`"))),
        };
        Ok(Output::ok(format!("{result}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn vecmath_dot_uses_asm_kernel() {
        let out = VecMath
            .execute(&json!({"op":"dot","a":[1.0,2.0,3.0,4.0],"b":[10.0,20.0,30.0,40.0]}))
            .unwrap();
        assert!(!out.is_error);
        assert_eq!(out.content.trim(), "300");
    }

    #[test]
    fn vecmath_cosine_identical_is_one() {
        let out = VecMath
            .execute(&json!({"op":"cosine","a":[1.0,2.0,3.0],"b":[1.0,2.0,3.0]}))
            .unwrap();
        let v: f32 = out.content.trim().parse().unwrap();
        assert!((v - 1.0).abs() < 1e-4, "got {v}");
    }
}
