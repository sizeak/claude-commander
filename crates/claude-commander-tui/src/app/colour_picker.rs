//! The colour picker behind every Settings → Theme colour row: a grid of the
//! edited theme's colours (cell 0 clears the row's own value, so it inherits
//! again) and a hex row that takes typed or pasted `#rrggbb`.
//!
//! This module holds the picker's state, its key/paste handling (pure, so they
//! can be tested without an `App`) and how it draws. What a pick *means* is the
//! caller's.

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use tui_input::Input;

use crate::theme::{Theme, ThemeSwatch};

/// The most cells a grid row holds. A narrower pane gets fewer (see
/// [`ColourPicker::fit_to_width`]).
pub(crate) const MAX_GRID_COLUMNS: usize = 8;

/// Terminal columns one grid cell takes: `[■■]`.
pub(crate) const CELL_WIDTH: u16 = 4;

/// Which part of the picker has the keyboard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColourPickerFocus {
    Grid,
    Hex,
}

/// The open picker. `selected` indexes the grid: 0 is the "no value" cell
/// (named by `none_label`), `i` is `swatches[i - 1]`.
#[derive(Debug, Clone)]
pub struct ColourPicker {
    pub swatches: Vec<ThemeSwatch>,
    pub selected: usize,
    pub focus: ColourPickerFocus,
    pub hex: Input,
    /// Why the last Enter on the hex row was refused.
    pub error: Option<String>,
    /// Cells per grid row. Fitted to the pane when the picker opens, on every
    /// resize and on every frame ([`Self::fit_to_width`]), and `j`/`k` step by
    /// it, so navigation always moves by the row width the user is looking at.
    pub columns: usize,
    /// What cell 0 means here, e.g. "Inherit (usual)".
    pub none_label: String,
}

/// What a key did to the picker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PickerOutcome {
    /// Still open.
    Open,
    /// Close without saving.
    Cancel,
    /// Save this colour (`None` clears the row's own value).
    Pick(Option<String>),
}

/// Typed or pasted hex, normalised to lower-case `#rrggbb`: surrounding
/// whitespace is ignored and the `#` is optional.
pub(crate) fn normalize_hex(raw: &str) -> Result<String, String> {
    let trimmed = raw.trim();
    let digits = trimmed.strip_prefix('#').unwrap_or(trimmed);
    if digits.len() == 6 && digits.chars().all(|c| c.is_ascii_hexdigit()) {
        Ok(format!("#{}", digits.to_ascii_lowercase()))
    } else {
        Err(format!("\"{trimmed}\" is not a #rrggbb colour"))
    }
}

impl ColourPicker {
    /// Open on `current` (the row's own saved colour, if it has one): the
    /// matching swatch is preselected; a colour the theme doesn't have goes in
    /// the hex row, which then has focus — so Enter keeps it rather than
    /// picking the grid's `none_label` cell.
    pub fn open(theme: &Theme, current: Option<&str>, none_label: impl Into<String>) -> Self {
        let swatches = theme.swatches();
        let current = current.and_then(|c| normalize_hex(c).ok());
        let mut picker = Self {
            selected: 0,
            focus: ColourPickerFocus::Grid,
            hex: Input::default(),
            error: None,
            swatches,
            columns: MAX_GRID_COLUMNS,
            none_label: none_label.into(),
        };
        if let Some(current) = current {
            match picker.swatches.iter().position(|s| s.hex == current) {
                Some(i) => picker.selected = i + 1,
                None => {
                    picker.hex = current.into();
                    picker.focus = ColourPickerFocus::Hex;
                }
            }
        }
        picker
    }

    /// Fit the grid to a pane `width` columns wide: as many whole cells as
    /// fit, at least one and at most [`MAX_GRID_COLUMNS`].
    pub fn fit_to_width(&mut self, width: u16) {
        self.columns = usize::from(width / CELL_WIDTH).clamp(1, MAX_GRID_COLUMNS);
    }

    /// Number of grid cells, the `none_label` cell included.
    pub fn cells(&self) -> usize {
        self.swatches.len() + 1
    }

    /// The highlighted swatch (`None` on the `none_label` cell).
    pub fn selected_swatch(&self) -> Option<&ThemeSwatch> {
        self.selected
            .checked_sub(1)
            .and_then(|i| self.swatches.get(i))
    }

