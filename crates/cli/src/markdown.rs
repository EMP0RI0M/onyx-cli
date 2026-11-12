//! Minimal, zero-dependency Markdown & syntax highlighter for the Onyx CLI.
//! Formats headings, bullet lists, blockquotes, code blocks with syntax highlighting,
//! and inline emphasis (*italic*, **bold**, `code`).

#[allow(unused_imports)]
use crate::tui::char_width;

const RESET: &str = "\x1b[0m";
const BOLD: &str = "\x1b[1m";
#[allow(dead_code)]
const DIM: &str = "\x1b[2m";
const ITALIC: &str = "\x1b[3m";
const RED: &str = "\x1b[31m";
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
    let mut table_rows: Vec<Vec<String>> = Vec::new();

    let flush_table = |rows: &mut Vec<Vec<String>>, out: &mut Vec<String>| {
        if rows.is_empty() {
            return;
        }
        let rendered = render_table_block(rows, width);
        out.extend(rendered);
        rows.clear();
    };

    for raw_line in text.lines() {
        let trimmed = raw_line.trim_end();
        let stripped = trimmed.trim_start();

        // Check if line is part of a markdown table (| col1 | col2 |)
        if !in_code_block && stripped.starts_with('|') && stripped.ends_with('|') && stripped.len() > 1 {
            let cells: Vec<String> = stripped
                .trim_matches('|')
                .split('|')
                .map(|c| c.trim().to_string())
                .collect();
            table_rows.push(cells);
            continue;
        } else if !table_rows.is_empty() {
            flush_table(&mut table_rows, &mut out);
        }

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

    if !table_rows.is_empty() {
        flush_table(&mut table_rows, &mut out);
    }

    // Auto-close open code block if any
    if in_code_block {
        let rem = width.saturating_sub(6).max(2);
        out.push(format!("  {FG_240}└{}{RESET}", "─".repeat(rem)));
    }

    out
}

