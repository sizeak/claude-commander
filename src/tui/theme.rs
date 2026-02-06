//! TUI Theme configuration
//!
//! Centralized theme system for consistent styling across the UI.
//! Supports multiple color depths for terminal compatibility.

use ratatui::style::{Color, Style};

/// Terminal color capability
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ColorMode {
    /// Basic 16 ANSI colors (maximum compatibility)
    Basic,
    /// 256 color palette
    #[default]
    Indexed,
    /// True color (24-bit RGB)
    TrueColor,
}

impl ColorMode {
    /// Detect the best color mode for the current terminal
    pub fn detect() -> Self {
        // Check COLORTERM first (most reliable for true color)
        if let Ok(colorterm) = std::env::var("COLORTERM") {
            if colorterm == "truecolor" || colorterm == "24bit" {
                return Self::TrueColor;
            }
        }

        // Check TERM for 256 color support
        if let Ok(term) = std::env::var("TERM") {
            if term.contains("256color") || term.contains("kitty") || term.contains("alacritty") {
                // These terminals typically support true color even without COLORTERM
                if term.contains("kitty") || term.contains("alacritty") {
                    return Self::TrueColor;
                }
                return Self::Indexed;
            }
        }

        Self::Basic
    }
}

/// Theme configuration for the TUI
#[derive(Clone)]
pub struct Theme {
    // Pane borders
    pub border_focused: Color,
    pub border_unfocused: Color,

    // Selection
    pub selection_bg: Color,
    pub selection_fg: Option<Color>,

    // Session status indicators
    pub status_running: Color,
    pub status_paused: Color,
    pub status_stopped: Color,

    // Text
    pub text_primary: Color,
    pub text_secondary: Color,
    pub text_accent: Color,
    pub text_project: Color,

    // Diff colors
    pub diff_added: Color,
    pub diff_removed: Color,
    pub diff_hunk_header: Color,
    pub diff_file_header: Color,
    pub diff_context: Color,

    // Modal borders
    pub modal_info: Color,
    pub modal_warning: Color,
    pub modal_error: Color,

    // Status bar
    pub status_bar_bg: Color,
    pub status_bar_fg: Color,
}

impl Default for Theme {
    fn default() -> Self {
        Self::for_color_mode(ColorMode::detect())
    }
}

impl Theme {
    /// Create a theme for the specified color mode
    pub fn for_color_mode(mode: ColorMode) -> Self {
        match mode {
            ColorMode::Basic => Self::basic(),
            ColorMode::Indexed => Self::indexed(),
            ColorMode::TrueColor => Self::truecolor(),
        }
    }

    /// Basic 16-color theme (maximum compatibility)
    pub fn basic() -> Self {
        Self {
            border_focused: Color::Cyan,
            border_unfocused: Color::DarkGray,

            selection_bg: Color::Blue,
            selection_fg: Some(Color::White),

            status_running: Color::Green,
            status_paused: Color::Yellow,
            status_stopped: Color::DarkGray,

            text_primary: Color::Reset,
            text_secondary: Color::DarkGray,
            text_accent: Color::Blue,
            text_project: Color::Green,

            diff_added: Color::Green,
            diff_removed: Color::Red,
            diff_hunk_header: Color::Cyan,
            diff_file_header: Color::Yellow,
            diff_context: Color::Reset,

            modal_info: Color::Cyan,
            modal_warning: Color::Yellow,
            modal_error: Color::Red,

            status_bar_bg: Color::Blue,
            status_bar_fg: Color::White,
        }
    }

    /// 256-color theme (good balance of compatibility and aesthetics)
    pub fn indexed() -> Self {
        Self {
            border_focused: Color::Indexed(117),  // Pastel sky blue
            border_unfocused: Color::Indexed(243),

            selection_bg: Color::Indexed(60),     // Muted purple-blue
            selection_fg: Some(Color::Indexed(255)),

            status_running: Color::Indexed(156),  // Pastel mint green
            status_paused: Color::Indexed(222),   // Pastel peach
            status_stopped: Color::Indexed(248),

            text_primary: Color::Reset,
            text_secondary: Color::Indexed(250),
            text_accent: Color::Indexed(147),     // Pastel lavender
            text_project: Color::Indexed(108),    // Muted sage green

            diff_added: Color::Indexed(156),      // Pastel mint
            diff_removed: Color::Indexed(210),    // Pastel coral
            diff_hunk_header: Color::Indexed(183), // Pastel orchid
            diff_file_header: Color::Indexed(223), // Pastel cream
            diff_context: Color::Reset,

            modal_info: Color::Indexed(117),      // Pastel sky
            modal_warning: Color::Indexed(222),   // Pastel peach
            modal_error: Color::Indexed(210),     // Pastel coral

            status_bar_bg: Color::Indexed(236),
            status_bar_fg: Color::Indexed(252),
        }
    }

