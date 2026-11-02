//! Minimal, zero-dependency Markdown & syntax highlighter for the Onyx CLI.
//! Formats headings, bullet lists, blockquotes, code blocks with syntax highlighting,
//! and inline emphasis (*italic*, **bold**, `code`).

use crate::tui::char_width;

const RESET: &str = "\x1b[0m";
const BOLD: &str = "\x1b[1m";
const DIM: &str = "\x1b[2m";
const ITALIC: &str = "\x1b[3m";
const CYAN: &str = "\x1b[36m";
const GREEN: &str = "\x1b[32m";
const YELLOW: &str = "\x1b[33m";
const MAGENTA: &str = "\x1b[35m";
const BLUE: &str = "\x1b[34m";
const FG_240: &str = "\x1b[38;5;240m";
const FG_242: &str = "\x1b[38;5;242m";
const FG_245: &str = "\x1b[38;5;245m";
const FG_250: &str = "\x1b[38;5;250m";
const FG_252: &str = "\x1b[38;5;252m";
const CODE_BG: &str = "\x1b[48;5;236m";

pub fn render_markdown(text: &str, width: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut in_code_block = false;
    let mut code_lang = String::new();

    for raw_line in text.lines() {
        let trimmed = raw_line.trim_end();
        let stripped = trimmed.trim_start();

        // Fenced code blocks
        if stripped.starts_with("```") {
            if !in_code_block {
                in_code_block = true;
                code_lang = stripped[3..].trim().to_string();
                let label = if code_lang.is_empty() { "code" } else { &code_lang };
                let rem = width.saturating_sub(label.chars().count() + 10).max(2);
                out.push(format!("  {FG_240}┌─{RESET} {BOLD}{CYAN}{label}{RESET} {FG_240}{}{RESET}", "─".repeat(rem)));
            } else {
                in_code_block = false;
                let rem = width.saturating_sub(6).max(2);
                out.push(format!("  {FG_240}└{}{RESET}", "─".repeat(rem)));
            }
            continue;
        }

        if in_code_block {
            let highlighted = highlight_code_line(raw_line, &code_lang);
            out.push(format!("  {FG_240}│{RESET} {highlighted}"));
            continue;
        }

        // Headings
        if stripped.starts_with("### ") {
            let content = stripped[4..].trim();
            out.push(format!("  {BOLD}{CYAN}{content}{RESET}"));
            continue;
        } else if stripped.starts_with("## ") {
            let content = stripped[3..].trim();
            out.push(format!("  {BOLD}{BLUE}{content}{RESET}"));
            continue;
        } else if stripped.starts_with("# ") {
            let content = stripped[2..].trim();
            out.push(format!("  {BOLD}{MAGENTA}{content}{RESET}"));
            let rule_len = content.chars().count().min(width.saturating_sub(4));
            out.push(format!("  {FG_240}{}{RESET}", "━".repeat(rule_len)));
            continue;
        }

        // Blockquotes
        if stripped.starts_with('>') {
            let content = stripped[1..].trim();
            let formatted = format_inline_markdown(content);
            out.push(format!("  {FG_242}▎{RESET} {ITALIC}{FG_250}{formatted}{RESET}"));
            continue;
        }

        // Bullet lists
        if stripped.starts_with("* ") || stripped.starts_with("- ") || stripped.starts_with("• ") {
            let content = stripped[2..].trim();
            let formatted = format_inline_markdown(content);
            out.push(format!("  {CYAN}•{RESET} {FG_252}{formatted}{RESET}"));
            continue;
        }

        // Numbered lists (e.g. "1. ")
        if let Some(pos) = stripped.find(". ") {
            if pos > 0 && stripped[..pos].chars().all(|c| c.is_ascii_digit()) {
                let num = &stripped[..pos];
                let content = stripped[pos + 2..].trim();
                let formatted = format_inline_markdown(content);
                out.push(format!("  {CYAN}{num}.{RESET} {FG_252}{formatted}{RESET}"));
                continue;
            }
        }

        // Empty line
        if stripped.is_empty() {
            out.push(String::new());
            continue;
        }

        // Standard paragraph
        let formatted = format_inline_markdown(trimmed);
        out.push(format!("  {FG_250}{formatted}{RESET}"));
    }

    // Auto-close open code block if any
    if in_code_block {
        let rem = width.saturating_sub(6).max(2);
        out.push(format!("  {FG_240}└{}{RESET}", "─".repeat(rem)));
    }

    out
}

