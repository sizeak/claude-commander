//! TUI Theme configuration
//!
//! Centralized theme system for consistent styling across the UI.
//! Supports multiple color depths for terminal compatibility.

use diffgrid::style::{Appearance, Ink, Palette, Rgb, Role};
use ratatui::style::{Color, Style};

use claude_commander_core::config::theme::{AgentWorkingStyle, ThemeOverrides};
use claude_commander_core::term_caps::ColorMode;

/// All recognised preset names, in display order.
pub const PRESET_NAMES: &[&str] = &[
    "(auto)",
    "basic",
    "indexed",
    "truecolor",
    "monokai-dimmed",
    "zedokai",
    "rose-pine",
    "lcars",
];

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
    pub status_creating: Color,
    pub status_running: Color,
    pub status_stopped: Color,
    pub status_pr: Color,
    pub status_pr_merged: Color,

    // PR badge text colours (per state). `status_pr` is reused for the
    // "open + awaiting review" colour.
    pub pr_open: Color,
    pub pr_draft: Color,
    pub pr_closed: Color,

    // PR label pill backgrounds (darker variants of the text colours above)
    // used when `invert_pr_label_color = false`.
    pub pr_pill_open_bg: Color,
    pub pr_pill_draft_bg: Color,
    pub pr_pill_closed_bg: Color,
    pub pr_pill_review_bg: Color,
    pub pr_pill_merged_bg: Color,
    /// Text colour used on top of PR pill backgrounds.
    pub pr_pill_text: Color,

    // Agent state and notification indicators
    pub agent_working: AgentWorkingStyle,
    pub agent_waiting: Color,
    pub unread_indicator: Color,

    // Text
    pub text_primary: Color,
    pub text_secondary: Color,
    pub text_accent: Color,
    /// Accent for conversation mode (the assistant's name, reply text framing,
    /// and the input rules) — a warm light purple/pink, distinct from the
    /// session/agent status colours.
    pub conversation_accent: Color,
    pub project_colors: Vec<(Color, Color)>, // (project_header, session_title)

    // Diff colors
    pub diff_added: Color,
    pub diff_removed: Color,
    pub diff_hunk_header: Color,
    pub diff_file_header: Color,
    pub diff_context: Color,
    /// Background band behind revealed context lines (GitHub-style expand) in
    /// the review diff view.
    pub diff_expand_bg: Color,
    /// Background band behind hunk-header (`@@ … @@`) lines in the review diff
    /// view; kept dimmer than `diff_expand_bg`.
    pub diff_hunk_header_bg: Color,

    // Modal borders
    pub modal_info: Color,
    pub modal_warning: Color,
    pub modal_error: Color,

    // Quick-switch palette: background + text for *command* rows, so they
    // stand out visually from session rows in the unified palette view.
    pub palette_command_bg: Color,
    pub palette_command_fg: Color,

    // Status bar
    pub status_bar_bg: Color,
    pub status_bar_fg: Color,
    /// Accent for the hotkey letter in `[n]ew session` and the board's top-bar
    /// title, both of which are painted *on the status bar*.
    ///
    /// Not `text_accent`: that is tuned to read on the canvas, and reusing it
    /// here only worked while every preset's bar was dark. See
    /// `every_preset_status_bar_accent_is_legible_on_its_bar`.
    pub status_bar_accent: Color,

    /// Colour capability this theme was built for. Drives capability-aware
    /// palettes (e.g. the review diff view) so RGB fills degrade gracefully.
    pub mode: ColorMode,

    /// Whether this theme is drawn on a light or a dark terminal background.
    ///
    /// The terminal never tells us, and asking it (`OSC 11`) is deliberately out
    /// of scope, so this is a flag a preset declares about itself. It matters
    /// because a derived *fill* has to be scaled toward the surface it sits on:
    /// scaling a green toward black on a light terminal gives a muddy near-black
    /// band, which is what [`fill_color`] used to do unconditionally.
    pub appearance: Appearance,
}

impl Default for Theme {
    fn default() -> Self {
        Self::for_color_mode(ColorMode::detect())
    }
}

/// Colour palette for the full-screen review diff view, derived entirely from
/// the active [`Theme`] so it follows the user's chosen preset. Foregrounds use
/// the theme's diff/text/border colours directly; the add/remove line *fills*
/// are dimmed from the theme's diff colours on true-color terminals, fall back
/// to fixed dark indexed fills on 256-colour, and to foreground-only on
/// 16-colour.
#[derive(Debug, Clone, Copy)]
pub struct ReviewPalette {
    /// Background fill for an added line.
    pub add_bg: Color,
    /// Background fill for a removed line.
    pub del_bg: Color,
    /// Brighter fill for the changed span within an added line (word diff).
    pub add_emph_bg: Color,
    /// Brighter fill for the changed span within a removed line (word diff).
    pub del_emph_bg: Color,
    /// Gutter (line-number column) background on added / removed lines.
    pub add_gutter_bg: Color,
    pub del_gutter_bg: Color,
    /// Line-number / gutter foreground (dim).
    pub gutter_fg: Color,
    /// Default code foreground.
    pub text: Color,
    /// Foreground for the diagonal-hatch fill in alignment gaps.
    pub gap_fg: Color,
    /// Hunk header foreground.
    pub hunk_header: Color,
    /// Foreground for added lines' `+` sign and added-file rows.
    pub add_fg: Color,
    /// Foreground for removed lines' `-` sign and deleted-file rows.
    pub del_fg: Color,
    /// File-tree row colours for modified / renamed files.
    pub modified_fg: Color,
    pub renamed_fg: Color,
    /// File-tree directory colour.
    pub dir_fg: Color,
    /// Border colour for a staged comment box.
    pub comment_border: Color,
    /// Border colour for a drifted comment box.
    pub drift_border: Color,
    /// Border colour for the in-progress comment edit box.
    pub draft_border: Color,
    /// Pane border colours (focused / unfocused), matching the rest of the UI.
    pub border_focused: Color,
    pub border_unfocused: Color,
    /// Selection highlight, matching the session list.
    pub selection_bg: Color,
    pub selection_fg: Option<Color>,
    /// Selection highlight for the file list while focus sits in the diff body:
    /// the focused selection, background *and* foreground, muted toward the
    /// surface. The cursor row keeps a highlight at all times — without one
    /// there is nothing on screen saying which file the body is showing — and
    /// muting the whole selection keeps "which pane has the keys" readable.
    ///
    /// Both halves move together deliberately. A theme is free to carry its
    /// selection mostly in the foreground (LCARS' band is `Rgb(36, 24, 9)`,
    /// barely off black, under tan text), so muting only the background would
    /// erase the row's only signal on exactly those themes.
    pub selection_bg_unfocused: Color,
    pub selection_fg_unfocused: Option<Color>,
    /// Subtle background band laid across file-tree rows marked reviewed, so a
    /// "read" file is obvious at a glance beyond the ` ✓` check alone.
    pub reviewed_bg: Color,
    /// Subtle background band on revealed context lines (GitHub-style expand)
    /// and their expand controls, so the extra file lines are clearly distinct
    /// from the diff's own ±3 context.
    pub context_bg: Color,
    /// Very subtle background band behind hunk-header (`@@ … @@`) lines — dimmer
    /// than [`Self::context_bg`] so a header reads as a quiet divider distinct
    /// from the revealed-context band.
    pub hunk_header_bg: Color,
    /// Colour capability, carried so [`Palette::syntax`] can degrade a
    /// truecolor highlight rather than emitting an RGB escape a 16-colour
    /// terminal will render as something arbitrary.
    pub mode: ColorMode,
}

/// The review view's [`diffgrid`] palette: semantic [`Role`] in, `ratatui`
/// colours out.
///
/// This is the *whole* of what the review view has to write to render a diff.
/// The colours themselves still come from the user's theme preset, already
/// degraded for the terminal's capability by [`Theme::review_palette`]; this
/// impl only says which of them a given role wears.
impl Palette for ReviewPalette {
    type Color = Color;

    fn role(&self, role: Role) -> Ink<Color> {
        use diffgrid::LineOrigin;
        let ink = |fg, bg| Ink::new(fg, bg, false);
        match role {
            Role::Context => ink(self.text, Color::Reset),
            Role::Addition => ink(self.text, self.add_bg),
            Role::Deletion => ink(self.text, self.del_bg),
            Role::Gutter(LineOrigin::Context) => ink(self.gutter_fg, Color::Reset),
            Role::Gutter(LineOrigin::Addition) => ink(self.gutter_fg, self.add_gutter_bg),
            Role::Gutter(LineOrigin::Deletion) => ink(self.gutter_fg, self.del_gutter_bg),
            Role::HunkHeader => ink(self.hunk_header, self.hunk_header_bg),
            // Revealed context and its expand control share the band, so the
            // extra file lines read as one region distinct from the diff's own
            // ±3 context.
            Role::ExpandedContext | Role::ExpandControl => ink(self.gutter_fg, self.context_bg),
            Role::AlignmentGap => ink(self.gap_fg, Color::Reset),
            Role::Padding => ink(self.text, Color::Reset),
            // `Role` is `#[non_exhaustive]`: an unrecognised role renders as
            // plain text rather than failing to build the day one is added.
            _ => ink(self.text, Color::Reset),
        }
    }

    fn emphasis_bg(&self, role: Role) -> Color {
        match role {
            Role::Addition => self.add_emph_bg,
            Role::Deletion => self.del_emph_bg,
            // Only a changed line can carry word-diff emphasis; anything else
            // keeps its own fill rather than borrowing a colour it never wears.
            other => self.role(other).bg,
        }
    }

    fn selection_bg(&self) -> Color {
        self.selection_bg
    }