    /// True color theme (richest visual experience)
    pub fn truecolor() -> Self {
        Self {
            border_focused: Color::Rgb(137, 180, 250),   // Pastel sky blue
            border_unfocused: Color::Rgb(88, 91, 112),

            selection_bg: Color::Rgb(69, 71, 90),
            selection_fg: Some(Color::Rgb(245, 245, 250)),

            status_running: Color::Rgb(166, 227, 161),   // Pastel mint
            status_paused: Color::Rgb(249, 226, 175),    // Pastel peach
            status_stopped: Color::Rgb(147, 153, 178),   // Muted lavender

            text_primary: Color::Rgb(245, 245, 250),
            text_secondary: Color::Rgb(166, 173, 200),
            text_accent: Color::Rgb(180, 190, 254),      // Pastel periwinkle
            text_project: Color::Rgb(129, 178, 134),     // Muted sage green

            diff_added: Color::Rgb(166, 227, 161),       // Pastel mint
            diff_removed: Color::Rgb(243, 139, 168),     // Pastel rose
            diff_hunk_header: Color::Rgb(203, 166, 247), // Pastel mauve
            diff_file_header: Color::Rgb(249, 226, 175), // Pastel peach
            diff_context: Color::Reset,

            modal_info: Color::Rgb(137, 180, 250),       // Pastel sky
            modal_warning: Color::Rgb(249, 226, 175),    // Pastel peach
            modal_error: Color::Rgb(243, 139, 168),      // Pastel rose

            status_bar_bg: Color::Rgb(49, 50, 68),
            status_bar_fg: Color::Rgb(205, 214, 244),
        }
    }

    /// Style for focused pane borders
    pub fn border_focused(&self) -> Style {
        Style::default().fg(self.border_focused)
    }

    /// Style for unfocused pane borders
    pub fn border_unfocused(&self) -> Style {
        Style::default().fg(self.border_unfocused)
    }

    /// Style for selected items
    pub fn selection(&self) -> Style {
        let style = Style::default().bg(self.selection_bg);
        match self.selection_fg {
            Some(fg) => style.fg(fg),
            None => style,
        }
    }

    /// Style for status bar
    pub fn status_bar(&self) -> Style {
        Style::default().bg(self.status_bar_bg).fg(self.status_bar_fg)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic_theme() {
        let theme = Theme::basic();
        assert_eq!(theme.border_focused, Color::Cyan);
        assert_eq!(theme.status_running, Color::Green);
    }

    #[test]
    fn test_indexed_theme() {
        let theme = Theme::indexed();
        assert_eq!(theme.border_focused, Color::Indexed(117));
        assert_eq!(theme.selection_bg, Color::Indexed(60));
    }

    #[test]
    fn test_truecolor_theme() {
        let theme = Theme::truecolor();
        assert_eq!(theme.border_focused, Color::Rgb(137, 180, 250));
        assert_eq!(theme.status_running, Color::Rgb(166, 227, 161));
    }

    #[test]
    fn test_theme_styles() {
        let theme = Theme::basic();
        let style = theme.border_focused();
        assert_eq!(style.fg, Some(Color::Cyan));
    }

    #[test]
    fn test_selection_style() {
        let theme = Theme::indexed();
        let style = theme.selection();
        assert_eq!(style.bg, Some(Color::Indexed(60)));
        assert_eq!(style.fg, Some(Color::Indexed(255)));
    }

    #[test]
    fn test_color_mode_for_theme() {
        let basic = Theme::for_color_mode(ColorMode::Basic);
        let indexed = Theme::for_color_mode(ColorMode::Indexed);
        let truecolor = Theme::for_color_mode(ColorMode::TrueColor);

        assert_eq!(basic.border_focused, Color::Cyan);
        assert_eq!(indexed.border_focused, Color::Indexed(117));
        assert_eq!(truecolor.border_focused, Color::Rgb(137, 180, 250));
    }
}
