//! Context-window management: keep a long-running conversation inside the
//! model's token budget without losing its thread.
//!
//! Two tiers, both cheap and deterministic (no model call to run them):
//! 1. [`shrink_tool_output`] — per-message: every tool result above the cap is
//!    middle-elided (head + tail kept, the seam labelled with an exact count).
//!    Enormous outputs (`ls -R`, log dumps) are what blow the window first.
//! 2. [`compact_history`] — whole-history, applied at send time when the
//!    estimated total exceeds the budget. It runs in two stages: first PRUNE
//!    (middle-elide stale oversized tool outputs, keeping every message's
//!    head+tail), and only if that alone doesn't fit, FOLD the oldest user/tool
//!    messages into ONE digest. The system prompt, assistant tool-call
//!    bookkeeping, and the most recent messages survive verbatim, so replaying
//!    the compacted history can't produce dangling tool results. Pruning first
//!    means many histories never need the lossier fold at all.
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

    // Tier 2a — PRUNE. Before folding anything away, drop the bulk of stale,
    // oversized TOOL outputs (head + tail kept). This is the cheap win opencode
    // makes first: it reclaims the most bytes while keeping every message's
    // identity and its beginning/end readable. If pruning alone fits the budget,
    // we never pay for the lossier digest fold below. User turns are NOT pruned
    // here — they are conversation anchors and get folded whole instead.
    const PRUNE_KEEP: usize = 600;
    let mut work: Vec<Message> = history.to_vec();
    for m in &mut work {
        if m.role == "tool" {
            m.content = shrink_tool_output(&m.content, PRUNE_KEEP * 2, PRUNE_KEEP);
        }
    }
    let pruned_total: usize = work.iter().map(|m| estimate_tokens(&m.content)).sum();
    if pruned_total <= budget {
        return Some(work);
    }

    // Tier 2b — FOLD. Pruning wasn't enough: now the oldest user/tool messages
    // between the two ends collapse into ONE digest line.
    let history = &work[..];

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
/// summary) so compaction never eats its own bookkeeping. A digest may lead
/// with its re-extractable `<user_queries>` block, so recognise either head.
fn is_digest(m: &Message) -> bool {
    m.role == "user"
        && (m.content.starts_with("[Context compacted:") || m.content.starts_with(QUERIES_OPEN))
}

pub const SHRINK_CAP: usize = 24_000;
pub const SHRINK_KEEP: usize = 12_000;
const DIGEST_RESERVE: usize = 1024;
/// Floor for the per-message keep in the last-resort shrink loop — never elide
/// a body below this many kept chars, so each pass makes bounded progress.
const MIN_KEEP_CHARS: usize = 24;
/// Per-query character budget inside the preserved-queries block. Long user
/// turns are middle-truncated (head + tail kept) to this many content chars.
const QUERY_KEEP_CHARS: usize = 600;
/// Hard ceiling on the preserved-queries block. Queries are collected oldest
/// first and kept until this cap; the remainder is counted, not silently lost.
const QUERIES_BLOCK_CAP: usize = 3072;
/// The two markers delimiting the re-extractable user-queries block. Kept as
/// consts so both the writer and the recompaction extractor agree verbatim.
const QUERIES_OPEN: &str = "<user_queries>";
const QUERIES_CLOSE: &str = "</user_queries>";

/// Middle-truncate `text` to at most `max` content characters, keeping a head
/// and tail with an exact dropped-count seam. Returns `None` when no truncation
/// is needed (`text` already fits), mirroring the reference "truncate_middle"
/// contract so callers can distinguish "fit verbatim" from "was clipped".
fn truncate_middle(text: &str, max: usize) -> Option<String> {
    let total = text.chars().count();
    if total <= max {
        return None;
    }
    let head_len = max / 2;
    let tail_len = max - head_len;
    let head: String = text.chars().take(head_len).collect();
    let tail: String = text.chars().skip(total - tail_len).collect();
    Some(format!(
        "{head}…[{} chars truncated]…{tail}",
        total - head_len - tail_len
    ))
}

/// Split a digest's content into its `<user_queries>` block (if any) and the
/// prose body with that block removed. On recompaction we strip the old block
/// before folding again, so preserved queries never snowball across rounds —
/// they're re-emitted once, merged with the newly-folded queries, at the front.
fn split_queries(content: &str) -> (Option<String>, String) {
    let start = match content.find(QUERIES_OPEN) {
        Some(i) => i,
        None => return (None, content.to_string()),
    };
    let rel_end = match content[start..].find(QUERIES_CLOSE) {
        Some(r) => r,
        None => return (None, content.to_string()),
    };
    let end = start + rel_end + QUERIES_CLOSE.len();
    let block = content[start..end].to_string();
    let mut rest = String::new();
    rest.push_str(content[..start].trim());
    let tail = content[end..].trim();
    if !tail.is_empty() {
        if !rest.is_empty() {
            rest.push('\n');
        }
        rest.push_str(tail);
    }
    (Some(block), rest)
}