    /// The hex row's value, if it is a valid colour.
    pub fn hex_value(&self) -> Option<String> {
        normalize_hex(self.hex.value()).ok()
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> PickerOutcome {
        match self.focus {
            ColourPickerFocus::Grid => self.grid_key(key),
            ColourPickerFocus::Hex => self.hex_key(key),
        }
    }

    fn grid_key(&mut self, key: KeyEvent) -> PickerOutcome {
        let last = self.cells() - 1;
        let columns = self.columns.max(1);
        match key.code {
            KeyCode::Esc => return PickerOutcome::Cancel,
            KeyCode::Enter => {
                return PickerOutcome::Pick(self.selected_swatch().map(|s| s.hex.clone()));
            }
            KeyCode::Left | KeyCode::Char('h') => self.selected = self.selected.saturating_sub(1),
            KeyCode::Right | KeyCode::Char('l') => self.selected = (self.selected + 1).min(last),
            KeyCode::Up | KeyCode::Char('k') => {
                if self.selected >= columns {
                    self.selected -= columns;
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                // Onto a short last row, land on its last cell.
                if self.selected / columns < last / columns {
                    self.selected = (self.selected + columns).min(last);
                }
            }
            KeyCode::Tab | KeyCode::BackTab => self.focus = ColourPickerFocus::Hex,
            KeyCode::Char('#') => {
                self.focus = ColourPickerFocus::Hex;
                self.hex = "#".into();
                self.error = None;
            }
            _ => {}
        }
        PickerOutcome::Open
    }

    fn hex_key(&mut self, key: KeyEvent) -> PickerOutcome {
        match key.code {
            KeyCode::Esc => PickerOutcome::Cancel,
            KeyCode::Tab | KeyCode::BackTab => {
                self.focus = ColourPickerFocus::Grid;
                PickerOutcome::Open
            }
            KeyCode::Enter => match normalize_hex(self.hex.value()) {
                Ok(hex) => PickerOutcome::Pick(Some(hex)),
                Err(e) => {
                    self.error = Some(if self.hex.value().trim().is_empty() {
                        "Type a hex colour, or Tab back to the swatches".to_string()
                    } else {
                        e
                    });
                    PickerOutcome::Open
                }
            },
            _ => {
                if super::edit_text_input(&mut self.hex, key) {
                    self.error = None;
                }
                PickerOutcome::Open
            }
        }
    }

    /// The footer hint for the picker's current focus.
    pub fn footer_hint(&self) -> &'static str {
        match self.focus {
            ColourPickerFocus::Grid => "←↓↑→/hjkl: move  Enter: pick  Tab/#: hex  Esc: cancel",
            ColourPickerFocus::Hex => {
                "Enter: save  type or paste #rrggbb  Tab: swatches  Esc: cancel"
            }
        }
    }

    /// The picker as lines: the swatch grid (cell 0 is `none_label`), the
    /// highlighted swatch's role and hex, then the hex row with a live
    /// preview. `columns` must already be fitted to the pane it is drawn in.
    pub fn lines(&self, theme: &Theme) -> Vec<Line<'static>> {
        let grid_focused = self.focus == ColourPickerFocus::Grid;
        let secondary = Style::default().fg(theme.text_secondary);
        let bracket = if grid_focused {
            Style::default().fg(theme.text_primary)
        } else {
            secondary
        };
        let mut lines = Vec::new();
        let cells: Vec<Option<&ThemeSwatch>> = std::iter::once(None)
            .chain(self.swatches.iter().map(Some))
            .collect();
        let columns = self.columns.max(1);
        for (row, chunk) in cells.chunks(columns).enumerate() {
            let mut spans = Vec::with_capacity(chunk.len() * 3);
            for (col, cell) in chunk.iter().enumerate() {
                let selected = row * columns + col == self.selected;
                let (open, close) = if selected { ("[", "]") } else { (" ", " ") };
                spans.push(Span::styled(open, bracket));
                spans.push(match cell {
                    Some(s) => Span::styled("■■", Style::default().fg(s.color)),
                    None => Span::styled("··", secondary),
                });
                spans.push(Span::styled(close, bracket));
            }
            lines.push(Line::from(spans));
        }
        lines.push(Line::from(match self.selected_swatch() {
            Some(s) => vec![
                Span::styled(
                    format!(" {}", s.role),
                    Style::default().fg(theme.text_primary),
                ),
                Span::styled(format!("  {}", s.hex), secondary),
            ],
            None => vec![Span::styled(format!(" {}", self.none_label), secondary)],
        }));
        lines.push(Line::from(""));

        let label_style = if grid_focused {
            Style::default()
        } else {
            theme.selection()
        };
        let mut hex_spans = vec![Span::styled(format!("{:<10}", "Hex"), label_style)];
        let preview = self
            .hex_value()
            .and_then(|h| crate::widgets::parse_hex_color(&h));
        hex_spans.push(match preview {
            Some(c) => Span::styled("■■ ", Style::default().fg(c)),
            None => Span::raw("   "),
        });
        let raw = self.hex.value();
        let value_style = if !raw.trim().is_empty() && preview.is_none() {
            Style::default().fg(theme.diff_removed)
        } else {
            Style::default().fg(theme.text_accent)
        };
        if grid_focused && raw.is_empty() {
            hex_spans.push(Span::styled(
                "#rrggbb (Tab or # to type or paste)",
                secondary,
            ));
        } else if grid_focused {
            hex_spans.push(Span::styled(raw.to_string(), value_style));
        } else {
            hex_spans.push(Span::styled(
                super::input_with_caret(&self.hex),
                value_style,
            ));
        }
        lines.push(Line::from(hex_spans));
        if let Some(err) = &self.error {
            lines.push(Line::from(Span::styled(
                format!(" {err}"),
                Style::default().fg(theme.modal_error),
            )));
        }
        lines
    }

