//! Context-window management: keep a long-running conversation inside the
//! model's token budget without losing its thread.
//!
//! Two tiers, both cheap and deterministic (no model call to run them):
//! 1. [`shrink_tool_output`] — per-message: every tool result above the cap is
//!    middle-elided (head + tail kept, the seam labelled with an exact count).
//!    Enormous outputs (`ls -R`, log dumps) are what blow the window first.
//! 2. [`compact_history`] — whole-history: when the estimated total exceeds the
//!    budget, the oldest `user`/`tool` messages between the two ends are folded
//!    into ONE digest message; the system prompt, the first exchange, and the
//!    most recent messages survive verbatim. Tool-call bookkeeping messages are
//!    never dropped, so replaying the compacted history can't produce dangling
//!    tool results.
//!
//! The estimator ([`estimate_tokens`]) is deliberately conservative: ~4 chars
//! per token for ASCII-heavy text, halved for non-ASCII payloads which tend to
//! tokenize denser per char. Overestimating wastes some window; underestimating
//! gets an HTTP 400. We choose the safe side.

use crate::Message;

/// Approximate tokens in one message (role overhead included).
pub fn estimate_tokens(text: &str) -> usize {
    let chars = text.chars().count();
    let ascii = text.bytes().filter(|b| b.is_ascii()).count();
    // Non-ASCII chars average ~1 token each; ASCII ~4 chars/token.
    let non_ascii = chars - ascii;
    (ascii / 4 + non_ascii).max(1) + 4 // role/separator overhead
}

/// Middle-elide a tool output longer than `cap` chars: keep the first and last
/// `keep` chars, replace the seam with an exact-count marker. Idempotent —
/// an already-shrunk message passes through untouched (the marker itself would
/// otherwise push it back over the cap).
pub fn shrink_tool_output(content: &str, cap: usize, keep: usize) -> String {
    let total = content.chars().count();
    if total <= cap {
        return content.to_string();
    }
    // If already contains an elision marker, strip the old marker before re-shrinking
    // so repeated shrinkage is both idempotent and capable of tightening further.
    let base_content = if let (Some(start), Some(end)) = (
        content.find("\n\n[... "),
        content.find(" characters elided to fit the context window ...]\n\n"),
    ) {
        let after = end + " characters elided to fit the context window ...]\n\n".len();
        format!("{}{}", &content[..start], &content[after..])
    } else {
        content.to_string()
    };

    let total = base_content.chars().count();
    if total <= cap {
        return content.to_string();
    }
    let keep = keep.min(total / 2);
    let head: String = base_content.chars().take(keep).collect();
    let tail: String = base_content.chars().skip(total - keep).collect();
    format!(
        "{head}\n\n[... {} characters elided to fit the context window ...]\n\n{tail}",
        total - 2 * keep
    )
}

