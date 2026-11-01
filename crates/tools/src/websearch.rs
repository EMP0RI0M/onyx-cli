//! WebSearch tool — a thin, dependency-free port using `curl` against the
//! DuckDuckGo lite endpoint. Best-effort: it returns stripped text snippets,
//! not a ranked index. Governance of *whether* the network may be touched lives
//! in the Policy layer, not here.

use crate::{Args, Output, Tool, ToolError};
use std::process::Command;

fn url_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

// Crude HTML→text: drop <script>/<style> bodies and all tags, collapse whitespace.
fn strip_html(html: &str) -> String {
    let mut text = String::with_capacity(html.len());
    let mut in_tag = false;
    let mut in_skip = false; // inside <script>/<style>
    let bytes: Vec<char> = html.chars().collect();
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        if c == '<' {
            in_tag = true;
            // detect skip-block opens
            let rest: String = bytes[i..].iter().take(8).collect::<String>().to_lowercase();
            if rest.starts_with("<script") || rest.starts_with("<style") {
                in_skip = true;
            } else if rest.starts_with("</script") || rest.starts_with("</style") {
                in_skip = false;
            }
            i += 1;
            continue;
        }
        if c == '>' {
            in_tag = false;
            i += 1;
            continue;
        }
        if !in_tag && !in_skip {
            text.push(c);
        }
        i += 1;
    }
    // collapse runs of whitespace
    let mut collapsed = String::with_capacity(text.len());
    let mut last_space = false;
    for c in text.chars() {
        if c.is_whitespace() {
            if !last_space {
                collapsed.push(' ');
            }
            last_space = true;
        } else {
            collapsed.push(c);
            last_space = false;
        }
    }
    collapsed
}

pub struct WebSearch;

impl Tool for WebSearch {
    fn name(&self) -> &str {
        "web_search"
    }
    fn arg_keys(&self) -> Vec<String> {
        vec!["query".into()]
    }
    fn description(&self) -> &str {
        "Search the web via DuckDuckGo lite; returns stripped text snippets."
    }
    fn execute(&self, args: &Args) -> Result<Output, ToolError> {
        let query = args
            .get("query")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::BadArgs("missing `query`".into()))?;
        let url = format!("https://lite.duckduckgo.com/lite/?q={}", url_encode(query));

        let out = Command::new("curl")
            .args(["-sS", "--max-time", "8", "-A", "tektile-term/0.1", &url])
            .output()
            .map_err(ToolError::Io)?;

        if !out.status.success() {
            let e = String::from_utf8_lossy(&out.stderr);
            return Ok(Output::err(format!("curl failed: {e}")));
        }

        let html = String::from_utf8_lossy(&out.stdout);
        let mut text = strip_html(&html);
        // trim to a sane result budget
        if text.len() > 2500 {
            text.truncate(2500);
        }
        Ok(Output::ok(text))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_html_removes_tags() {
        assert_eq!(strip_html("<p>hello <b>world</b></p>").trim(), "hello world");
    }

    #[test]
    fn url_encode_spaces_and_symbols() {
        assert_eq!(url_encode("a b&c"), "a+b%26c");
    }
}