    /// A bracketed paste: it always replaces the hex row and focuses it,
    /// wherever the focus was — the paste *is* the colour (as in the Flutter
    /// client's picker).
    pub fn paste(&mut self, text: &str) {
        let clean: String = text.chars().filter(|c| !c.is_control()).collect();
        self.hex = clean.trim().into();
        self.focus = ColourPickerFocus::Hex;
        self.error = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyModifiers;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn picker(current: Option<&str>) -> ColourPicker {
        ColourPicker::open(&Theme::truecolor(), current, "Inherit (usual)")
    }

    fn press(p: &mut ColourPicker, codes: &[KeyCode]) -> PickerOutcome {
        let mut out = PickerOutcome::Open;
        for c in codes {
            out = p.handle_key(key(*c));
        }
        out
    }

    fn type_str(p: &mut ColourPicker, s: &str) {
        for c in s.chars() {
            p.handle_key(key(KeyCode::Char(c)));
        }
    }

    #[test]
    fn normalize_hex_accepts_an_optional_hash_and_whitespace() {
        assert_eq!(normalize_hex("#AABBCC").unwrap(), "#aabbcc");
        assert_eq!(normalize_hex("aabbcc").unwrap(), "#aabbcc");
        assert_eq!(normalize_hex("  aabbcc \t").unwrap(), "#aabbcc");
        assert!(normalize_hex("").is_err());
        assert!(normalize_hex("#abc").is_err());
        assert!(normalize_hex("##aabbcc").is_err());
        assert!(normalize_hex("gg0000").is_err());
    }

    #[test]
    fn opening_preselects_the_matching_swatch() {
        let p = picker(Some("#B4BEFE")); // truecolor's accent
        assert_eq!(p.selected, 1);
        assert_eq!(p.selected_swatch().unwrap().role, "accent");
        assert_eq!(p.hex.value(), "");
        assert_eq!(p.focus, ColourPickerFocus::Grid);
    }

    #[test]
    fn opening_on_an_off_theme_colour_prefills_and_focuses_the_hex_row() {
        let mut p = picker(Some("#123456"));
        assert_eq!(p.selected, 0);
        assert_eq!(p.hex.value(), "#123456");
        assert_eq!(
            p.focus,
            ColourPickerFocus::Hex,
            "Enter must keep the colour, not pick the grid's No colour"
        );
        assert_eq!(
            press(&mut p, &[KeyCode::Enter]),
            PickerOutcome::Pick(Some("#123456".into()))
        );
    }

    #[test]
    fn opening_with_no_colour_selects_no_colour() {
        let p = picker(None);
        assert_eq!(p.selected, 0);
        assert!(p.selected_swatch().is_none());
    }

    #[test]
    fn arrows_and_hjkl_move_around_the_grid() {
        let mut p = picker(None);
        assert!(
            p.cells() > MAX_GRID_COLUMNS * 2,
            "the test needs three rows"
        );
        press(&mut p, &[KeyCode::Right, KeyCode::Char('l')]);
        assert_eq!(p.selected, 2);
        press(&mut p, &[KeyCode::Down]);
        assert_eq!(p.selected, 2 + MAX_GRID_COLUMNS);
        press(&mut p, &[KeyCode::Char('j')]);
        assert_eq!(p.selected, 2 + 2 * MAX_GRID_COLUMNS);
        press(&mut p, &[KeyCode::Char('k'), KeyCode::Up]);
        assert_eq!(p.selected, 2);
        press(&mut p, &[KeyCode::Up]);
        assert_eq!(p.selected, 2, "no row above the first");
        press(&mut p, &[KeyCode::Char('h'), KeyCode::Left, KeyCode::Left]);
        assert_eq!(p.selected, 0, "clamped at No colour");
    }

    #[test]
    fn the_grid_fits_its_width_and_navigation_follows() {
        let mut p = picker(None);
        p.fit_to_width(26); // a pane 26 columns wide
        assert_eq!(p.columns, 6);
        press(&mut p, &[KeyCode::Down]);
        assert_eq!(p.selected, 6, "j steps by the drawn row width");
        press(&mut p, &[KeyCode::Up]);
        assert_eq!(p.selected, 0);
        p.fit_to_width(200);
        assert_eq!(p.columns, MAX_GRID_COLUMNS, "capped");
        p.fit_to_width(2);
        assert_eq!(p.columns, 1, "at least one");
        press(&mut p, &[KeyCode::Down]);
        assert_eq!(p.selected, 1);
    }

    #[test]
    fn down_onto_a_short_last_row_lands_on_its_last_cell() {
        let mut p = picker(None);
        let last = p.cells() - 1;
        p.selected = (last / MAX_GRID_COLUMNS) * MAX_GRID_COLUMNS - 1; // end of the row above
        press(&mut p, &[KeyCode::Down]);
        assert_eq!(p.selected, last);
        press(&mut p, &[KeyCode::Down, KeyCode::Right]);
        assert_eq!(p.selected, last, "never past the end");
    }

    #[test]
    fn enter_picks_the_highlighted_swatch_and_no_colour_clears() {
        let mut p = picker(None);
        assert_eq!(
            press(&mut p, &[KeyCode::Right, KeyCode::Enter]),
            PickerOutcome::Pick(Some("#b4befe".into()))
        );
        let mut p = picker(Some("#b4befe"));
        assert_eq!(
            press(&mut p, &[KeyCode::Left, KeyCode::Enter]),
            PickerOutcome::Pick(None)
        );
    }

    #[test]
    fn esc_cancels_from_either_focus() {
        let mut p = picker(None);
        assert_eq!(press(&mut p, &[KeyCode::Esc]), PickerOutcome::Cancel);
        let mut p = picker(None);
        assert_eq!(
            press(&mut p, &[KeyCode::Tab, KeyCode::Esc]),
            PickerOutcome::Cancel
        );
    }

    #[test]
    fn hash_opens_the_hex_row_and_typing_picks() {
        let mut p = picker(None);
        press(&mut p, &[KeyCode::Char('#')]);
        assert_eq!(p.focus, ColourPickerFocus::Hex);
        type_str(&mut p, "A1B2C3");
        assert_eq!(p.hex.value(), "#A1B2C3");
        assert_eq!(p.hex_value().as_deref(), Some("#a1b2c3"), "live preview");
        // h/j/k/l are text here, not navigation.
        let mut q = picker(None);
        press(&mut q, &[KeyCode::Tab]);
        type_str(&mut q, "hjkl");
        assert_eq!(q.hex.value(), "hjkl");
        assert_eq!(q.selected, 0);
        assert_eq!(
            press(&mut p, &[KeyCode::Enter]),
            PickerOutcome::Pick(Some("#a1b2c3".into()))
        );
    }

    #[test]
    fn tab_moves_between_grid_and_hex_row() {
        let mut p = picker(None);
        press(&mut p, &[KeyCode::Tab]);
        assert_eq!(p.focus, ColourPickerFocus::Hex);
        press(&mut p, &[KeyCode::Tab]);
        assert_eq!(p.focus, ColourPickerFocus::Grid);
    }

    #[test]
    fn invalid_hex_is_refused_with_a_message() {
        let mut p = picker(None);
        press(&mut p, &[KeyCode::Tab]);
        type_str(&mut p, "#12zz");
        assert!(p.hex_value().is_none());
        assert_eq!(press(&mut p, &[KeyCode::Enter]), PickerOutcome::Open);
        assert!(
            p.error.as_deref().unwrap().contains("#12zz"),
            "{:?}",
            p.error
        );
        // Editing clears the message.
        press(&mut p, &[KeyCode::Backspace]);
        assert!(p.error.is_none());
        // An empty row is refused too, not read as "clear".
        let mut q = picker(None);
        press(&mut q, &[KeyCode::Tab]);
        assert_eq!(press(&mut q, &[KeyCode::Enter]), PickerOutcome::Open);
        assert!(q.error.is_some());
    }

    #[test]
    fn paste_over_the_grid_replaces_the_hex_row() {
        let mut p = picker(Some("#123456"));
        p.paste("  3366FF\n");
        assert_eq!(p.focus, ColourPickerFocus::Hex);
        assert_eq!(p.hex.value(), "3366FF");
        assert_eq!(
            press(&mut p, &[KeyCode::Enter]),
            PickerOutcome::Pick(Some("#3366ff".into()))
        );
    }

    #[test]
    fn paste_in_the_hex_row_replaces_it_too() {
        let mut p = picker(None);
        press(&mut p, &[KeyCode::Char('#')]);
        type_str(&mut p, "12");
        p.paste(" aabbcc ");
        assert_eq!(p.hex.value(), "aabbcc", "the paste *is* the colour");
        assert_eq!(p.focus, ColourPickerFocus::Hex);
    }
}
