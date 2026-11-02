//! WebFetch tool — fetch web content and documentation URLs.
use crate::{Args, Output, Tool, ToolError};

pub struct WebFetch;

impl Tool for WebFetch {
    fn name(&self) -> &str {
        "web_fetch"
    }

    fn description(&self) -> &str {
        "Fetch raw text or HTML content from a given web URL. Args: url (string), max_bytes (optional int)."
    }

    fn arg_keys(&self) -> Vec<String> {
        vec!["url".into(), "max_bytes".into()]
    }

    fn execute(&self, args: &Args) -> Result<Output, ToolError> {
        let url = args.get("url").and_then(|v| v.as_str()).ok_or_else(|| {
            ToolError::BadArgs("missing 'url'".into())
        })?;

        if !url.starts_with("http://") && !url.starts_with("https://") {
            return Err(ToolError::BadArgs("url must start with http:// or https://".into()));
        }

        let max_bytes = args.get("max_bytes").and_then(|v| v.as_u64()).unwrap_or(50_000) as usize;

        // Use curl under the hood
        let out = std::process::Command::new("curl")
            .arg("-sSL")
            .arg("-m")
            .arg("15")
            .arg(url)
            .output()
            .map_err(ToolError::Io)?;

        if !out.status.success() {
            return Err(ToolError::Other(format!(
                "curl failed with status: {:?}",
                out.status.code()
            )));
        }

        let body = String::from_utf8_lossy(&out.stdout);
        let trimmed: String = body.chars().take(max_bytes).collect();

        Ok(Output::ok(trimmed))
    }
}