    fn syntax(&self, rgb: Rgb) -> Color {
        match self.mode {
            ColorMode::TrueColor => Color::Rgb(rgb.r, rgb.g, rgb.b),
            // Below truecolor the review view passes no highlighter at all, so
            // this is unreachable in practice — but a palette that answered
            // with an RGB escape anyway would be a trap for the next caller.
            _ => self.text,
        }
    }

    /// Two deviations from the default precedence, both pre-existing behaviour:
    ///
    /// * a theme that sets `selection_fg` overrides the foreground on a
    ///   selected row, syntax highlight included — the session list does the
    ///   same, and a cursor row that keeps per-token colours does not read as
    ///   selected;
    /// * an expand control's *actionable* segments (which
    ///   [`diffgrid::style::expand_control_spans`] marks bold) take the hunk-header
    ///   accent, so "click me" is visibly distinct from the "n hidden lines" hint.
    fn ink(&self, style: &diffgrid::style::SpanStyle) -> Ink<Color> {
        let base = self.role(style.role);
        let bg = if style.selected {
            self.selection_bg
        } else if style.emphasis {
            self.emphasis_bg(style.role)
        } else {
            base.bg
        };
        let mut fg = match style.syntax_fg {
            Some(rgb) => self.syntax(rgb),
            None => base.fg,
        };
        if style.role == Role::ExpandControl && style.bold {
            fg = self.hunk_header;
        }
        if let Some(sel_fg) = self.selection_fg.filter(|_| style.selected) {
            fg = sel_fg;
        }
        Ink {
            fg,
            bg,
            bold: style.bold || base.bold,
        }
    }
}

impl Theme {
    /// The review-diff palette, derived from this theme.
    pub fn review_palette(&self) -> ReviewPalette {
        let add = self.diff_added;
        let del = self.diff_removed;
        // Line fills: dimmed theme colours on true-color; fixed indexed darks
        // on 256-colour; none (foreground-only) on 16-colour.
        let fill = |base, strength| fill_color(base, strength, self.appearance);
        let (add_bg, del_bg, add_emph_bg, del_emph_bg, add_gutter_bg, del_gutter_bg) =
            match self.mode {
                ColorMode::TrueColor => (
                    fill(add, 0.26),
                    fill(del, 0.26),
                    fill(add, 0.40),
                    fill(del, 0.40),
                    fill(add, 0.34),
                    fill(del, 0.34),
                ),
                ColorMode::Indexed => (
                    Color::Indexed(22),
                    Color::Indexed(52),
                    Color::Indexed(28),
                    Color::Indexed(88),
                    Color::Indexed(22),
                    Color::Indexed(52),
                ),
                ColorMode::Basic => (
                    Color::Reset,
                    Color::Reset,
                    Color::Green,
                    Color::Red,
                    Color::Reset,
                    Color::Reset,
                ),
            };
        // A subtle green band reads as "done", complementing the green ✓ check.
        // No band on 16-colour terminals — the dim + check carry it there.
        let reviewed_bg = match self.mode {
            ColorMode::TrueColor => fill(add, 0.18),
            ColorMode::Indexed => Color::Indexed(22),
            ColorMode::Basic => Color::Reset,
        };
        // The unfocused file list's highlight: the selection at 70% strength,
        // both halves scaled together so the rule is simply "the same row,
        // quieter". Only true-color can express that; below it the palette
        // keeps the full selection (the pane border still marks focus) rather
        // than emitting an RGB escape the terminal would render as something
        // arbitrary.
        const UNFOCUSED: f32 = 0.7;
        let (selection_bg_unfocused, selection_fg_unfocused) = match self.mode {
            ColorMode::TrueColor => (
                toward_surface(self.selection_bg, UNFOCUSED, self.appearance),
                self.selection_fg
                    .map(|fg| toward_surface(fg, UNFOCUSED, self.appearance)),
            ),
            ColorMode::Indexed | ColorMode::Basic => (self.selection_bg, self.selection_fg),
        };
        // Named theme bands for the review diff view.
        let context_bg = self.diff_expand_bg;
        let hunk_header_bg = self.diff_hunk_header_bg;
        ReviewPalette {
            add_bg,
            del_bg,
            add_emph_bg,
            del_emph_bg,
            add_gutter_bg,
            del_gutter_bg,
            gutter_fg: self.text_secondary,
            text: self.text_primary,
            gap_fg: self.border_unfocused,
            hunk_header: self.diff_hunk_header,
            add_fg: add,
            del_fg: del,
            modified_fg: self.diff_file_header,
            renamed_fg: self.text_accent,
            dir_fg: self.text_accent,
            comment_border: self.diff_file_header,
            drift_border: del,
            draft_border: self.modal_warning,
            border_focused: self.border_focused,
            border_unfocused: self.border_unfocused,
            selection_bg: self.selection_bg,
            selection_fg: self.selection_fg,
            selection_bg_unfocused,
            selection_fg_unfocused,
            reviewed_bg,
            context_bg,
            hunk_header_bg,
            mode: self.mode,
        }
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
        let (status_bar_bg, status_bar_fg) = ColorMode::Basic.status_bar_colors();
        Self {
            mode: ColorMode::Basic,
            appearance: Appearance::Dark,
            border_focused: Color::Cyan,
            border_unfocused: Color::DarkGray,

            selection_bg: Color::Blue,
            selection_fg: Some(Color::White),

            status_creating: Color::Yellow,
            status_running: Color::Green,
            status_stopped: Color::DarkGray,
            status_pr: Color::Green,
            status_pr_merged: Color::DarkGray,

            pr_open: Color::Blue,
            pr_draft: Color::DarkGray,
            pr_closed: Color::Red,

            // Basic 16-colour palette: the non-"Light" ANSI colours are
            // already dark enough to read white bold text on top.
            pr_pill_open_bg: Color::Blue,
            pr_pill_draft_bg: Color::DarkGray,
            pr_pill_closed_bg: Color::Red,
            pr_pill_review_bg: Color::Green,
            pr_pill_merged_bg: Color::Magenta,
            pr_pill_text: Color::White,

            agent_working: AgentWorkingStyle::Rainbow,
            agent_waiting: Color::Yellow,
            unread_indicator: Color::Blue,

            text_primary: Color::Reset,
            text_secondary: Color::DarkGray,
            text_accent: Color::Blue,
            conversation_accent: Color::LightMagenta,
            project_colors: vec![
                (Color::Magenta, Color::LightMagenta),
                (Color::Cyan, Color::LightCyan),
                (Color::Blue, Color::LightBlue),
                (Color::Yellow, Color::LightYellow),
                (Color::Green, Color::LightGreen),
                (Color::Red, Color::LightRed),
            ],

            diff_added: Color::Green,
            diff_removed: Color::Red,
            diff_hunk_header: Color::Cyan,
            diff_file_header: Color::Yellow,
            diff_context: Color::Reset,
            // 16-colour has no room for subtle bands — foreground only.
            diff_expand_bg: Color::Reset,
            diff_hunk_header_bg: Color::Reset,

            modal_info: Color::Cyan,
            modal_warning: Color::Yellow,
            modal_error: Color::Red,

            palette_command_bg: Color::DarkGray,
            palette_command_fg: Color::White,

            status_bar_bg,
            status_bar_fg,
            // Not Blue: `text_accent` is Blue here, and a blue letter on the
            // blue bar was invisible but for its bold.
            status_bar_accent: Color::LightYellow,
        }
    }

    /// 256-color theme (good balance of compatibility and aesthetics)
    pub fn indexed() -> Self {
        let (status_bar_bg, status_bar_fg) = ColorMode::Indexed.status_bar_colors();
        Self {
            mode: ColorMode::Indexed,
            appearance: Appearance::Dark,
            border_focused: Color::Indexed(117), // Pastel sky blue
            border_unfocused: Color::Indexed(243),

            selection_bg: Color::Indexed(60), // Muted purple-blue
            selection_fg: Some(Color::Indexed(255)),

            status_creating: Color::Indexed(228), // Pastel yellow
            status_running: Color::Indexed(156),  // Pastel mint green
            status_stopped: Color::Indexed(248),
            status_pr: Color::Indexed(114),       // Pastel green
            status_pr_merged: Color::Indexed(97), // Dark purple

            pr_open: Color::Indexed(111),   // Pastel sky blue
            pr_draft: Color::Indexed(245),  // Mid-grey
            pr_closed: Color::Indexed(167), // Soft red

            // Darker variants readable with bold near-white text
            pr_pill_open_bg: Color::Indexed(24),    // Dark blue
            pr_pill_draft_bg: Color::Indexed(240),  // Dark grey
            pr_pill_closed_bg: Color::Indexed(124), // Dark red
            pr_pill_review_bg: Color::Indexed(22),  // Dark green
            pr_pill_merged_bg: Color::Indexed(54),  // Very dark purple
            pr_pill_text: Color::Indexed(231),      // Near-white

            agent_working: AgentWorkingStyle::Rainbow,
            agent_waiting: Color::Indexed(208),    // Orange
            unread_indicator: Color::Indexed(117), // Sky blue

            text_primary: Color::Reset,
            text_secondary: Color::Indexed(250),
            text_accent: Color::Indexed(147), // Pastel lavender
            conversation_accent: Color::Indexed(218), // Light pink #ffafd7
            project_colors: vec![
                (Color::Indexed(168), Color::Indexed(218)), // Pink
                (Color::Indexed(68), Color::Indexed(117)),  // Blue
                (Color::Indexed(71), Color::Indexed(157)),  // Green
                (Color::Indexed(173), Color::Indexed(222)), // Orange
                (Color::Indexed(134), Color::Indexed(183)), // Purple
                (Color::Indexed(73), Color::Indexed(152)),  // Teal
            ],

            diff_added: Color::Indexed(156),       // Pastel mint
            diff_removed: Color::Indexed(210),     // Pastel coral
            diff_hunk_header: Color::Indexed(183), // Pastel orchid
            diff_file_header: Color::Indexed(223), // Pastel cream
            diff_context: Color::Reset,
            diff_expand_bg: Color::Indexed(236), // Matches the status bar band
            diff_hunk_header_bg: Color::Indexed(234), // Dimmer divider band

            modal_info: Color::Indexed(117),    // Pastel sky
            modal_warning: Color::Indexed(222), // Pastel peach
            modal_error: Color::Indexed(210),   // Pastel coral

            palette_command_bg: Color::Indexed(23), // Deep muted turquoise
            palette_command_fg: Color::Indexed(252), // Near-white

            status_bar_bg,
            status_bar_fg,
            status_bar_accent: Color::Indexed(147), // Matches text_accent
        }
    }

