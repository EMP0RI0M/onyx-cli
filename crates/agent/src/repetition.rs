//! Generation reliability and streamed repetition detection.
//!
//! Small reasoning models can get trapped in repetitive n-gram loops when
//! enumerating lists or generating tokens (e.g. `Also "ws_bridge.py" etc.`).
//! This module provides a fast, bounded sliding-window repetition detector
//! that detects stalled generation and allows safe recovery.

#[derive(Debug, Clone)]
pub struct RepetitionDetector {
    /// Minimum repeating phrase length in characters (e.g. 15 chars).
    min_ngram_len: usize,
    /// Maximum phrase length to inspect (e.g. 120 chars).
    max_ngram_len: usize,
    /// Number of consecutive repetitions required to trigger a stall detection.
    consecutive_threshold: usize,
    /// Buffer of recent characters emitted in the stream.
    recent_buffer: String,
    /// Maximum characters kept in the recent buffer.
    max_buffer_size: usize,
}

impl Default for RepetitionDetector {
    fn default() -> Self {
        Self::new(7, 800, 2)
    }
}

impl RepetitionDetector {
    pub fn new(min_ngram_len: usize, max_ngram_len: usize, consecutive_threshold: usize) -> Self {
        Self {
            min_ngram_len,
            max_ngram_len,
            consecutive_threshold,
            recent_buffer: String::with_capacity(4096),
            max_buffer_size: 4096,
        }
    }

    /// Reset the detector state between turns or steps.
    pub fn reset(&mut self) {
        self.recent_buffer.clear();
    }

    /// Feed a newly received delta fragment. Returns `Some(repeated_phrase)` if a stall loop is detected.
    pub fn push_delta(&mut self, delta: &str) -> Option<String> {
        self.recent_buffer.push_str(delta);
        if self.recent_buffer.len() > self.max_buffer_size {
            let keep_start = self.recent_buffer.len() - (self.max_buffer_size / 2);
            self.recent_buffer = self.recent_buffer[keep_start..].to_string();
        }

        self.detect_repetition()
    }

    /// Inspect recent buffer for repetitive contiguous n-grams across entire buffer.
    pub fn detect_repetition(&self) -> Option<String> {
        let buf = &self.recent_buffer;
        let buf_len = buf.len();

        let search_lookback = if buf_len > 1024 { 1024 } else { buf_len };
        let search_start = buf_len.saturating_sub(search_lookback);

        let buf_bytes = buf.as_bytes();

        for end_idx in (search_start..=buf_len).rev() {
            if !buf.is_char_boundary(end_idx) {
                continue;
            }

            for len in (self.min_ngram_len..=self.max_ngram_len).rev() {
                let span = len * self.consecutive_threshold;
                if end_idx < span {
                    continue;
                }

                let start_cand = end_idx - len;
                if !buf.is_char_boundary(start_cand) {
                    continue;
                }

                // Fast byte-level pre-filter on last and first character of repetition blocks
                if buf_bytes[end_idx - 1] != buf_bytes[start_cand - 1] {
                    continue;
                }
                if buf_bytes[start_cand] != buf_bytes[start_cand - len] {
                    continue;
                }

                let candidate = &buf[start_cand..end_idx];

                // Check if candidate consists purely of repetitive whitespace, single punctuation, or common syntax delimiters
                if candidate.chars().all(|c| c.is_whitespace() || c == '-' || c == '*' || c == '=' || c == '#' || c == '/' || c == '}' || c == ']' || c == ',' || c == ';') {
                    continue;
                }

                // Exclude single/double character repetition (e.g. repeated dashes, braces or spaces)
                let unique_chars: std::collections::HashSet<char> = candidate.chars().collect();
                if unique_chars.len() <= 3 {
                    continue;
                }

                let mut match_count = 1usize;
                let mut boundaries_valid = true;
                for i in 1..self.consecutive_threshold {
                    let start = end_idx - (i + 1) * len;
                    let end = end_idx - i * len;
                    if !buf.is_char_boundary(start) || !buf.is_char_boundary(end) {
                        boundaries_valid = false;
                        break;
                    }
                    if &buf[start..end] == candidate {
                        match_count += 1;
                    } else {
                        break;
                    }
                }

                if boundaries_valid && match_count >= self.consecutive_threshold {
                    return Some(candidate.to_string());
                }
            }
        }

        None
    }
}

/// Filter for model streaming that isolates `<think>...</think>` tags
/// so internal reasoning never leaks into user-facing content deltas.
#[derive(Debug, Clone, Default)]
pub struct ThinkTagFilter {
    buf: String,
    in_think: bool,
}

impl ThinkTagFilter {
    pub fn new() -> Self {
        Self { buf: String::new(), in_think: false }
    }