/// A compacted view of `history`: returns `None` when the estimate already fits
/// `budget`. Otherwise folds the oldest droppable messages into a single digest
/// `user` message placed right after the leading system message.
///
/// Never drops: the system message, the first user message, any assistant
/// message (they anchor tool-call bookkeeping), and the newest messages needed
/// to stay under budget. If folding everything droppable still doesn't fit,
/// the largest remaining tool outputs are shrunk before giving up.
pub fn compact_history(history: &[Message], budget: usize) -> Option<Vec<Message>> {
    let total: usize = history.iter().map(|m| estimate_tokens(&m.content)).sum();
    if total <= budget {
        return None;
    }

    // Partition: the system message and any assistant / tool-call bookkeeping
    // message survive verbatim (dropping them would dangle a tool result).
    // Everything else — user turns and tool outputs, including an oversized
    // first user turn — is foldable and can be digested or shrunk.
    let mut out: Vec<Message> = Vec::with_capacity(history.len());
    let mut fold: Vec<&Message> = Vec::new();
    for m in history {
        if m.role == "assistant" || m.tool_call.is_some() {
            out.push(m.clone());
        } else if m.role == "system" {
            out.push(m.clone());
        } else {
            fold.push(m);
        }
    }

    // Rebuild from the end backwards until we fit; leftover fold entries get
    // digested into one summary message inserted after the pinned prefix.
    // The newest message is always kept even if it alone busts the budget
    // (shrinking handles that below); everything older can be folded.
    let pinned_tokens: usize = out.iter().map(|m| estimate_tokens(&m.content)).sum();
    let mut kept_tail: Vec<&Message> = Vec::new();
    let mut used = pinned_tokens;
    for m in fold.iter().rev() {
        let t = estimate_tokens(&m.content);
        // Always keep at least the newest message; break BEFORE pushing so
        // kept_tail never exceeds fold (else `dropped` underflows to empty).
        if !kept_tail.is_empty() && used + t > budget.saturating_sub(DIGEST_RESERVE) {
            break;
        }
        used += t;
        kept_tail.push(m);
    }
    kept_tail.reverse();

    let dropped: Vec<&Message> = fold
        .iter()
        .take(fold.len().saturating_sub(kept_tail.len()))
        .copied()
        .collect();

    // Folded-away messages become ONE digest line, placed before the kept tail.
    let mut rebuilt = out;
    if !dropped.is_empty() {
        let digest = digest_messages(&dropped);
        // Insert the digest right after the leading system message (position 1),
        // before the kept tail. System is always history[0] in this loop's use.
        let insert_at = rebuilt
            .iter()
            .position(|m| m.role != "system")
            .unwrap_or(rebuilt.len());
        rebuilt.insert(insert_at, Message::text("user", digest));
    }
    for m in kept_tail {
        rebuilt.push(m.clone());
    }

    // Last resort: middle-elide surviving oversized bodies until the estimate
    // fits. Each pass recomputes an even character-share of the *current* total
    // against the budget, so the cap tightens every iteration — this converges
    // where a fixed floor would plateau. Idempotent + strictly shrinking. Digest
    // and system/assistant bookkeeping are exempt; only tool/user bodies shrink.
    let mut guard = 0;
    loop {
        guard += 1;
        let total: usize = rebuilt.iter().map(|m| estimate_tokens(&m.content)).sum();
        if total <= budget || guard > 12 {
            break;
        }
        let eligible: Vec<usize> = rebuilt
            .iter()
            .enumerate()
            .filter(|(_, m)| (m.role == "tool" || m.role == "user") && !is_digest(m))
            .map(|(i, _)| i)
            .collect();
        if eligible.is_empty() {
            break; // nothing shrinkable left; pinned prefix alone busts the budget
        }
        // Give the eligible pool its fair share of the budget, in characters.
        let eligible_tok: usize =
            eligible.iter().map(|&i| estimate_tokens(&rebuilt[i].content)).sum();
        let excess = total.saturating_sub(budget);
        // Deduct excess with a small margin so token rounding doesn't leave total slightly above budget
        let new_pool_tok = eligible_tok.saturating_sub(excess + 32).max(eligible.len());
        let target_chars = new_pool_tok.saturating_mul(4) / eligible.len();
        let target_chars = target_chars.max(MIN_KEEP_CHARS * 2);
        let keep = (target_chars / 2).max(MIN_KEEP_CHARS);

        let mut progress = false;
        for &i in &eligible {
            let before = rebuilt[i].content.chars().count();
            rebuilt[i].content = shrink_tool_output(&rebuilt[i].content, target_chars, keep);
            if rebuilt[i].content.chars().count() < before {
                progress = true;
            }
        }
        if !progress {
            break; // all at/below their caps; further passes can't help
        }
    }
    Some(rebuilt)
}

/// The synthetic digest line we inject — exempt from shrinking (it's already a
/// summary) so compaction never eats its own bookkeeping.
fn is_digest(m: &Message) -> bool {
    m.role == "user" && m.content.starts_with("[Context compacted:")
}

pub const SHRINK_CAP: usize = 24_000;
pub const SHRINK_KEEP: usize = 12_000;
const DIGEST_RESERVE: usize = 1024;
/// Floor for the per-message keep in the last-resort shrink loop — never elide
/// a body below this many kept chars, so each pass makes bounded progress.
const MIN_KEEP_CHARS: usize = 24;

