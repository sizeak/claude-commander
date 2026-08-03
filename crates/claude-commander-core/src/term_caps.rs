//! Terminal colour capability, and the tmux colour vocabulary derived from it.
//!
//! This is environment sniffing plus string formatting, not rendering, so it
//! belongs to the library rather than to a frontend: core needs a tmux
//! `status-style` string when it constructs a [`SessionManager`], and the
//! telemetry env fingerprint records the detected capability. Both would
//! otherwise force core to reach into the TUI crate.
//!
//! [`ColorMode::status_bar_colors`] is the single source of truth for the
//! built-in presets' status-bar colours — the TUI's `Theme::basic`/`indexed`/
//! `truecolor` read it rather than repeating the pairs.
//!
//! [`SessionManager`]: crate::session::SessionManager

use ratatui::style::Color;

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
        if let Ok(colorterm) = std::env::var("COLORTERM")
            && (colorterm == "truecolor" || colorterm == "24bit")
        {
            return Self::TrueColor;
        }

        // Check TERM for 256 color support
        if let Ok(term) = std::env::var("TERM")
            && (term.contains("256color") || term.contains("kitty") || term.contains("alacritty"))
        {
            // These terminals typically support true color even without COLORTERM
            if term.contains("kitty") || term.contains("alacritty") {
                return Self::TrueColor;
            }
            return Self::Indexed;
        }

        Self::Basic
    }

    /// Stable lowercase identifier, used as the telemetry `color_mode` value.
    pub fn name(self) -> &'static str {
        match self {
            ColorMode::Basic => "basic",
            ColorMode::Indexed => "indexed",
            ColorMode::TrueColor => "truecolor",
        }
    }

    /// `(background, foreground)` for the status bar in the built-in preset for
    /// this capability tier. The TUI's `Theme` presets read these so the pairs
    /// are defined once; the named presets (monokai-dimmed, rose-pine, …) pick
    /// their own and are not represented here.
    pub fn status_bar_colors(self) -> (Color, Color) {
        match self {
            ColorMode::Basic => (Color::Blue, Color::White),
            ColorMode::Indexed => (Color::Indexed(236), Color::Indexed(252)),
            ColorMode::TrueColor => (Color::Rgb(49, 50, 68), Color::Rgb(205, 214, 244)),
        }
    }

    /// tmux-compatible `status-style` string for this tier's status bar, so a
    /// session's tmux status bar matches the TUI's own.
    ///
    /// Note this deliberately reflects the *auto-detected* preset only: it
    /// ignores a configured `theme` preset and `[theme]` overrides, matching the
    /// behaviour of the `Theme::default().tmux_status_style()` call it replaced.
    pub fn tmux_status_style(self) -> String {
        let (bg, fg) = self.status_bar_colors();
        format!("bg={},fg={}", color_to_tmux(bg), color_to_tmux(fg))
    }
}

/// Convert a ratatui `Color` to a tmux-compatible color string
pub fn color_to_tmux(color: Color) -> String {
    match color {
        Color::Rgb(r, g, b) => format!("#{:02x}{:02x}{:02x}", r, g, b),
        Color::Indexed(n) => format!("colour{}", n),
        Color::Black => "black".into(),
        Color::Red => "red".into(),
        Color::Green => "green".into(),
        Color::Yellow => "yellow".into(),
        Color::Blue => "blue".into(),
        Color::Magenta => "magenta".into(),
        Color::Cyan => "cyan".into(),
        Color::White | Color::Gray => "white".into(),
        Color::DarkGray => "brightblack".into(),
        Color::LightRed => "brightred".into(),
        Color::LightGreen => "brightgreen".into(),
        Color::LightYellow => "brightyellow".into(),
        Color::LightBlue => "brightblue".into(),
        Color::LightMagenta => "brightmagenta".into(),
        Color::LightCyan => "brightcyan".into(),
        Color::Reset => "default".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_color_to_tmux_rgb() {
        assert_eq!(color_to_tmux(Color::Rgb(49, 50, 68)), "#313244");
        assert_eq!(color_to_tmux(Color::Rgb(0, 0, 0)), "#000000");
        assert_eq!(color_to_tmux(Color::Rgb(255, 255, 255)), "#ffffff");
    }

    #[test]
    fn test_color_to_tmux_indexed() {
        assert_eq!(color_to_tmux(Color::Indexed(236)), "colour236");
        assert_eq!(color_to_tmux(Color::Indexed(0)), "colour0");
    }

    #[test]
    fn test_color_to_tmux_named() {
        assert_eq!(color_to_tmux(Color::Blue), "blue");
        assert_eq!(color_to_tmux(Color::White), "white");
        assert_eq!(color_to_tmux(Color::DarkGray), "brightblack");
        assert_eq!(color_to_tmux(Color::Reset), "default");
    }

    /// The strings tmux is configured with must not drift: they were previously
    /// produced by `Theme::default().tmux_status_style()` and are compared
    /// byte-for-byte here so the move stayed behaviour-preserving.
    #[test]
    fn test_tmux_status_style_per_color_mode() {
        assert_eq!(ColorMode::Basic.tmux_status_style(), "bg=blue,fg=white");
        assert_eq!(
            ColorMode::Indexed.tmux_status_style(),
            "bg=colour236,fg=colour252"
        );
        assert_eq!(
            ColorMode::TrueColor.tmux_status_style(),
            "bg=#313244,fg=#cdd6f4"
        );
    }

    /// `name()` feeds the recorded telemetry `color_mode` field, so the three
    /// values are part of the wire format and must stay stable.
    #[test]
    fn test_color_mode_names_are_stable() {
        assert_eq!(ColorMode::Basic.name(), "basic");
        assert_eq!(ColorMode::Indexed.name(), "indexed");
        assert_eq!(ColorMode::TrueColor.name(), "truecolor");
        // Whatever the live environment is, the recorded value is one of those.
        assert!(matches!(
            ColorMode::detect().name(),
            "basic" | "indexed" | "truecolor"
        ));
    }
}
