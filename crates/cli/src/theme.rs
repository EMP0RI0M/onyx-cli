//! onyx theme — a single token layer for the ratatui renderer.
//!
//! Every color and `Style` the dashboard draws resolves through here so a
//! restyle is one edit. The palette is the ONYX "Premium Dark Workstation"
//! identity: primary cyan `#00E5FF` on near-black, with semantic accents for
//! success / error / warning / info. Some tokens (`CardState`, `info`, the
//! semantic `C_*` consts) are the shared accent vocabulary the transcript
//! builders reach for; they stay exported even where a given frame doesn't
//! touch every one.
#![allow(dead_code)]

use ratatui::style::{Color, Modifier, Style};

/// Base-16-ish palette tokens as raw RGB so both fg colors and gradients can
/// read the channels. These are the source of truth; the `Color` consts below
/// are derived from them.
pub const PRIMARY: (u8, u8, u8) = (0, 229, 255); // #00E5FF  cyan accent
pub const INFO: (u8, u8, u8) = (56, 189, 248); //  #38BDF8  sky blue
pub const FG_MAIN: (u8, u8, u8) = (241, 245, 249); // #F1F5F9 body text
pub const FG_SEC: (u8, u8, u8) = (137, 147, 159); // #89939F muted text
pub const BORDER: (u8, u8, u8) = (37, 43, 53); //   #252B35 borders
pub const SUCCESS: (u8, u8, u8) = (32, 217, 154); // #20D99A ok
pub const ERROR: (u8, u8, u8) = (255, 100, 124); // #FF647C fail
pub const WARNING: (u8, u8, u8) = (255, 202, 104); // #FFCA68 pending/attention
pub const PANEL_BG: (u8, u8, u8) = (9, 12, 18); //   near-black surface
pub const SELECTION_BG: (u8, u8, u8) = (30, 38, 50); // highlighted row bg

pub const fn rgb(t: (u8, u8, u8)) -> Color {
    Color::Rgb(t.0, t.1, t.2)
}

// Derived `Color` consts (convenience for widget `.fg(...)`/.bg(...)).
pub const C_PRIMARY: Color = Color::Rgb(PRIMARY.0, PRIMARY.1, PRIMARY.2);
pub const C_INFO: Color = Color::Rgb(INFO.0, INFO.1, INFO.2);
pub const C_FG_MAIN: Color = Color::Rgb(FG_MAIN.0, FG_MAIN.1, FG_MAIN.2);
pub const C_FG_SEC: Color = Color::Rgb(FG_SEC.0, FG_SEC.1, FG_SEC.2);
pub const C_BORDER: Color = Color::Rgb(BORDER.0, BORDER.1, BORDER.2);
pub const C_SUCCESS: Color = Color::Rgb(SUCCESS.0, SUCCESS.1, SUCCESS.2);
pub const C_ERROR: Color = Color::Rgb(ERROR.0, ERROR.1, ERROR.2);
pub const C_WARNING: Color = Color::Rgb(WARNING.0, WARNING.1, WARNING.2);
pub const C_PANEL_BG: Color = Color::Rgb(PANEL_BG.0, PANEL_BG.1, PANEL_BG.2);
pub const C_SELECTION_BG: Color = Color::Rgb(SELECTION_BG.0, SELECTION_BG.1, SELECTION_BG.2);

/// Linear interpolation between two palette tuples by `t` in `0.0..=1.0`.
/// Used by the animated gradient header to shimmer cyan→blue across the
/// wordmark without a per-frame allocation.
pub fn lerp(a: (u8, u8, u8), b: (u8, u8, u8), t: f64) -> Color {
    let t = t.clamp(0.0, 1.0);
    let mix = |x: u8, y: u8| (x as f64 + (y as f64 - x as f64) * t) as u8;
    Color::Rgb(mix(a.0, b.0), mix(a.1, b.1), mix(a.2, b.2))
}

// ------------------------------------------------------------- style helpers ----

/// Default cell style: main text on the panel background.
pub fn base() -> Style {
    Style::default().fg(C_FG_MAIN).bg(C_PANEL_BG)
}

/// Bold primary-accent text (brand marks, prompts, selected command names).
pub fn accent_bold() -> Style {
    Style::default().fg(C_PRIMARY).add_modifier(Modifier::BOLD)
}

/// Muted secondary text (dim labels, hints, timestamps).
pub fn dim() -> Style {
    Style::default().fg(C_FG_SEC)
}

/// Body text emphasis (user turns, card titles).
pub fn strong() -> Style {
    Style::default().fg(C_FG_MAIN).add_modifier(Modifier::BOLD)
}

/// Informational line (ℹ notices, model name echoes).
pub fn info() -> Style {
    Style::default().fg(C_INFO)
}

/// Border-only style for rounded boxes.
pub fn border() -> Style {
    Style::default().fg(C_BORDER)
}

/// Border tinted toward the accent for an active/focused composer.
pub fn border_focus() -> Style {
    Style::default().fg(C_PRIMARY)
}

/// Tool-card / status chrome style for one of the three execution states.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CardState {
    Running,
    Ok,
    Denied,
}

impl CardState {
    /// Accent color for the card border + status glyph.
    pub fn color(self) -> Color {
        match self {
            CardState::Running => C_WARNING,
            CardState::Ok => C_SUCCESS,
            CardState::Denied => C_ERROR,
        }
    }

    /// Status label text shown at the top of a tool card.
    pub fn label(self) -> &'static str {
        match self {
            CardState::Running => "Executing your request…",
            CardState::Ok => "Executed successfully",
            CardState::Denied => "Denied by policy",
        }
    }

    /// Leading status glyph for the tool-name row.
    pub fn glyph(self) -> &'static str {
        match self {
            CardState::Running => "⠋",
            CardState::Ok => "✓",
            CardState::Denied => "✗",
        }
    }
}

/// Selected row highlight for the slash dropdown list.
pub fn selection() -> Style {
    Style::default()
        .fg(C_FG_MAIN)
        .bg(C_SELECTION_BG)
        .add_modifier(Modifier::BOLD)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rgb_tokens_match_onyx_palette() {
        assert_eq!(rgb(PRIMARY), Color::Rgb(0, 229, 255));
        assert_eq!(rgb(BORDER), Color::Rgb(37, 43, 53));
        assert_eq!(rgb(SUCCESS), Color::Rgb(32, 217, 154));
    }

    #[test]
    fn lerp_endpoints_are_exact() {
        assert_eq!(lerp(PRIMARY, INFO, 0.0), Color::Rgb(0, 229, 255));
        assert_eq!(lerp(PRIMARY, INFO, 1.0), Color::Rgb(56, 189, 248));
    }

    #[test]
    fn lerp_clamps_out_of_range() {
        assert_eq!(lerp(PRIMARY, INFO, -1.0), Color::Rgb(0, 229, 255));
        assert_eq!(lerp(PRIMARY, INFO, 2.0), Color::Rgb(56, 189, 248));
    }

    #[test]
    fn card_states_have_distinct_accents() {
        assert_eq!(CardState::Ok.color(), C_SUCCESS);
        assert_eq!(CardState::Denied.color(), C_ERROR);
        assert_eq!(CardState::Running.color(), C_WARNING);
        assert_ne!(CardState::Ok.color(), CardState::Denied.color());
    }
}