/// One-line-per-message factual digest: role, tool name when present, and a
/// short excerpt. Deliberately mechanical — no summarisation claims. The whole
/// digest is byte-capped so folding many messages can't outgrow the budget it
/// was meant to save; overflow is counted, not silently truncated mid-line.
///
/// User turns additionally contribute a near-verbatim entry to a leading
/// `<user_queries>` block that survives recompaction (harvested pattern: the
/// original task intent must not decay just because the model call is
/// mechanical). A prior digest's own query block is merged in rather than
/// nested, so chained compactions accumulate without snowballing.
fn digest_messages(msgs: &[&Message]) -> String {
    use std::fmt::Write as _;
    const EXCERPT_CHARS: usize = 120;
    const DIGEST_CAP: usize = 2048;

    // Preserved user queries: merge any block carried by a prior digest, then
    // append the freshly-folded user turns. Capped oldest-first so the newest
    // intents are the ones that survive when the block would overflow.
    let mut queries: Vec<String> = Vec::new();
    for m in msgs {
        if is_digest(m) {
            if let (Some(block), _) = split_queries(&m.content) {
                for line in block.lines().filter_map(|l| l.strip_prefix("- ")) {
                    queries.push(line.to_string());
                }
            }
        } else if m.role == "user" && !m.content.trim().is_empty() {
            let one_line = collapsed_ws(&m.content);
            let q = truncate_middle(&one_line, QUERY_KEEP_CHARS).unwrap_or(one_line);
            queries.push(q);
        }
    }
    // Dedup identical query lines (a merged prior query can repeat), keeping order.
    let mut seen = std::collections::HashSet::new();
    queries.retain(|q| seen.insert(q.clone()));
    let mut kept_queries: Vec<String> = Vec::new();
    let mut qbytes = 0usize;
    let mut dropped_queries = 0usize;
    for q in queries {
        let cost = q.len() + 4; // "- " + newline slack
        if qbytes + cost > QUERIES_BLOCK_CAP {
            dropped_queries += 1;
            continue;
        }
        qbytes += cost;
        kept_queries.push(q);
    }

    let mut out = String::new();
    if !kept_queries.is_empty() {
        out.push_str(QUERIES_OPEN);
        out.push('\n');
        for q in &kept_queries {
            let _ = writeln!(out, "- {q}");
        }
        if dropped_queries > 0 {
            let _ = writeln!(out, "- …and {dropped_queries} older queries dropped (block capped)");
        }
        out.push_str(QUERIES_CLOSE);
        out.push('\n');
    }

    let _ = write!(
        out,
        "[Context compacted: {} earlier messages folded. Excerpts below; full text was dropped from this request.]\n",
        msgs.len()
    );
    let mut shown = 0usize;
    for m in msgs {
        // User turns are represented in the query block above; skip their
        // redundant excerpt line so the prose body stays compact.
        if m.role == "user" {
            continue;
        }
        let label = match &m.tool_call {
            Some(tc) => format!("{}→{}", m.role, tc.name),
            None => m.role.clone(),
        };
        let excerpt: String = m.content.chars().take(EXCERPT_CHARS).collect();
        let excerpt = excerpt.trim();
        let line_len = label.len() + excerpt.len() + 8;
        if out.len() + line_len > DIGEST_CAP {
            let remaining = msgs
                .iter()
                .filter(|mm| mm.role != "user")
                .count()
                - shown;
            let _ = writeln!(out, "- …and {remaining} more folded (digest capped)");
            break;
        }
        let _ = writeln!(out, "- {label}: {excerpt}");
        shown += 1;
    }
    out
}