/// One-line-per-message factual digest: role, tool name when present, and a
/// short excerpt. Deliberately mechanical — no summarisation claims. The whole
/// digest is byte-capped so folding many messages can't outgrow the budget it
/// was meant to save; overflow is counted, not silently truncated mid-line.
fn digest_messages(msgs: &[&Message]) -> String {
    use std::fmt::Write as _;
    const EXCERPT_CHARS: usize = 120;
    const DIGEST_CAP: usize = 2048;
    let mut s = format!(
        "[Context compacted: {} earlier messages folded. Excerpts below; full text was dropped from this request.]\n",
        msgs.len()
    );
    let mut shown = 0usize;
    for m in msgs {
        let label = match &m.tool_call {
            Some(tc) => format!("{}→{}", m.role, tc.name),
            None => m.role.clone(),
        };
        let excerpt: String = m.content.chars().take(EXCERPT_CHARS).collect();
        let excerpt = excerpt.trim();
        let line_len = label.len() + excerpt.len() + 8;
        if s.len() + line_len > DIGEST_CAP {
            let _ = writeln!(s, "- …and {} more folded (digest capped)", msgs.len() - shown);
            break;
        }
        let _ = writeln!(s, "- {label}: {excerpt}");
        shown += 1;
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ToolCall;
    use serde_json::json;

    fn hist(sizes: &[usize]) -> Vec<Message> {
        let mut h = vec![Message::text("system", "you are an agent")];
        for (i, n) in sizes.iter().enumerate() {
            let role = if i % 2 == 0 { "user" } else { "tool" };
            h.push(Message::text(role, "x".repeat(*n)));
        }
        h
    }

    #[test]
    fn small_conversations_are_not_compacted() {
        assert!(compact_history(&hist(&[10, 10, 10]), 4096).is_none());
    }

    #[test]
    fn large_history_folds_into_under_budget() {
        let h = hist(&[40_000; 8]); // way over a 4k-token budget
        let c = compact_history(&h, 4096).expect("should compact");
        let total: usize = c.iter().map(|m| estimate_tokens(&m.content)).sum();
        assert!(total <= 4096, "compacted still over budget: {total}");
        // System message survives verbatim at position 0.
        assert_eq!(c[0].role, "system");
        // A digest message was injected.
        assert!(c.iter().any(|m| m.content.starts_with("[Context compacted:")));
    }

    #[test]
    fn assistant_and_system_messages_survive() {
        let h = vec![
            Message::text("system", "sys"),
            Message::text("user", "u1"),
            Message {
                role: "assistant".into(),
                content: "calling".into(),
                tool_call: Some(ToolCall { name: "bash".into(), args: json!({"command": "ls"}) }),
            },
            Message::text("tool", "y".repeat(50_000)),
            Message::text("user", "u2"),
        ];
        let mut c = compact_history(&h, 100).expect("compact");
        assert_eq!(c[0].content, "sys");
        assert!(c.iter().any(|m| m.tool_call.is_some()), "tool-call bookkeeping dropped");
        // The huge tool message either survived shrunk or was folded into the digest.
        for m in &mut c {
            if m.role == "tool" {
                m.content = shrink_tool_output(&m.content, SHRINK_CAP, SHRINK_KEEP);
            }
        }
        let total: usize = c.iter().map(|m| estimate_tokens(&m.content)).sum();
        assert!(total <= 200, "should be near budget, got {total}");
    }

    #[test]
    fn shrink_is_idempotent_and_exact() {
        let big = "a".repeat(30_000);
        let s = shrink_tool_output(&big, 24_000, 12_000);
        assert!(s.contains("6000 characters elided"));
        let again = shrink_tool_output(&s, 24_000, 12_000);
        assert_eq!(s, again, "shrinking twice must be stable");
        assert_eq!(shrink_tool_output("short", 24_000, 12_000), "short");
    }

    #[test]
    fn digest_labels_tool_calls() {
        let m = Message {
            role: "tool".into(),
            content: "result text".into(),
            tool_call: Some(ToolCall { name: "vecmath".into(), args: json!({}) }),
        };
        let d = digest_messages(&[&m]);
        assert!(d.contains("tool→vecmath"), "{d}");
    }
}