    pub fn push(&mut self, delta: &str, mut on_content: impl FnMut(&str), mut on_reasoning: impl FnMut(&str)) {
        self.buf.push_str(delta);
        loop {
            if !self.in_think {
                if let Some(idx) = self.buf.find("<think>") {
                    let before = self.buf[..idx].to_string();
                    if !before.is_empty() {
                        on_content(&before);
                    }
                    self.buf = self.buf[idx + 7..].to_string();
                    self.in_think = true;
                } else if self.buf.ends_with('<')
                    || self.buf.ends_with("<t")
                    || self.buf.ends_with("<th")
                    || self.buf.ends_with("<thi")
                    || self.buf.ends_with("<thin")
                    || self.buf.ends_with("<think")
                {
                    let open_pos = self.buf.rfind('<').unwrap();
                    let before = self.buf[..open_pos].to_string();
                    if !before.is_empty() {
                        on_content(&before);
                    }
                    self.buf = self.buf[open_pos..].to_string();
                    break;
                } else {
                    let out = std::mem::take(&mut self.buf);
                    if !out.is_empty() {
                        on_content(&out);
                    }
                    break;
                }
            } else {
                if let Some(idx) = self.buf.find("</think>") {
                    let before = self.buf[..idx].to_string();
                    if !before.is_empty() {
                        on_reasoning(&before);
                    }
                    self.buf = self.buf[idx + 8..].to_string();
                    self.in_think = false;
                } else if self.buf.ends_with('<')
                    || self.buf.ends_with("</")
                    || self.buf.ends_with("</t")
                    || self.buf.ends_with("</th")
                    || self.buf.ends_with("</thi")
                    || self.buf.ends_with("</thin")
                    || self.buf.ends_with("</think")
                {
                    let open_pos = self.buf.rfind('<').unwrap();
                    let before = self.buf[..open_pos].to_string();
                    if !before.is_empty() {
                        on_reasoning(&before);
                    }
                    self.buf = self.buf[open_pos..].to_string();
                    break;
                } else {
                    let out = std::mem::take(&mut self.buf);
                    if !out.is_empty() {
                        on_reasoning(&out);
                    }
                    break;
                }
            }
        }
    }

    pub fn finish(&mut self, mut on_content: impl FnMut(&str), mut on_reasoning: impl FnMut(&str)) {
        if !self.buf.is_empty() {
            let rem = std::mem::take(&mut self.buf);
            if self.in_think {
                on_reasoning(&rem);
            } else {
                on_content(&rem);
            }
        }
    }
}

/// Helper function to strip any `<think>...</think>` tags from a complete string,
/// returning (clean_content, extracted_reasoning).
pub fn strip_think_tags(text: &str) -> (String, Option<String>) {
    let mut clean_content = String::new();
    let mut extracted_reasoning = String::new();
    let mut rem = text;

    while let Some(start) = rem.find("<think>") {
        clean_content.push_str(&rem[..start]);
        let after_start = &rem[start + 7..];
        if let Some(end) = after_start.find("</think>") {
            extracted_reasoning.push_str(&after_start[..end]);
            rem = &after_start[end + 8..];
        } else {
            extracted_reasoning.push_str(after_start);
            rem = "";
            break;
        }
    }
    clean_content.push_str(rem);

    let clean = clean_content.trim().to_string();
    let reasoning = if extracted_reasoning.trim().is_empty() {
        None
    } else {
        Some(extracted_reasoning.trim().to_string())
    };
    (clean, reasoning)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detects_phrase_repetition() {
        let mut detector = RepetitionDetector::new(15, 50, 3);
        let phrase = "Also \"ws_bridge.py\" etc. ";
        for _ in 0..2 {
            assert!(detector.push_delta(phrase).is_none());
        }
        let stalled = detector.push_delta(phrase);
        assert!(stalled.is_some());
        assert_eq!(stalled.unwrap(), phrase);
    }

    #[test]
    fn test_ignores_legitimate_code_syntax() {
        let mut detector = RepetitionDetector::default();
        let code = r#"
            let a = 1;
            let b = 2;
            let c = 3;
            let d = 4;
            let e = 5;
        "#;
        assert!(detector.push_delta(code).is_none());
    }

    #[test]
    fn test_ignores_whitespace_indentation() {
        let mut detector = RepetitionDetector::new(4, 20, 5);
        let indents = "                "; // 16 spaces
        assert!(detector.push_delta(indents).is_none());
    }

    #[test]
    fn test_ignores_long_json_and_syntax_delimiters() {
        let mut detector = RepetitionDetector::default();
        let json_array = r#"[
            {"id": 1, "status": "active"},
            {"id": 2, "status": "active"},
            {"id": 3, "status": "active"},
            {"id": 4, "status": "active"},
            {"id": 5, "status": "active"}
        ]"#;
        assert!(detector.push_delta(json_array).is_none());
    }

    #[test]
    fn test_multilingual_repetition_detection() {
        let mut detector = RepetitionDetector::new(10, 60, 3);
        let jp_phrase = "ディレクトリの項目を確認中... ";
        for _ in 0..2 {
            assert!(detector.push_delta(jp_phrase).is_none());
        }
        let stalled = detector.push_delta(jp_phrase);
        assert!(stalled.is_some());
    }
}