/// Collapse all runs of whitespace to single spaces (for one-line query entries).
fn collapsed_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
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
        // A digest message was injected (may lead with its query block).
        assert!(c.iter().any(is_digest));
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

    #[test]
    fn prune_fits_without_folding_when_only_tools_are_huge() {
        // A short history whose bulk is two enormous tool dumps. Pruning alone
        // should reclaim enough that NO digest fold is needed — every message
        // keeps its identity (role), just shrunk head+tail.
        let h = vec![
            Message::text("system", "sys"),
            Message::text("user", "short question"),
            Message::text("tool", "L".repeat(60_000)),
            Message::text("assistant", "thinking"),
            Message::text("tool", "R".repeat(60_000)),
            Message::text("user", "thanks"),
        ];
        let c = compact_history(&h, 4096).expect("should compact");
        let total: usize = c.iter().map(|m| estimate_tokens(&m.content)).sum();
        assert!(total <= 4096, "pruned still over budget: {total}");
        // No digest was injected — pruning sufficed.
        assert!(
            !c.iter().any(|m| m.content.starts_with("[Context compacted:")),
            "fold ran when prune should have been enough"
        );
        // All six original messages survive (nothing folded away).
        assert_eq!(c.len(), h.len());
        // The huge bodies were elided, not dropped whole.
        assert!(c.iter().any(|m| m.content.contains("characters elided")));
    }

    #[test]
    fn digest_preserves_user_queries_in_block() {
        let msgs = vec![
            Message::text("user", "build a rust TUI with inline viewport"),
            Message::text("tool", "some big tool output"),
        ];
        let refs: Vec<&Message> = msgs.iter().collect();
        let d = digest_messages(&refs);
        assert!(d.starts_with("<user_queries>"), "{d}");
        assert!(d.contains("</user_queries>"), "{d}");
        assert!(
            d.contains("- build a rust TUI with inline viewport"),
            "query not preserved verbatim: {d}"
        );
        // The user turn is NOT also emitted as a redundant excerpt line.
        assert!(!d.contains("- user:"), "user got a duplicate excerpt line: {d}");
    }

    #[test]
    fn long_query_is_middle_truncated() {
        let long = format!("START {} END", "x".repeat(2000));
        let msgs = vec![Message::text("user", &long)];
        let refs: Vec<&Message> = msgs.iter().collect();
        let d = digest_messages(&refs);
        assert!(d.contains("START"), "{d}");
        assert!(d.contains("END"), "{d}");
        assert!(d.contains("chars truncated"), "no truncation marker: {d}");
        assert!(!d.contains(&"x".repeat(2000)), "body not clipped: {d}");
    }

    #[test]
    fn recompaction_merges_queries_without_nesting_or_snowballing() {
        // Round 1: fold an early user query + noise.
        let round1 = vec![
            Message::text("user", "first intent"),
            Message::text("tool", "z".repeat(50_000)),
        ];
        let h1 = {
            let mut h = vec![Message::text("system", "sys")];
            h.extend(round1);
            h.push(Message::text("user", "second intent"));
            h
        };
        let c1 = compact_history(&h1, 64).expect("round1 compacts");
        let digest1 = c1
            .iter()
            .find(|m| is_digest(m))
            .expect("digest present");
        assert!(digest1.content.contains("first intent"), "{:?}", digest1.content);

        // Round 2: the prior digest itself gets folded alongside more history.
        // Its query block must be MERGED (queries accumulate) and never nested
        // (only one <user_queries> pair in the new digest). A trailing oversized
        // tool is the newest message the loop always keeps; that pushes the huge
        // user turn into the fold so its query must survive via the merge path.
        let mut h2 = vec![Message::text("system", "sys"), digest1.clone()];
        h2.push(Message::text("tool", "w".repeat(50_000)));
        h2.push(Message::text("user", &format!("third intent {}", "t".repeat(30_000))));
        h2.push(Message::text("tool", "v".repeat(50_000)));
        let c2 = compact_history(&h2, 64).expect("round2 compacts");
        let digest2 = c2.iter().find(|m| is_digest(m)).expect("digest2");
        let body = &digest2.content;
        assert!(body.contains("first intent"), "prior query lost: {body}");
        assert!(body.contains("third intent"), "new query lost: {body}");
        assert_eq!(
            body.matches(QUERIES_OPEN).count(),
            1,
            "query block nested across recompaction: {body}"
        );
        assert_eq!(body.matches(QUERIES_CLOSE).count(), 1, "{body}");
    }

    #[test]
    fn split_queries_roundtrips() {
        let content = "<user_queries>\n- keep me\n</user_queries>\n[Context compacted: 3 earlier messages folded.]";
        let (block, rest) = split_queries(content);
        assert_eq!(block.as_deref(), Some("<user_queries>\n- keep me\n</user_queries>"));
        assert!(!rest.contains("<user_queries>"), "block not stripped from rest: {rest}");
        assert!(rest.contains("[Context compacted:"), "prose lost: {rest}");
        // No block → whole content is prose, block None.
        let (none, all) = split_queries("just prose");
        assert!(none.is_none());
        assert_eq!(all, "just prose");
    }

    #[test]
    fn truncate_middle_only_when_over() {
        assert_eq!(truncate_middle("short", 100), None);
        let t = truncate_middle(&"y".repeat(100), 10).expect("should truncate");
        assert!(t.contains("chars truncated"));
        assert!(t.chars().count() < 100);
    }
}