    /// True color theme (richest visual experience)
    pub fn truecolor() -> Self {
        let (status_bar_bg, status_bar_fg) = ColorMode::TrueColor.status_bar_colors();
        Self {
            mode: ColorMode::TrueColor,
            appearance: Appearance::Dark,
            border_focused: Color::Rgb(137, 180, 250), // Pastel sky blue
            border_unfocused: Color::Rgb(88, 91, 112),

            selection_bg: Color::Rgb(69, 71, 90),
            selection_fg: Some(Color::Rgb(245, 245, 250)),

            status_creating: Color::Rgb(249, 240, 107), // Pastel yellow
            status_running: Color::Rgb(166, 227, 161),  // Pastel mint
            status_stopped: Color::Rgb(147, 153, 178),  // Muted lavender
            status_pr: Color::Rgb(126, 198, 153),       // Soft GitHub-ish green
            status_pr_merged: Color::Rgb(137, 100, 180), // Dark purple

            pr_open: Color::Rgb(137, 180, 250),  // Pastel sky blue
            pr_draft: Color::Rgb(147, 153, 178), // Muted grey-lavender
            pr_closed: Color::Rgb(243, 139, 168), // Pastel rose / soft red

            // Darker variants for pill backgrounds, readable with bold near-white text
            pr_pill_open_bg: Color::Rgb(40, 80, 160), // Dark blue
            pr_pill_draft_bg: Color::Rgb(80, 86, 110), // Dark slate
            pr_pill_closed_bg: Color::Rgb(160, 60, 85), // Dark rose
            pr_pill_review_bg: Color::Rgb(46, 125, 80), // Dark green
            pr_pill_merged_bg: Color::Rgb(75, 55, 110), // Very dark purple
            pr_pill_text: Color::Rgb(245, 245, 250),  // Near-white

            agent_working: AgentWorkingStyle::Rainbow,
            agent_waiting: Color::Rgb(250, 179, 135), // Peach/orange
            unread_indicator: Color::Rgb(137, 180, 250), // Sky blue

            text_primary: Color::Rgb(245, 245, 250),
            text_secondary: Color::Rgb(166, 173, 200),
            text_accent: Color::Rgb(180, 190, 254), // Pastel periwinkle
            conversation_accent: Color::Rgb(245, 194, 231), // Pastel pink #f5c2e7
            project_colors: vec![
                (Color::Rgb(199, 120, 140), Color::Rgb(243, 174, 190)), // Pink
                (Color::Rgb(100, 140, 210), Color::Rgb(160, 190, 245)), // Blue
                (Color::Rgb(100, 165, 110), Color::Rgb(166, 218, 170)), // Green
                (Color::Rgb(210, 160, 100), Color::Rgb(245, 210, 165)), // Orange
                (Color::Rgb(160, 130, 200), Color::Rgb(200, 175, 240)), // Purple
                (Color::Rgb(90, 170, 170), Color::Rgb(155, 215, 215)),  // Teal
            ],

            diff_added: Color::Rgb(166, 227, 161), // Pastel mint
            diff_removed: Color::Rgb(243, 139, 168), // Pastel rose
            diff_hunk_header: Color::Rgb(203, 166, 247), // Pastel mauve
            diff_file_header: Color::Rgb(249, 226, 175), // Pastel peach
            diff_context: Color::Reset,
            diff_expand_bg: Color::Rgb(49, 50, 68), // Matches the status bar band
            diff_hunk_header_bg: Color::Rgb(30, 31, 44), // Dimmer divider band

            modal_info: Color::Rgb(137, 180, 250), // Pastel sky
            modal_warning: Color::Rgb(249, 226, 175), // Pastel peach
            modal_error: Color::Rgb(243, 139, 168), // Pastel rose

            palette_command_bg: Color::Rgb(60, 88, 92), // Deep muted turquoise-teal
            palette_command_fg: Color::Rgb(205, 214, 244), // Near-white lavender

            status_bar_bg,
            status_bar_fg,
            status_bar_accent: Color::Rgb(180, 190, 254), // Matches text_accent
        }
    }

    /// Monokai Dimmed — a muted/desaturated take on the classic Monokai color scheme
    pub fn monokai_dimmed() -> Self {
        Self {
            mode: ColorMode::TrueColor,
            appearance: Appearance::Dark,
            border_focused: Color::Rgb(181, 165, 106), // Muted gold/yellow #b5a56a
            border_unfocused: Color::Rgb(85, 85, 85),  // Dark gray #555555

            selection_bg: Color::Rgb(58, 61, 65), // Dark blue-gray #3a3d41
            selection_fg: Some(Color::Rgb(255, 255, 255)),

            status_creating: Color::Rgb(220, 220, 170), // Muted yellow #dcdcaa
            status_running: Color::Rgb(181, 206, 168),  // Muted green #b5cea8
            status_stopped: Color::Rgb(128, 128, 128),  // Gray #808080
            status_pr: Color::Rgb(181, 206, 168),       // Muted green
            status_pr_merged: Color::Rgb(128, 128, 128), // Gray

            pr_open: Color::Rgb(103, 150, 230), // Muted blue #6796e6
            pr_draft: Color::Rgb(128, 128, 128), // Gray #808080
            pr_closed: Color::Rgb(209, 105, 105), // Muted red #d16969

            pr_pill_open_bg: Color::Rgb(45, 65, 120), // Dark muted blue
            pr_pill_draft_bg: Color::Rgb(70, 70, 70), // Dark gray
            pr_pill_closed_bg: Color::Rgb(120, 50, 50), // Dark muted red
            pr_pill_review_bg: Color::Rgb(55, 90, 55), // Dark muted green
            pr_pill_merged_bg: Color::Rgb(85, 60, 100), // Dark muted purple
            pr_pill_text: Color::Rgb(230, 230, 230),  // Near-white

            agent_working: AgentWorkingStyle::Rainbow,
            agent_waiting: Color::Rgb(220, 220, 170), // Muted yellow #dcdcaa
            unread_indicator: Color::Rgb(103, 150, 230), // Muted blue #6796e6

            text_primary: Color::Rgb(212, 212, 212), // Light gray #d4d4d4
            text_secondary: Color::Rgb(150, 150, 150), // Medium gray #969696
            text_accent: Color::Rgb(124, 165, 212),  // Muted blue #7ca5d4
            conversation_accent: Color::Rgb(198, 163, 207), // Muted lilac
            project_colors: vec![
                (Color::Rgb(181, 165, 106), Color::Rgb(220, 220, 170)), // Gold / muted yellow
                (Color::Rgb(100, 160, 160), Color::Rgb(140, 200, 200)), // Teal
                (Color::Rgb(206, 145, 120), Color::Rgb(230, 180, 160)), // Coral
                (Color::Rgb(106, 130, 180), Color::Rgb(150, 170, 210)), // Slate-blue
                (Color::Rgb(140, 170, 140), Color::Rgb(181, 206, 168)), // Sage
                (Color::Rgb(170, 140, 170), Color::Rgb(197, 134, 192)), // Mauve
            ],

            diff_added: Color::Rgb(181, 206, 168), // Muted green #b5cea8
            diff_removed: Color::Rgb(209, 105, 105), // Muted red #d16969
            diff_hunk_header: Color::Rgb(197, 134, 192), // Muted purple #c586c0
            diff_file_header: Color::Rgb(220, 220, 170), // Muted yellow #dcdcaa
            diff_context: Color::Reset,
            diff_expand_bg: Color::Rgb(45, 45, 45), // Matches the status bar band
            diff_hunk_header_bg: Color::Rgb(28, 28, 28), // Dimmer divider band

            modal_info: Color::Rgb(124, 165, 212), // Muted blue #7ca5d4
            modal_warning: Color::Rgb(220, 220, 170), // Muted yellow #dcdcaa
            modal_error: Color::Rgb(209, 105, 105), // Muted red #d16969

            palette_command_bg: Color::Rgb(50, 50, 50), // Dark gray
            palette_command_fg: Color::Rgb(204, 204, 204), // Light gray #cccccc

            status_bar_bg: Color::Rgb(45, 45, 45), // Dark gray #2d2d2d
            status_bar_fg: Color::Rgb(204, 204, 204), // Light gray #cccccc
            status_bar_accent: Color::Rgb(124, 165, 212), // Matches text_accent
        }
    }

