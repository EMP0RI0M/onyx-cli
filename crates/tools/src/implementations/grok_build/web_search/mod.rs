//! `web_search` tool — structured web search with domain filtering and snippet extraction.
use crate::{Args, Output, Tool, ToolError};
use std::process::Command;

#[derive(Default)]
pub struct WebSearchTool;

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

fn strip_html_tags(html: &str) -> String {
    let mut text = String::with_capacity(html.len());
    let mut in_tag = false;
    let mut in_skip = false;
    let chars: Vec<char> = html.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '<' {
            in_tag = true;
            let rest: String = chars[i..].iter().take(8).collect::<String>().to_lowercase();
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
    let mut collapsed = String::with_capacity(text.len());
    let mut last_space = false;
    for c in text.chars() {
        if c.is_whitespace() {
            if !last_space {
                collapsed.push(' ');
                last_space = true;
            }
        } else {
            collapsed.push(c);
            last_space = false;
        }
    }
    collapsed.trim().to_string()
}

impl Tool for WebSearchTool {
    fn name(&self) -> &str {
        "web_search"
    }

    fn description(&self) -> &str {
        "Search the web for up-to-date documentation, technical references, code snippets, and answers. Args: query (string), allowed_domains (optional array of strings)."
    }

    fn arg_keys(&self) -> Vec<String> {
        vec!["query".into(), "allowed_domains".into()]
    }

    fn execute(&self, args: &Args) -> Result<Output, ToolError> {
        let raw_query = args.get("query").and_then(|v| v.as_str()).ok_or_else(|| {
            ToolError::BadArgs("missing 'query'".into())
        })?;

        let mut query = raw_query.to_string();
        if let Some(domains) = args.get("allowed_domains").and_then(|v| v.as_array()) {
            for d in domains {
                if let Some(domain_str) = d.as_str() {
                    query.push_str(&format!(" site:{domain_str}"));
                }
            }
        }

        let encoded = url_encode(&query);
        let url = format!("https://html.duckduckgo.com/html/?q={encoded}");

        let child = Command::new("curl")
            .arg("-sSL")
            .arg("-m")
            .arg("10")
            .arg("-A")
            .arg("Mozilla/5.0 (X11; Linux x86_64; rv:109.0) Gecko/20100101 Firefox/119.0")
            .arg(&url)
            .output();

        match child {
            Ok(res) if res.status.success() => {
                let html = String::from_utf8_lossy(&res.stdout);
                let cleaned = strip_html_tags(&html);

                let mut out = format!("Search results for: \"{query}\"\n\n");
                let mut lines: Vec<&str> = cleaned.lines().map(str::trim).filter(|s| !s.is_empty()).collect();
                
                // Keep the most informative chunk
                let snippet_limit = 20;
                let take_count = lines.len().min(snippet_limit);
                for line in lines.drain(..take_count) {
                    if line.len() > 10 {
                        out.push_str(&format!("- {line}\n"));
                    }
                }

                if out.lines().count() <= 2 {
                    out.push_str("(No detailed snippets returned; check query terms or network access)");
                }

                Ok(Output::ok(out))
            }
            Ok(res) => {
                let err = String::from_utf8_lossy(&res.stderr);
                Err(ToolError::Other(format!("Search request failed with status: {:?} ({err})", res.status.code())))
            }
            Err(e) => Err(ToolError::Io(e)),
        }
    }
}