/// Highlight inline markdown (`code`, **bold**, *italic*)
pub fn format_inline_markdown(text: &str) -> String {
    let mut out = String::with_capacity(text.len() * 2);
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    let n = chars.len();

    while i < n {
        // Inline code `...`
        if chars[i] == '`' {
            if let Some(end) = chars[i + 1..].iter().position(|&c| c == '`') {
                let code_content: String = chars[i + 1..i + 1 + end].iter().collect();
                out.push_str(&format!("{YELLOW}{CODE_BG} {code_content} {RESET}"));
                i = i + 1 + end + 1;
                continue;
            }
        }

        // Bold **...**
        if i + 1 < n && chars[i] == '*' && chars[i + 1] == '*' {
            if let Some(end) = chars[i + 2..].windows(2).position(|w| w == ['*', '*']) {
                let bold_content: String = chars[i + 2..i + 2 + end].iter().collect();
                out.push_str(&format!("{BOLD}{bold_content}{RESET}"));
                i = i + 2 + end + 2;
                continue;
            }
        }

        // Italic *...*
        if chars[i] == '*' {
            if let Some(end) = chars[i + 1..].iter().position(|&c| c == '*') {
                let it_content: String = chars[i + 1..i + 1 + end].iter().collect();
                out.push_str(&format!("{ITALIC}{it_content}{RESET}"));
                i = i + 1 + end + 1;
                continue;
            }
        }

        out.push(chars[i]);
        i += 1;
    }

    out
}

/// Light syntax highlighter for code lines (Rust, Python, JS/TS, Bash)
fn highlight_code_line(line: &str, _lang: &str) -> String {
    let mut out = String::with_capacity(line.len() * 2);
    let chars: Vec<char> = line.chars().collect();
    let mut i = 0;
    let n = chars.len();

    while i < n {
        let c = chars[i];

        // String literals ("..." or '...')
        if c == '"' || c == '\'' {
            let quote = c;
            out.push_str(GREEN);
            out.push(quote);
            i += 1;
            while i < n {
                let qc = chars[i];
                out.push(qc);
                if qc == '\\' && i + 1 < n {
                    i += 1;
                    out.push(chars[i]);
                } else if qc == quote {
                    i += 1;
                    break;
                }
                i += 1;
            }
            out.push_str(RESET);
            continue;
        }

        // Line comments (// or #)
        if (c == '/' && i + 1 < n && chars[i + 1] == '/') || c == '#' {
            let comment: String = chars[i..].iter().collect();
            out.push_str(&format!("{FG_245}{comment}{RESET}"));
            break;
        }

        // Word tokens (keywords, types, identifiers)
        if c.is_alphanumeric() || c == '_' {
            let start = i;
            while i < n && (chars[i].is_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            let word: String = chars[start..i].iter().collect();
            if is_keyword(&word) {
                out.push_str(&format!("{BOLD}{MAGENTA}{word}{RESET}"));
            } else if is_type(&word) {
                out.push_str(&format!("{YELLOW}{word}{RESET}"));
            } else if word.chars().all(|ch| ch.is_ascii_digit()) {
                out.push_str(&format!("{CYAN}{word}{RESET}"));
            } else {
                out.push_str(&format!("{FG_252}{word}{RESET}"));
            }
            continue;
        }

        // Operators / punctuation
        out.push(c);
        i += 1;
    }

    out
}

fn is_keyword(w: &str) -> bool {
    matches!(
        w,
        "fn" | "let" | "mut" | "pub" | "struct" | "enum" | "impl" | "use" | "mod"
            | "if" | "else" | "match" | "return" | "for" | "while" | "in" | "loop"
            | "break" | "continue" | "async" | "await" | "def" | "class" | "import"
            | "from" | "const" | "var" | "function" | "true" | "false"
    )
}

fn is_type(w: &str) -> bool {
    matches!(
        w,
        "i32" | "i64" | "u32" | "u64" | "usize" | "isize" | "f32" | "f64"
            | "bool" | "char" | "str" | "String" | "Vec" | "Option" | "Result"
            | "Some" | "None" | "Ok" | "Err" | "Self" | "Box" | "Arc" | "Mutex"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_markdown_rendering_basics() {
        let text = "# Title\n\nSome **bold** and *italic* and `code`.\n- item 1\n- item 2";
        let rendered = render_markdown(text, 80);
        assert!(!rendered.is_empty());
        assert!(rendered[0].contains("Title"));
    }

    #[test]
    fn test_code_block_highlighting() {
        let code = "```rust\nfn main() {\n    let x: i32 = 42;\n}\n```";
        let rendered = render_markdown(code, 80);
        assert!(rendered.len() >= 4);
        assert!(rendered[0].contains("rust"));
        assert!(rendered[rendered.len() - 1].contains("└"));
    }
}