    /// Zedokai — inspired by the Zed editor's Monokai variant with a filter/spectrum twist
    pub fn zedokai() -> Self {
        Self {
            mode: ColorMode::TrueColor,
            appearance: Appearance::Dark,
            border_focused: Color::Rgb(249, 38, 114), // Vivid pink #f92672
            border_unfocused: Color::Rgb(73, 72, 62), // Dark gray #49483e

            selection_bg: Color::Rgb(73, 72, 62), // Dark warm gray #49483e
            selection_fg: Some(Color::Rgb(248, 248, 242)), // Bright white #f8f8f2

            status_creating: Color::Rgb(253, 151, 31), // Vivid orange #fd971f
            status_running: Color::Rgb(166, 226, 46),  // Bright green #a6e22e
            status_stopped: Color::Rgb(117, 113, 94),  // Warm gray #75715e
            status_pr: Color::Rgb(166, 226, 46),       // Bright green
            status_pr_merged: Color::Rgb(117, 113, 94), // Warm gray

            pr_open: Color::Rgb(102, 217, 239), // Sky blue #66d9ef
            pr_draft: Color::Rgb(117, 113, 94), // Warm gray #75715e
            pr_closed: Color::Rgb(249, 38, 114), // Pink #f92672

            pr_pill_open_bg: Color::Rgb(30, 90, 100), // Dark cyan
            pr_pill_draft_bg: Color::Rgb(55, 55, 48), // Dark warm gray
            pr_pill_closed_bg: Color::Rgb(120, 15, 55), // Dark pink
            pr_pill_review_bg: Color::Rgb(50, 100, 15), // Dark green
            pr_pill_merged_bg: Color::Rgb(70, 45, 100), // Dark purple
            pr_pill_text: Color::Rgb(248, 248, 242),  // Bright white #f8f8f2

            agent_working: AgentWorkingStyle::Rainbow,
            agent_waiting: Color::Rgb(253, 151, 31), // Orange #fd971f
            unread_indicator: Color::Rgb(102, 217, 239), // Sky blue #66d9ef

            text_primary: Color::Rgb(248, 248, 242), // Warm white #f8f8f2
            text_secondary: Color::Rgb(117, 113, 94), // Warm gray #75715e
            text_accent: Color::Rgb(174, 129, 255),  // Vivid purple #ae81ff
            conversation_accent: Color::Rgb(217, 160, 255), // Light purple #d9a0ff
            project_colors: vec![
                (Color::Rgb(249, 38, 114), Color::Rgb(255, 100, 150)), // Pink
                (Color::Rgb(102, 217, 239), Color::Rgb(140, 230, 245)), // Cyan
                (Color::Rgb(166, 226, 46), Color::Rgb(200, 240, 110)), // Green
                (Color::Rgb(253, 151, 31), Color::Rgb(255, 190, 100)), // Orange
                (Color::Rgb(174, 129, 255), Color::Rgb(200, 170, 255)), // Purple
                (Color::Rgb(230, 219, 116), Color::Rgb(245, 235, 160)), // Yellow
            ],

            diff_added: Color::Rgb(166, 226, 46), // Green #a6e22e
            diff_removed: Color::Rgb(249, 38, 114), // Pink #f92672
            diff_hunk_header: Color::Rgb(102, 217, 239), // Blue #66d9ef
            diff_file_header: Color::Rgb(230, 219, 116), // Yellow #e6db74
            diff_context: Color::Reset,
            diff_expand_bg: Color::Rgb(30, 31, 28), // Matches the status bar band
            diff_hunk_header_bg: Color::Rgb(18, 19, 17), // Dimmer divider band

            modal_info: Color::Rgb(102, 217, 239), // Cyan #66d9ef
            modal_warning: Color::Rgb(253, 151, 31), // Orange #fd971f
            modal_error: Color::Rgb(249, 38, 114), // Pink #f92672

            palette_command_bg: Color::Rgb(50, 50, 42), // Dark warm gray
            palette_command_fg: Color::Rgb(248, 248, 242), // Warm white #f8f8f2

            status_bar_bg: Color::Rgb(30, 31, 28), // Very dark bg #1e1f1c
            status_bar_fg: Color::Rgb(248, 248, 242), // Warm white #f8f8f2
            status_bar_accent: Color::Rgb(174, 129, 255), // Matches text_accent
        }
    }

    /// Rosé Pine — a soft pink/rose aesthetic inspired by Rosé Pine
    pub fn rose_pine() -> Self {
        Self {
            mode: ColorMode::TrueColor,
            appearance: Appearance::Dark,
            border_focused: Color::Rgb(235, 111, 146), // Rose/pink #eb6f92
            border_unfocused: Color::Rgb(57, 53, 82),  // Muted overlay #393552

            selection_bg: Color::Rgb(42, 39, 63), // Subtle highlight #2a273f
            selection_fg: Some(Color::Rgb(224, 222, 244)), // Soft white #e0def4

            status_creating: Color::Rgb(246, 193, 119), // Gold #f6c177
            status_running: Color::Rgb(156, 207, 216),  // Foam/teal #9ccfd8
            status_stopped: Color::Rgb(110, 106, 134),  // Muted #6e6a86
            status_pr: Color::Rgb(156, 207, 216),       // Foam/teal
            status_pr_merged: Color::Rgb(110, 106, 134), // Muted

            pr_open: Color::Rgb(196, 167, 231), // Iris/purple #c4a7e7
            pr_draft: Color::Rgb(110, 106, 134), // Muted #6e6a86
            pr_closed: Color::Rgb(235, 111, 146), // Love/pink #eb6f92

            pr_pill_open_bg: Color::Rgb(80, 60, 110), // Dark iris
            pr_pill_draft_bg: Color::Rgb(55, 53, 70), // Dark muted
            pr_pill_closed_bg: Color::Rgb(110, 45, 65), // Dark love
            pr_pill_review_bg: Color::Rgb(40, 90, 95), // Dark foam
            pr_pill_merged_bg: Color::Rgb(50, 40, 75), // Dark iris variant
            pr_pill_text: Color::Rgb(224, 222, 244),  // Soft white #e0def4

            agent_working: AgentWorkingStyle::Rainbow,
            agent_waiting: Color::Rgb(246, 193, 119), // Gold #f6c177
            unread_indicator: Color::Rgb(196, 167, 231), // Iris #c4a7e7

            text_primary: Color::Rgb(224, 222, 244), // Soft white #e0def4
            text_secondary: Color::Rgb(144, 140, 170), // Subtle #908caa
            text_accent: Color::Rgb(196, 167, 231),  // Iris/purple #c4a7e7
            conversation_accent: Color::Rgb(235, 188, 186), // Rose #ebbcba
            project_colors: vec![
                (Color::Rgb(235, 111, 146), Color::Rgb(235, 188, 186)), // Rose / rose-lighter
                (Color::Rgb(196, 167, 231), Color::Rgb(196, 167, 231)), // Iris
                (Color::Rgb(156, 207, 216), Color::Rgb(156, 207, 216)), // Foam
                (Color::Rgb(246, 193, 119), Color::Rgb(246, 193, 119)), // Gold
                (Color::Rgb(49, 116, 143), Color::Rgb(62, 143, 176)),   // Pine
                (Color::Rgb(235, 111, 146), Color::Rgb(215, 130, 126)), // Love variant
            ],

            diff_added: Color::Rgb(156, 207, 216), // Foam #9ccfd8
            diff_removed: Color::Rgb(235, 111, 146), // Love/pink #eb6f92
            diff_hunk_header: Color::Rgb(196, 167, 231), // Iris #c4a7e7
            diff_file_header: Color::Rgb(246, 193, 119), // Gold #f6c177
            diff_context: Color::Reset,
            diff_expand_bg: Color::Rgb(31, 29, 46), // Matches the status bar band
            diff_hunk_header_bg: Color::Rgb(19, 18, 29), // Dimmer divider band

            modal_info: Color::Rgb(156, 207, 216), // Foam #9ccfd8
            modal_warning: Color::Rgb(246, 193, 119), // Gold #f6c177
            modal_error: Color::Rgb(235, 111, 146), // Love #eb6f92

            palette_command_bg: Color::Rgb(38, 35, 58), // Dark navy-rose #26233a
            palette_command_fg: Color::Rgb(224, 222, 244), // Soft white #e0def4

            status_bar_bg: Color::Rgb(31, 29, 46), // Dark bg #1f1d2e
            status_bar_fg: Color::Rgb(224, 222, 244), // Muted rose fg #e0def4
            status_bar_accent: Color::Rgb(196, 167, 231), // Matches text_accent
        }
    }