/// Format and align Markdown tables gracefully into terminal boxes
fn render_table_block(rows: &[Vec<String>], width: usize) -> Vec<String> {
    if rows.is_empty() {
        return Vec::new();
    }
    let num_cols = rows.iter().map(|r| r.len()).max().unwrap_or(0);
    if num_cols == 0 {
        return Vec::new();
    }

    // Normalize column counts
    let norm: Vec<Vec<String>> = rows
        .iter()
        .map(|r| {
            let mut row = r.clone();
            while row.len() < num_cols {
                row.push(String::new());
            }
            row
        })
        .collect();

    // Identify and filter out separator rows (|---|---|)
    let mut data_rows = Vec::new();
    let mut is_header = false;
    for (i, r) in norm.iter().enumerate() {
        let sep = r.iter().all(|c| c.chars().all(|ch| ch == '-' || ch == ':' || ch == ' ') && !c.is_empty());
        if sep {
            if i == 1 {
                is_header = true;
            }
            continue;
        }
        data_rows.push(r.clone());
    }

    if data_rows.is_empty() {
        return Vec::new();
    }

    // Calculate natural column widths
    let mut col_widths = vec![4usize; num_cols];
    for r in &data_rows {
        for (c, cell) in r.iter().enumerate() {
            col_widths[c] = col_widths[c].max(cell.chars().count());
        }
    }

    // Fit within terminal budget
    let border_padding = num_cols * 3 + 4;
    let avail = width.saturating_sub(border_padding).max(num_cols * 8);
    let total_w: usize = col_widths.iter().sum();
    if total_w > avail {
        for w in &mut col_widths {
            let share = ((*w as f64 / total_w as f64) * avail as f64).round() as usize;
            *w = share.max(6);
        }
    }

    let mut out = Vec::new();

    // Top border
    let top = col_widths
        .iter()
        .map(|w| "─".repeat(*w + 2))
        .collect::<Vec<_>>()
        .join("┬");
    out.push(format!("  {FG_240}┌{}┐{RESET}", top));

    for (row_idx, row) in data_rows.iter().enumerate() {
        // Wrap cell contents
        let mut wrapped_cells: Vec<Vec<String>> = Vec::new();
        let mut max_cell_lines = 1;
        for (c, cell) in row.iter().enumerate() {
            let max_w = col_widths[c];
            let cell_fmt = format_inline_markdown(cell);
            let mut lines = Vec::new();
            let mut cur = String::new();
            let mut cur_len = 0;
            for word in cell_fmt.split_whitespace() {
                let vis_len = strip_ansi(word).chars().count();
                if cur_len + vis_len + 1 > max_w && !cur.is_empty() {
                    lines.push(cur);
                    cur = word.to_string();
                    cur_len = vis_len;
                } else {
                    if !cur.is_empty() {
                        cur.push(' ');
                        cur_len += 1;
                    }
                    cur.push_str(word);
                    cur_len += vis_len;
                }
            }
            if !cur.is_empty() {
                lines.push(cur);
            }
            if lines.is_empty() {
                lines.push(String::new());
            }
            max_cell_lines = max_cell_lines.max(lines.len());
            wrapped_cells.push(lines);
        }

        for sub_line in 0..max_cell_lines {
            let mut line_parts = Vec::new();
            for c in 0..num_cols {
                let cell_line = wrapped_cells[c].get(sub_line).cloned().unwrap_or_default();
                let vis_len = strip_ansi(&cell_line).chars().count();
                let pad = col_widths[c].saturating_sub(vis_len);
                if row_idx == 0 && is_header {
                    line_parts.push(format!(" {BOLD}{CYAN}{cell_line}{RESET}{} ", " ".repeat(pad)));
                } else {
                    line_parts.push(format!(" {FG_250}{cell_line}{RESET}{} ", " ".repeat(pad)));
                }
            }
            out.push(format!("  {FG_240}│{RESET}{}{FG_240}│{RESET}", line_parts.join(&format!("{FG_240}│{RESET}"))));
        }

        // Header separator
        if row_idx == 0 && is_header && data_rows.len() > 1 {
            let mid = col_widths
                .iter()
                .map(|w| "─".repeat(*w + 2))
                .collect::<Vec<_>>()
                .join("┼");
            out.push(format!("  {FG_240}├{}┤{RESET}", mid));
        }
    }

    // Bottom border
    let bot = col_widths
        .iter()
        .map(|w| "─".repeat(*w + 2))
        .collect::<Vec<_>>()
        .join("┴");
    out.push(format!("  {FG_240}└{}┘{RESET}", bot));

    out
}

fn strip_ansi(s: &str) -> String {
    let mut out = String::new();
    let mut in_esc = false;
    for c in s.chars() {
        if c == '\x1b' {
            in_esc = true;
        } else if in_esc {
            if c == 'm' {
                in_esc = false;
            }
        } else {
            out.push(c);
        }
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

/// Light syntax highlighter for code lines (Rust, Python, JS/TS, Bash, Diff/Patch)
fn highlight_code_line(line: &str, lang: &str) -> String {
    let lower_lang = lang.to_ascii_lowercase();
    if lower_lang == "diff" || lower_lang == "patch" {
        if line.starts_with("+++") || line.starts_with("---") {
            return format!("{BOLD}{FG_252}{line}{RESET}");
        } else if line.starts_with('+') {
            return format!("{GREEN}{line}{RESET}");
        } else if line.starts_with('-') {
            return format!("{RED}{line}{RESET}");
        } else if line.starts_with("@@") {
            return format!("{CYAN}{line}{RESET}");
        } else if line.starts_with("diff --git") || line.starts_with("index ") {
            return format!("{BOLD}{MAGENTA}{line}{RESET}");
        } else {
            return format!("{FG_250}{line}{RESET}");
        }
    }

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

    #[test]
    fn test_diff_highlighting() {
        let diff = "```diff\n--- a/file.rs\n+++ b/file.rs\n@@ -1,2 +1,2 @@\n-old line\n+new line\n```";
        let rendered = render_markdown(diff, 80);
        assert!(rendered.len() >= 6);
        assert!(rendered.iter().any(|l| l.contains("\x1b[31m-old line")));
        assert!(rendered.iter().any(|l| l.contains("\x1b[32m+new line")));
    }
}