    /// LCARS — the Star Trek: TNG console palette: black canvas, amber primary,
    /// lilac and periwinkle accents, tan text.
    ///
    /// The peer of the Flutter client's LCARS theme, and the colours are
    /// transcribed from the same source: `client/lib/theme/tokens.dart`, itself a
    /// transcription of the design deck. Where a token has no matching field here
    /// the value is derived, and says so.
    ///
    /// Colour only. The client's LCARS is a *structural* re-skin — elbow rails,
    /// block panels, condensed uppercase type — and a terminal can render almost
    /// none of that, so none of it is attempted here.
    pub fn lcars() -> Self {
        Self {
            mode: ColorMode::TrueColor,
            appearance: Appearance::Dark,
            border_focused: Color::Rgb(247, 160, 29), // Amber #f7a01d
            border_unfocused: Color::Rgb(92, 74, 107), // Rail filler #5c4a6b

            selection_bg: Color::Rgb(36, 24, 9), // Deep amber-brown #241809
            selection_fg: Some(Color::Rgb(255, 204, 153)), // Tan #ffcc99

            status_creating: Color::Rgb(156, 156, 255), // Periwinkle #9c9cff
            // Amber, not a green: the deck's RUN blocks are amber, so `working`
            // collapses onto the primary here.
            status_running: Color::Rgb(247, 160, 29),
            status_stopped: Color::Rgb(92, 74, 107), // Idle #5c4a6b
            status_pr: Color::Rgb(143, 191, 143),    // Sage #8fbf8f
            status_pr_merged: Color::Rgb(204, 153, 204), // Lilac #cc99cc

            pr_open: Color::Rgb(156, 156, 255), // Periwinkle #9c9cff
            pr_draft: Color::Rgb(138, 122, 106), // Muted tan #8a7a6a
            pr_closed: Color::Rgb(204, 68, 68), // Danger #cc4444

            // Derived: each hue darkened by hand until bold tan reads on top.
            // The deck's own near-blacks are row *fills* behind body text, too
            // dark to distinguish five pill states from one another.
            pr_pill_open_bg: Color::Rgb(46, 46, 92), // Dark periwinkle
            pr_pill_draft_bg: Color::Rgb(43, 37, 48), // Dark neutral
            pr_pill_closed_bg: Color::Rgb(92, 30, 30), // Dark danger
            pr_pill_review_bg: Color::Rgb(46, 74, 46), // Dark sage
            pr_pill_merged_bg: Color::Rgb(74, 46, 74), // Dark lilac
            pr_pill_text: Color::Rgb(255, 204, 153), // Tan #ffcc99

            // The one preset that is not Rainbow: the shared palette's six
            // pastels are exactly the hues LCARS avoids.
            agent_working: AgentWorkingStyle::Solid(Color::Rgb(247, 160, 29)),
            agent_waiting: Color::Rgb(204, 102, 102), // Salmon #cc6666
            unread_indicator: Color::Rgb(156, 156, 255), // Periwinkle #9c9cff

            text_primary: Color::Rgb(255, 204, 153), // Tan #ffcc99
            text_secondary: Color::Rgb(138, 122, 106), // Muted tan #8a7a6a
            text_accent: Color::Rgb(204, 153, 204),  // Lilac #cc99cc
            // Derived: the lilac lightened, so conversation chrome stays in the
            // family without colliding with `text_accent`.
            conversation_accent: Color::Rgb(224, 179, 224),
            // Derived: the palette's six accents, each paired with a lightened
            // variant for the session title under the project header.
            project_colors: vec![
                (Color::Rgb(247, 160, 29), Color::Rgb(255, 204, 153)), // Amber / tan
                (Color::Rgb(204, 153, 204), Color::Rgb(224, 179, 224)), // Lilac
                (Color::Rgb(156, 156, 255), Color::Rgb(189, 189, 255)), // Periwinkle
                (Color::Rgb(204, 102, 102), Color::Rgb(224, 153, 153)), // Salmon
                (Color::Rgb(143, 191, 143), Color::Rgb(179, 217, 179)), // Sage
                (Color::Rgb(201, 143, 74), Color::Rgb(229, 184, 122)), // Held tan
            ],

            diff_added: Color::Rgb(143, 191, 143), // Sage #8fbf8f
            // Salmon rather than the deck's harder `danger` #cc4444: removed
            // lines are body text and have to stay legible at length.
            diff_removed: Color::Rgb(204, 102, 102), // Salmon #cc6666
            diff_hunk_header: Color::Rgb(204, 153, 204), // Lilac #cc99cc
            diff_file_header: Color::Rgb(255, 204, 153), // Tan #ffcc99
            diff_context: Color::Reset,
            diff_expand_bg: Color::Rgb(36, 29, 43), // Divider #241d2b
            diff_hunk_header_bg: Color::Rgb(18, 15, 20), // Dimmer divider band

            modal_info: Color::Rgb(156, 156, 255), // Periwinkle #9c9cff
            // The deck's `held` tan, which has no session-status field here.
            modal_warning: Color::Rgb(201, 143, 74), // #c98f4a
            modal_error: Color::Rgb(204, 68, 68),    // Danger #cc4444

            palette_command_bg: Color::Rgb(36, 24, 9), // Deep amber-brown #241809
            palette_command_fg: Color::Rgb(255, 204, 153), // Tan #ffcc99

            // A solid amber bar with black text, as the deck's rails are. This
            // also reaches attached sessions through `tmux_status_style`.
            status_bar_bg: Color::Rgb(247, 160, 29),
            status_bar_fg: Color::Rgb(0, 0, 0),
            // Dark periwinkle: amber's complement, so the hotkey letter reads
            // at ~6:1 and stays distinct from the black bar text. The lilac
            // `text_accent` is barely legible here.
            status_bar_accent: Color::Rgb(46, 46, 92),
        }
    }

    /// Look up a preset palette by name.
    ///
    /// Recognised names: `"basic"`, `"indexed"`, `"truecolor"`, `"monokai-dimmed"`,
    /// `"zedokai"`, `"rosé-pine"` / `"rose-pine"`, `"lcars"`.
    pub fn from_preset(name: &str) -> Option<Self> {
        match name.to_lowercase().as_str() {
            "basic" => Some(Self::basic()),
            "indexed" => Some(Self::indexed()),
            "truecolor" => Some(Self::truecolor()),
            "monokai-dimmed" | "monokai_dimmed" => Some(Self::monokai_dimmed()),
            "zedokai" => Some(Self::zedokai()),
            "rosé-pine" | "rose-pine" | "rosé_pine" | "rose_pine" => Some(Self::rose_pine()),
            "lcars" => Some(Self::lcars()),
            _ => None,
        }
    }

    /// Apply user-supplied overrides on top of this theme.
    ///
    /// Only `Some` fields in `overrides` replace the corresponding color;
    /// `None` fields leave the base theme value untouched.
    pub fn with_overrides(mut self, overrides: &ThemeOverrides) -> Self {
        macro_rules! apply {
            ($field:ident) => {
                if let Some(cv) = overrides.$field {
                    self.$field = cv.0;
                }
            };
        }

        apply!(border_focused);
        apply!(border_unfocused);
        apply!(selection_bg);
        apply!(status_creating);
        apply!(status_running);
        apply!(status_stopped);
        apply!(status_pr);
        apply!(status_pr_merged);
        apply!(pr_open);
        apply!(pr_draft);
        apply!(pr_closed);
        apply!(pr_pill_open_bg);
        apply!(pr_pill_draft_bg);
        apply!(pr_pill_closed_bg);
        apply!(pr_pill_review_bg);
        apply!(pr_pill_merged_bg);
        apply!(pr_pill_text);
        apply!(agent_waiting);
        apply!(unread_indicator);
        apply!(text_primary);
        apply!(text_secondary);
        apply!(text_accent);
        apply!(diff_added);
        apply!(diff_removed);
        apply!(diff_hunk_header);
        apply!(diff_file_header);
        apply!(diff_context);
        apply!(diff_expand_bg);
        apply!(diff_hunk_header_bg);
        apply!(modal_info);
        apply!(modal_warning);
        apply!(modal_error);
        apply!(palette_command_bg);
        apply!(palette_command_fg);
        apply!(status_bar_bg);
        apply!(status_bar_fg);
        apply!(status_bar_accent);

        // selection_fg is Option<Color> in Theme but Option<ColorValue> in overrides
        if let Some(cv) = overrides.selection_fg {
            self.selection_fg = Some(cv.0);
        }

        // agent_working uses AgentWorkingStyle, not ColorValue, so it's applied
        // directly without unwrapping a `.0`.
        if let Some(style) = overrides.agent_working {
            self.agent_working = style;
        }

        // Not a colour: the surface derived fills are blended against. Every
        // preset declares itself dark, so this is the only way a light-terminal
        // user gets fills scaled toward white — see `fill_color`.
        if let Some(appearance) = overrides.appearance {
            self.appearance = appearance.into();
        }

        // project_colors is intentionally not overridable — paired-tuple
        // arrays are ergonomically poor in TOML and the feature has minimal
        // user demand.

        self
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

    /// Get project colors by index (cycles through palette)
    pub fn project_color(&self, index: usize) -> (Color, Color) {
        self.project_colors[index % self.project_colors.len()]
    }

    /// Style for status bar
    pub fn status_bar(&self) -> Style {
        Style::default()
            .bg(self.status_bar_bg)
            .fg(self.status_bar_fg)
    }
}

/// Build a saturated line fill from a base colour for the review diff view.
///
/// Amplifies saturation first (so dimmed pastel theme colours read as rich
/// green/red rather than washed-out grey), then scales the result *toward the
/// surface* by `strength`: `0.0` is the surface itself, `1.0` the saturated
/// colour at full strength.
///
/// Which surface is the whole point. Scaling toward black is right on a dark
/// terminal and wrong on a light one, where it produces a near-black band under
/// dark text — legible only by accident. `appearance` picks the end to scale
/// toward, so the same `strength` means "a fifth of the way from the background
/// to this colour" on both.
pub fn fill_color(base: Color, strength: f32, appearance: Appearance) -> Color {
    const SAT: f32 = 1.75;
    let (r, g, b) = color_to_approx_rgb(base);
    let (r, g, b) = (r as f32, g as f32, b as f32);
    let mean = (r + g + b) / 3.0;
    let saturate = |c: f32| (mean + (c - mean) * SAT).clamp(0.0, 255.0);
    blend_to_surface(
        [saturate(r), saturate(g), saturate(b)],
        strength,
        appearance,
    )
}

/// Scale a colour *toward the surface* by `strength`, leaving its hue alone:
/// `0.0` is the surface itself, `1.0` the colour unchanged.
///
/// The muting half of [`fill_color`], for colours that are already the shade
/// they want to be — a selection band that must read as present but inactive,
/// say — and would only be muddied by that function's saturation boost.
pub fn toward_surface(color: Color, strength: f32, appearance: Appearance) -> Color {
    let (r, g, b) = color_to_approx_rgb(color);
    blend_to_surface([r as f32, g as f32, b as f32], strength, appearance)
}

/// Scale an RGB triple toward the terminal's surface by `strength`.
///
/// Which surface is the whole point. Scaling toward black is right on a dark
/// terminal and wrong on a light one, where it produces a near-black band under
/// dark text — legible only by accident. `appearance` picks the end to scale
/// toward, so the same `strength` means "a fifth of the way from the background
/// to this colour" on both.
fn blend_to_surface(rgb: [f32; 3], strength: f32, appearance: Appearance) -> Color {
    // Black behind light text, white behind dark text.
    let surface = match appearance {
        Appearance::Dark => 0.0,
        Appearance::Light => 255.0,
        // `Appearance` is `#[non_exhaustive]`; an unknown value keeps today's
        // behaviour rather than failing to build.
        _ => 0.0,
    };
    let ch = |c: f32| (surface + (c - surface) * strength) as u8;
    Color::Rgb(ch(rgb[0]), ch(rgb[1]), ch(rgb[2]))
}

/// Scale a color's brightness toward black by the given factor (0.0 = black, 1.0 = unchanged).
///
/// For named and indexed colors that can't be scaled directly, falls back to the
/// closest indexed gray from the 256-color palette.
pub fn dim_color(color: Color, opacity: f32) -> Color {
    let opacity = opacity.clamp(0.0, 1.0);
    match color {
        Color::Rgb(r, g, b) => Color::Rgb(
            (r as f32 * opacity) as u8,
            (g as f32 * opacity) as u8,
            (b as f32 * opacity) as u8,
        ),
        Color::Reset => {
            // Reset means "terminal default" — dim to a gray proportional to opacity
            // Assume default text is ~200 brightness
            let v = (200.0 * opacity) as u8;
            Color::Rgb(v, v, v)
        }
        other => {
            // Convert named/indexed colors to approximate RGB, then dim
            let (r, g, b) = color_to_approx_rgb(other);
            Color::Rgb(
                (r as f32 * opacity) as u8,
                (g as f32 * opacity) as u8,
                (b as f32 * opacity) as u8,
            )
        }
    }
}

/// Approximate RGB values for named ANSI colors
fn color_to_approx_rgb(color: Color) -> (u8, u8, u8) {
    match color {
        Color::Black => (0, 0, 0),
        Color::Red => (205, 0, 0),
        Color::Green => (0, 205, 0),
        Color::Yellow => (205, 205, 0),
        Color::Blue => (0, 0, 238),
        Color::Magenta => (205, 0, 205),
        Color::Cyan => (0, 205, 205),
        Color::White | Color::Gray => (229, 229, 229),
        Color::DarkGray => (127, 127, 127),
        Color::LightRed => (255, 0, 0),
        Color::LightGreen => (0, 255, 0),
        Color::LightYellow => (255, 255, 0),
        Color::LightBlue => (92, 92, 255),
        Color::LightMagenta => (255, 0, 255),
        Color::LightCyan => (0, 255, 255),
        Color::Indexed(n) => indexed_to_rgb(n),
        Color::Rgb(r, g, b) => (r, g, b),
        Color::Reset => (200, 200, 200),
    }
}

/// Convert a 256-color index to approximate RGB
fn indexed_to_rgb(n: u8) -> (u8, u8, u8) {
    match n {
        // Standard 16 colors — delegate to named
        0 => (0, 0, 0),
        1 => (205, 0, 0),
        2 => (0, 205, 0),
        3 => (205, 205, 0),
        4 => (0, 0, 238),
        5 => (205, 0, 205),
        6 => (0, 205, 205),
        7 => (229, 229, 229),
        8 => (127, 127, 127),
        9 => (255, 0, 0),
        10 => (0, 255, 0),
        11 => (255, 255, 0),
        12 => (92, 92, 255),
        13 => (255, 0, 255),
        14 => (0, 255, 255),
        15 => (255, 255, 255),
        // 6x6x6 color cube (indices 16-231)
        16..=231 => {
            let n = n - 16;
            let b = n % 6;
            let g = (n / 6) % 6;
            let r = n / 36;
            let to_val = |c: u8| if c == 0 { 0u8 } else { 55 + 40 * c };
            (to_val(r), to_val(g), to_val(b))
        }
        // Grayscale ramp (indices 232-255)
        232..=255 => {
            let v = 8 + 10 * (n - 232);
            (v, v, v)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use claude_commander_core::config::theme::{ColorValue, ThemeOverrides};

    /// Core types its `[theme]` config colours as `ratatui_core::style::Color`;
    /// this crate renders with `ratatui::style::Color`. ratatui 0.30 re-exports
    /// ratatui-core's `style` module verbatim, so those are ONE type, not two
    /// structurally-identical ones — and `with_overrides` below relies on that to
    /// assign a `ColorValue` straight into a `Theme` field.
    ///
    /// Assigning in both directions asserts it. If a future ratatui bump moves to
    /// a ratatui-core major that core's own dependency doesn't share, this fails
    /// to compile here — next to the explanation — instead of surfacing as a
    /// confusing mismatch deep inside `with_overrides`.
    #[test]
    fn core_config_color_is_the_same_type_ratatui_renders() {
        let rendered = Color::Rgb(1, 2, 3);
        let from_config = ColorValue::from(rendered);
        let back: Color = from_config.0;
        assert_eq!(back, rendered);
    }

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

    #[test]
    fn test_from_preset_valid() {
        assert_eq!(
            Theme::from_preset("basic").unwrap().border_focused,
            Color::Cyan
        );
        assert_eq!(
            Theme::from_preset("indexed").unwrap().border_focused,
            Color::Indexed(117)
        );
        assert_eq!(
            Theme::from_preset("TrueColor").unwrap().border_focused,
            Color::Rgb(137, 180, 250)
        );
    }

    #[test]
    fn test_from_preset_monokai_dimmed() {
        let theme = Theme::from_preset("monokai-dimmed").unwrap();
        assert_eq!(theme.border_focused, Color::Rgb(181, 165, 106));
        assert_eq!(theme.status_running, Color::Rgb(181, 206, 168));
        assert_eq!(theme.text_primary, Color::Rgb(212, 212, 212));
        assert_eq!(theme.project_colors.len(), 6);

        // Underscore variant
        assert!(Theme::from_preset("monokai_dimmed").is_some());
    }

    #[test]
    fn test_from_preset_zedokai() {
        let theme = Theme::from_preset("zedokai").unwrap();
        assert_eq!(theme.border_focused, Color::Rgb(249, 38, 114));
        assert_eq!(theme.status_running, Color::Rgb(166, 226, 46));
        assert_eq!(theme.text_primary, Color::Rgb(248, 248, 242));
        assert_eq!(theme.project_colors.len(), 6);
    }

    #[test]
    fn test_from_preset_rose_pine() {
        let theme = Theme::from_preset("rose-pine").unwrap();
        assert_eq!(theme.border_focused, Color::Rgb(235, 111, 146));
        assert_eq!(theme.status_running, Color::Rgb(156, 207, 216));
        assert_eq!(theme.text_primary, Color::Rgb(224, 222, 244));
        assert_eq!(theme.project_colors.len(), 6);

        // Alternate name variants all resolve
        assert!(Theme::from_preset("rosé-pine").is_some());
        assert!(Theme::from_preset("rose_pine").is_some());
        assert!(Theme::from_preset("rosé_pine").is_some());
    }

    #[test]
    fn test_from_preset_lcars() {
        let theme = Theme::from_preset("lcars").unwrap();
        assert_eq!(theme.border_focused, Color::Rgb(247, 160, 29)); // Amber
        assert_eq!(theme.status_running, Color::Rgb(247, 160, 29));
        assert_eq!(theme.text_primary, Color::Rgb(255, 204, 153)); // Tan
        assert_eq!(theme.text_accent, Color::Rgb(204, 153, 204)); // Lilac
        assert_eq!(theme.agent_waiting, Color::Rgb(204, 102, 102)); // Salmon
        assert_eq!(theme.project_colors.len(), 6);

        // The status bar is a solid amber chrome accent with black text, so it
        // does not double as the review diff view's bands the way every other
        // preset's dark bar does — those stay dark enough to read code on.
        assert_eq!(theme.status_bar_bg, Color::Rgb(247, 160, 29));
        assert_eq!(theme.status_bar_fg, Color::Rgb(0, 0, 0));
        assert_eq!(theme.diff_expand_bg, Color::Rgb(36, 29, 43));
        assert_eq!(theme.diff_hunk_header_bg, Color::Rgb(18, 15, 20));
    }

    /// The Settings ▸ Theme picker enumerates [`PRESET_NAMES`] and nothing else,
    /// so a preset missing from it is unreachable from the UI however well
    /// [`Theme::from_preset`] resolves it.
    #[test]
    fn lcars_appears_in_preset_names() {
        assert!(PRESET_NAMES.contains(&"lcars"));
        // Every listed name must resolve, bar the auto-detect placeholder.
        for name in PRESET_NAMES.iter().filter(|n| **n != "(auto)") {
            assert!(
                Theme::from_preset(name).is_some(),
                "preset \"{name}\" is offered in the picker but does not resolve"
            );
        }
    }

    /// LCARS is the one preset whose spinner is a solid colour: the deck paints
    /// its RUN blocks amber, and the shared rainbow's six pastels are exactly the
    /// hues this palette avoids. Users can still override it.
    #[test]
    fn lcars_working_spinner_is_solid_amber() {
        let theme = Theme::from_preset("lcars").unwrap();
        assert_eq!(
            theme.agent_working,
            AgentWorkingStyle::Solid(Color::Rgb(247, 160, 29))
        );
        // Every other preset keeps the cycling rainbow.
        assert_eq!(Theme::rose_pine().agent_working, AgentWorkingStyle::Rainbow);
    }

    /// The persisted `[theme]` contract and the new preset in one test: a
    /// `config.toml` may name `lcars` alongside every form [`ColorValue`] accepts,
    /// and all of them must still parse. `config.toml` is never rewritten, so each
    /// of these four spellings is permanently load-bearing.
    #[test]
    fn lcars_preset_resolves_and_config_forms_survive() {
        let overrides: ThemeOverrides = toml::from_str(
            r##"
                preset = "lcars"
                border_unfocused = "dark_gray"
                selection_bg = 117
                border_focused = "#89b4fa"
                text_primary = "reset"
            "##,
        )
        .unwrap();

        assert_eq!(overrides.preset.as_deref(), Some("lcars"));
        assert_eq!(overrides.border_unfocused.unwrap().0, Color::DarkGray);
        assert_eq!(overrides.selection_bg.unwrap().0, Color::Indexed(117));
        assert_eq!(
            overrides.border_focused.unwrap().0,
            Color::Rgb(137, 180, 250)
        );
        assert_eq!(overrides.text_primary.unwrap().0, Color::Reset);

        // The preset resolves and per-field overrides still layer on top of it.
        let theme = Theme::from_preset(overrides.preset.as_deref().unwrap())
            .expect("lcars is a recognised preset")
            .with_overrides(&overrides);
        assert_eq!(theme.border_focused, Color::Rgb(137, 180, 250));
        assert_eq!(theme.text_primary, Color::Reset);
        // A field the fixture leaves alone keeps its LCARS value.
        assert_eq!(theme.status_running, Color::Rgb(247, 160, 29));
    }

    /// WCAG relative luminance, for [`contrast_ratio`].
    fn relative_luminance(color: Color) -> f32 {
        let (r, g, b) = color_to_approx_rgb(color);
        let lin = |c: u8| {
            let c = c as f32 / 255.0;
            if c <= 0.03928 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b)
    }

    /// WCAG contrast ratio between two colours, 1.0 (identical) to 21.0.
    fn contrast_ratio(a: Color, b: Color) -> f32 {
        let (la, lb) = (relative_luminance(a), relative_luminance(b));
        let (hi, lo) = if la > lb { (la, lb) } else { (lb, la) };
        (hi + 0.05) / (lo + 0.05)
    }

    /// The hotkey letter in `[n]ew session` and the board's top-bar title are
    /// painted on the **status bar**, so their accent has to contrast with
    /// `status_bar_bg` — not with the canvas.
    ///
    /// Both sites used `text_accent`, a colour chosen to read on the canvas. That
    /// held only because every preset's bar happened to be dark too: `lcars` has a
    /// light amber bar, where lilac `text_accent` is barely legible, and `basic`
    /// painted a blue letter on its own blue bar at a ratio of 1.0 — invisible but
    /// for the bold. `status_bar_accent` exists so a preset states this explicitly.
    #[test]
    fn every_preset_status_bar_accent_is_legible_on_its_bar() {
        for name in PRESET_NAMES.iter().filter(|n| **n != "(auto)") {
            let theme = Theme::from_preset(name).unwrap();
            let ratio = contrast_ratio(theme.status_bar_accent, theme.status_bar_bg);
            assert!(
                ratio >= 4.5,
                "preset \"{name}\" paints its status-bar accent at {ratio:.2}:1 on \
                 its own bar; WCAG AA wants 4.5:1"
            );
            // The plain bar text has to be legible on it as well.
            let fg_ratio = contrast_ratio(theme.status_bar_fg, theme.status_bar_bg);
            assert!(
                fg_ratio >= 4.5,
                "preset \"{name}\" status_bar_fg is {fg_ratio:.2}:1 on its own bar"
            );
        }
    }

    /// The accent is a distinct field, but the presets that already read well keep
    /// exactly the colour they rendered before it existed — so adding it is a
    /// no-op for every theme but the two that were broken.
    #[test]
    fn status_bar_accent_preserves_the_presets_that_were_already_legible() {
        for name in [
            "indexed",
            "truecolor",
            "monokai-dimmed",
            "zedokai",
            "rose-pine",
        ] {
            let theme = Theme::from_preset(name).unwrap();
            assert_eq!(
                theme.status_bar_accent, theme.text_accent,
                "preset \"{name}\" rendered its hotkey letter in text_accent and must not shift"
            );
        }
        // The two that could not keep `text_accent` deliberately diverge.
        assert_ne!(
            Theme::lcars().status_bar_accent,
            Theme::lcars().text_accent,
            "lilac on the amber bar is the bug being fixed"
        );
        assert_ne!(
            Theme::basic().status_bar_accent,
            Theme::basic().status_bar_bg,
            "basic drew a blue letter on a blue bar"
        );
    }

    #[test]
    fn status_bar_accent_is_overridable() {
        let themed = Theme::lcars().with_overrides(&ThemeOverrides {
            status_bar_accent: Some(ColorValue(Color::Rgb(9, 9, 9))),
            ..Default::default()
        });
        assert_eq!(themed.status_bar_accent, Color::Rgb(9, 9, 9));
    }

    #[test]
    fn test_from_preset_unknown_returns_none() {
        assert!(Theme::from_preset("catppuccin").is_none());
    }

    #[test]
    fn test_with_overrides_applies_some_fields() {
        let base = Theme::basic();
        let overrides = ThemeOverrides {
            border_focused: Some(ColorValue(Color::Rgb(255, 0, 0))),
            status_running: Some(ColorValue(Color::Yellow)),
            ..Default::default()
        };
        let themed = base.with_overrides(&overrides);
        assert_eq!(themed.border_focused, Color::Rgb(255, 0, 0));
        assert_eq!(themed.status_running, Color::Yellow);
        // Untouched fields keep the base value
        assert_eq!(themed.border_unfocused, Color::DarkGray);
        assert_eq!(themed.status_stopped, Color::DarkGray);
    }

    #[test]
    fn test_with_overrides_empty_is_identity() {
        let base = Theme::indexed();
        let themed = base.clone().with_overrides(&ThemeOverrides::default());
        assert_eq!(themed.border_focused, base.border_focused);
        assert_eq!(themed.selection_bg, base.selection_bg);
        assert_eq!(themed.status_bar_bg, base.status_bar_bg);
    }

    #[test]
    fn test_with_overrides_selection_fg() {
        let base = Theme::basic();
        let overrides = ThemeOverrides {
            selection_fg: Some(ColorValue(Color::Rgb(1, 2, 3))),
            ..Default::default()
        };
        let themed = base.with_overrides(&overrides);
        assert_eq!(themed.selection_fg, Some(Color::Rgb(1, 2, 3)));
    }

    /// A fill has to be blended toward the surface it will sit on. Scaling a
    /// green toward black on a light terminal produced a near-black band under
    /// dark text — legible only by accident — which is the bug this flag fixes.
    #[test]
    fn fill_scales_toward_the_terminal_background() {
        let green = Color::Rgb(0, 200, 0);
        let Color::Rgb(_, dg, _) = fill_color(green, 0.26, Appearance::Dark) else {
            panic!("fill_color always returns Rgb");
        };
        let Color::Rgb(lr, lg, lb) = fill_color(green, 0.26, Appearance::Light) else {
            panic!("fill_color always returns Rgb");
        };
        // Dark: mostly black, a hint of the colour.
        assert!(dg < 100, "dark fill should sit near black, got {dg}");
        // Light: mostly white, a hint of the colour — and still recognisably
        // green, i.e. the green channel stays ahead of the other two.
        assert!(lr > 180 && lb > 180, "light fill should sit near white");
        assert!(
            lg > lr && lg > lb,
            "light green fill should still read green"
        );
    }

    /// The light-surface blend has to be reachable from config, not just from
    /// `fill_color`'s argument: every preset declares itself dark, so without
    /// this override no user on a light terminal ever gets it.
    #[test]
    fn light_appearance_override_makes_review_fills_blend_toward_white() {
        use claude_commander_core::config::theme::AppearanceValue;

        let overrides = ThemeOverrides {
            appearance: Some(AppearanceValue::Light),
            ..Default::default()
        };
        let light = Theme::truecolor().with_overrides(&overrides);
        assert_eq!(light.appearance, Appearance::Light);

        let Color::Rgb(r, g, b) = light.review_palette().add_bg else {
            panic!("true-colour add fill is Rgb");
        };
        assert!(
            r > 150 && g > 150 && b > 150,
            "light-terminal add fill should sit near white, got ({r}, {g}, {b})"
        );

        // And the default — no override — is still the dark blend, so nothing
        // changes for existing users.
        let dark = Theme::truecolor().with_overrides(&ThemeOverrides::default());
        assert_eq!(dark.appearance, Appearance::Dark);
        let Color::Rgb(dr, dg, db) = dark.review_palette().add_bg else {
            panic!("true-colour add fill is Rgb");
        };
        assert!(
            dr < 100 && dg < 150 && db < 100,
            "default add fill should stay near black, got ({dr}, {dg}, {db})"
        );
    }

    /// `0.0` is the surface itself and `1.0` the saturated colour, on both.
    #[test]
    fn fill_endpoints_are_the_surface_and_the_colour() {
        let red = Color::Rgb(200, 0, 0);
        assert_eq!(fill_color(red, 0.0, Appearance::Dark), Color::Rgb(0, 0, 0));
        assert_eq!(
            fill_color(red, 0.0, Appearance::Light),
            Color::Rgb(255, 255, 255)
        );
        assert_eq!(
            fill_color(red, 1.0, Appearance::Dark),
            fill_color(red, 1.0, Appearance::Light),
            "at full strength the surface no longer contributes"
        );
    }

    /// Muting keeps the hue and moves toward *the terminal's* surface — darker
    /// on a dark theme, lighter on a light one. Scaling toward black on both
    /// would put a near-black band under a light theme's dark text.
    #[test]
    fn muting_moves_toward_the_terminal_surface() {
        let selection = Color::Rgb(69, 71, 90);
        let Color::Rgb(dr, _, db) = toward_surface(selection, 0.7, Appearance::Dark) else {
            panic!("toward_surface always returns Rgb");
        };
        assert!(dr < 69 && db < 90, "dark: muted band must darken");
        let Color::Rgb(lr, _, lb) = toward_surface(selection, 0.7, Appearance::Light) else {
            panic!("toward_surface always returns Rgb");
        };
        assert!(lr > 69 && lb > 90, "light: muted band must lighten");
        assert_eq!(
            toward_surface(selection, 1.0, Appearance::Dark),
            selection,
            "full strength is the colour itself"
        );
    }

    /// The unfocused file-list highlight is the selection, muted — present (so
    /// the row being read stays identifiable) but not the focused row.
    #[test]
    fn unfocused_selection_is_a_muted_selection() {
        let theme = Theme::truecolor();
        let pal = theme.review_palette();
        assert_eq!(pal.selection_bg, theme.selection_bg);
        assert_ne!(pal.selection_bg_unfocused, pal.selection_bg);
        assert_ne!(
            pal.selection_bg_unfocused,
            Color::Reset,
            "the unfocused cursor row must still be banded"
        );
        assert_ne!(
            pal.selection_fg_unfocused, pal.selection_fg,
            "the foreground is muted too, not dropped or left at full strength"
        );
        assert!(pal.selection_fg_unfocused.is_some());
        // Below true-color the palette can't express a weaker shade, so it
        // keeps the full selection rather than emitting an RGB escape.
        let indexed = Theme::for_color_mode(ColorMode::Indexed).review_palette();
        assert_eq!(indexed.selection_bg_unfocused, indexed.selection_bg);
        assert_eq!(indexed.selection_fg_unfocused, indexed.selection_fg);
    }

    /// Every built-in preset must still show its unfocused cursor row: on a
    /// theme whose band is already near-black (LCARS: `Rgb(36, 24, 9)`) the
    /// muted background alone is not a signal, so the muted *foreground* has to
    /// stay well clear of the surface. Guards the whole preset list, since the
    /// failure is invisible on the themes that carry selection in the band.
    #[test]
    fn every_preset_keeps_a_visible_unfocused_selection() {
        for preset in [
            "truecolor",
            "monokai-dimmed",
            "zedokai",
            "rose-pine",
            "lcars",
        ] {
            let theme = Theme::from_preset(preset).expect("preset name from `from_preset`'s docs");
            let pal = theme.review_palette();
            let Some(Color::Rgb(r, g, b)) = pal.selection_fg_unfocused else {
                panic!("{preset}: truecolor presets all set an RGB selection foreground");
            };
            let level = r.max(g).max(b);
            assert!(
                level > 96,
                "{preset}: unfocused row text ({r}, {g}, {b}) is too close to the \
                 surface to read as selected"
            );
        }
    }

    /// The role → colour mapping the review view renders every row through.
    #[test]
    fn review_palette_maps_roles_to_theme_colours() {
        use diffgrid::LineOrigin;
        use diffgrid::style::SpanStyle;

        let pal = Theme::truecolor().review_palette();
        assert_eq!(pal.role(Role::Addition).bg, pal.add_bg);
        assert_eq!(pal.role(Role::Deletion).bg, pal.del_bg);
        assert_eq!(pal.role(Role::Context).bg, Color::Reset);
        assert_eq!(
            pal.role(Role::Gutter(LineOrigin::Addition)).bg,
            pal.add_gutter_bg
        );
        assert_eq!(pal.role(Role::HunkHeader).fg, pal.hunk_header);
        assert_eq!(pal.role(Role::ExpandedContext).bg, pal.context_bg);
        assert_eq!(pal.emphasis_bg(Role::Deletion), pal.del_emph_bg);
        // Word-diff emphasis strengthens the row's own fill...
        let emph = pal.ink(&SpanStyle::new(Role::Addition).with_emphasis(true));
        assert_eq!(emph.bg, pal.add_emph_bg);
        // ...and selection beats it outright, taking the theme's selection
        // foreground with it so a cursor row reads as selected.
        let selected = pal.ink(
            &SpanStyle::new(Role::Addition)
                .with_emphasis(true)
                .with_selected(true),
        );
        assert_eq!(selected.bg, pal.selection_bg);
        assert_eq!(selected.fg, pal.selection_fg.unwrap_or(pal.text));
    }

    /// A clickable expand segment is accented; the "n hidden lines" hint is not.
    #[test]
    fn review_palette_accents_actionable_expand_segments() {
        use diffgrid::style::SpanStyle;

        let pal = Theme::truecolor().review_palette();
        let hint = pal.ink(&SpanStyle::new(Role::ExpandControl));
        let action = pal.ink(&SpanStyle::new(Role::ExpandControl).with_bold(true));
        assert_eq!(hint.fg, pal.gutter_fg);
        assert_eq!(action.fg, pal.hunk_header);
        assert!(action.bold);
        assert_eq!(action.bg, pal.context_bg);
    }

    /// Below truecolor the view passes no highlighter, and the palette must not
    /// answer with an RGB escape a 16-colour terminal cannot honour.
    #[test]
    fn review_palette_degrades_syntax_colour_below_truecolor() {
        let rgb = Rgb::new(10, 20, 30);
        assert_eq!(
            Theme::truecolor().review_palette().syntax(rgb),
            Color::Rgb(10, 20, 30)
        );
        let basic = Theme::basic().review_palette();
        assert_eq!(basic.syntax(rgb), basic.text);
    }

    #[test]
    fn test_dim_color_rgb() {
        // 50% opacity halves each channel
        assert_eq!(
            dim_color(Color::Rgb(200, 100, 50), 0.5),
            Color::Rgb(100, 50, 25)
        );
    }

    #[test]
    fn test_dim_color_full_opacity_unchanged() {
        assert_eq!(
            dim_color(Color::Rgb(200, 100, 50), 1.0),
            Color::Rgb(200, 100, 50)
        );
    }

    #[test]
    fn test_dim_color_zero_opacity_is_black() {
        assert_eq!(
            dim_color(Color::Rgb(200, 100, 50), 0.0),
            Color::Rgb(0, 0, 0)
        );
    }

    #[test]
    fn test_dim_color_named_converts_to_rgb() {
        // Green at 50% should be approximately half brightness
        let dimmed = dim_color(Color::Green, 0.5);
        assert!(matches!(dimmed, Color::Rgb(_, _, _)));
    }

    #[test]
    fn test_dim_color_indexed_converts_to_rgb() {
        let dimmed = dim_color(Color::Indexed(196), 0.5);
        assert!(matches!(dimmed, Color::Rgb(_, _, _)));
    }

    #[test]
    fn test_dim_color_reset_produces_gray() {
        let dimmed = dim_color(Color::Reset, 0.5);
        assert_eq!(dimmed, Color::Rgb(100, 100, 100));
    }

    #[test]
    fn test_dim_color_clamps_opacity() {
        // Opacity > 1.0 should be clamped to 1.0
        assert_eq!(
            dim_color(Color::Rgb(200, 100, 50), 2.0),
            Color::Rgb(200, 100, 50)
        );
        // Opacity < 0.0 should be clamped to 0.0
        assert_eq!(
            dim_color(Color::Rgb(200, 100, 50), -1.0),
            Color::Rgb(0, 0, 0)
        );
    }

    #[test]
    fn test_indexed_to_rgb_grayscale_ramp() {
        // Index 232 = darkest gray (8)
        assert_eq!(indexed_to_rgb(232), (8, 8, 8));
        // Index 255 = lightest gray (238)
        assert_eq!(indexed_to_rgb(255), (238, 238, 238));
    }

    #[test]
    fn test_indexed_to_rgb_color_cube() {
        // Index 16 = (0,0,0) in the 6x6x6 cube
        assert_eq!(indexed_to_rgb(16), (0, 0, 0));
        // Index 196 = (5,0,0) = bright red
        assert_eq!(indexed_to_rgb(196), (255, 0, 0));
    }

    /// The capability-tier presets must take their status-bar colours from
    /// [`ColorMode::status_bar_colors`], which is what core formats into the
    /// tmux `status-style` string. Re-hardcoding a pair here would silently
    /// desync the TUI's status bar from the tmux one, so assert they agree.
    #[test]
    fn test_tier_presets_take_status_bar_colors_from_color_mode() {
        for (theme, mode) in [
            (Theme::basic(), ColorMode::Basic),
            (Theme::indexed(), ColorMode::Indexed),
            (Theme::truecolor(), ColorMode::TrueColor),
        ] {
            let (bg, fg) = mode.status_bar_colors();
            assert_eq!(theme.status_bar_bg, bg, "status_bar_bg for {mode:?}");
            assert_eq!(theme.status_bar_fg, fg, "status_bar_fg for {mode:?}");
        }
    }

    /// LCARS' amber status bar, pinned as the tmux `status-style` string it would
    /// produce. Carried over from `test_tmux_status_style_per_theme`, which called
    /// the `Theme::tmux_status_style()` this branch removed (core now formats the
    /// string from [`ColorMode::status_bar_colors`]); the tier-tuple values it
    /// also asserted are covered byte-for-byte in `term_caps`.
    ///
    /// NB the comment beside `lcars()` says this "reaches attached sessions" — it
    /// does not, and did not before this branch either. The only production caller
    /// resolves the style from the *auto-detected* `ColorMode`, which never yields
    /// a named preset, so a user on LCARS still gets their tier's bar in tmux.
    /// Kept as a pin on the intended value, and as a marker for that gap.
    #[test]
    fn test_lcars_status_bar_would_render_as_amber_in_tmux() {
        use claude_commander_core::term_caps::color_to_tmux;
        let lcars = Theme::lcars();
        assert_eq!(
            format!(
                "bg={},fg={}",
                color_to_tmux(lcars.status_bar_bg),
                color_to_tmux(lcars.status_bar_fg)
            ),
            "bg=#f7a01d,fg=#000000"
        );
    }
}
