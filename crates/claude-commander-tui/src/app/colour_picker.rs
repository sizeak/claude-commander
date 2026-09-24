//! The workspace colour picker in Settings → Workspaces: a grid of the active
//! theme's colours (cell 0 is "No colour") and a hex row that takes typed or
//! pasted `#rrggbb`.
//!
//! This module holds the picker's state and its key/paste handling, which are
//! pure so they can be tested without an `App`. What a pick *means* — saving
//! through `set_workspace_color` — is the caller's.

use crossterm::event::{KeyCode, KeyEvent};
use tui_input::Input;

use crate::theme::{Theme, ThemeSwatch};
use claude_commander_protocol::workspace::{WorkspaceRejection, validate_workspace_color};

/// Cells per grid row. Fixed rather than width-derived so `j`/`k` move by
/// the same step the grid is drawn with.
pub(crate) const GRID_COLUMNS: usize = 8;

/// Which part of the picker has the keyboard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColourPickerFocus {
    Grid,
    Hex,
}

/// The open picker. `selected` indexes the grid: 0 is "No colour", `i` is
/// `swatches[i - 1]`.
#[derive(Debug, Clone)]
pub struct ColourPicker {
    pub swatches: Vec<ThemeSwatch>,
    pub selected: usize,
    pub focus: ColourPickerFocus,
    pub hex: Input,
    /// Why the last Enter on the hex row was refused.
    pub error: Option<String>,
}

/// What a key did to the picker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PickerOutcome {
    /// Still open.
    Open,
    /// Close without saving.
    Cancel,
    /// Save this colour (`None` clears it).
    Pick(Option<String>),
}

/// Typed or pasted hex, normalised for the protocol's rule: surrounding
/// whitespace is ignored and the `#` is optional. The rule itself is
/// [`validate_workspace_color`]'s.
pub(crate) fn normalize_hex(raw: &str) -> Result<String, WorkspaceRejection> {
    let trimmed = raw.trim();
    if trimmed.starts_with('#') {
        validate_workspace_color(trimmed)
    } else {
        validate_workspace_color(&format!("#{trimmed}"))
    }
}

impl ColourPicker {
    /// Open on `current` (the workspace's saved colour): the matching swatch
    /// is preselected; a colour the theme doesn't have goes in the hex row.
    pub fn open(theme: &Theme, current: Option<&str>) -> Self {
        let swatches = theme.swatches();
        let current = current.and_then(|c| normalize_hex(c).ok());
        let mut picker = Self {
            selected: 0,
            focus: ColourPickerFocus::Grid,
            hex: Input::default(),
            error: None,
            swatches,
        };
        if let Some(current) = current {
            match picker.swatches.iter().position(|s| s.hex == current) {
                Some(i) => picker.selected = i + 1,
                None => picker.hex = current.into(),
            }
        }
        picker
    }

    /// Number of grid cells, "No colour" included.
    pub fn cells(&self) -> usize {
        self.swatches.len() + 1
    }

    /// The highlighted swatch (`None` on "No colour").
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
        match key.code {
            KeyCode::Esc => return PickerOutcome::Cancel,
            KeyCode::Enter => {
                return PickerOutcome::Pick(self.selected_swatch().map(|s| s.hex.clone()));
            }
            KeyCode::Left | KeyCode::Char('h') => self.selected = self.selected.saturating_sub(1),
            KeyCode::Right | KeyCode::Char('l') => self.selected = (self.selected + 1).min(last),
            KeyCode::Up | KeyCode::Char('k') => {
                if self.selected >= GRID_COLUMNS {
                    self.selected -= GRID_COLUMNS;
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                // Onto a short last row, land on its last cell.
                if self.selected / GRID_COLUMNS < last / GRID_COLUMNS {
                    self.selected = (self.selected + GRID_COLUMNS).min(last);
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
                        e.to_string()
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

    /// A bracketed paste: it always lands in the hex row. Pasted over the
    /// grid, it replaces the row (the paste *is* the colour); in the row, it
    /// goes in at the caret like typing.
    pub fn paste(&mut self, text: &str) {
        let clean: String = text.chars().filter(|c| !c.is_control()).collect();
        if self.focus == ColourPickerFocus::Grid {
            self.hex = Input::default();
            self.focus = ColourPickerFocus::Hex;
        }
        super::insert_into_input(&mut self.hex, clean.trim());
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
        ColourPicker::open(&Theme::truecolor(), current)
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
    fn opening_on_an_off_theme_colour_prefills_the_hex_row() {
        let p = picker(Some("#123456"));
        assert_eq!(p.selected, 0);
        assert_eq!(p.hex.value(), "#123456");
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
        assert!(p.cells() > GRID_COLUMNS * 2, "the test needs three rows");
        press(&mut p, &[KeyCode::Right, KeyCode::Char('l')]);
        assert_eq!(p.selected, 2);
        press(&mut p, &[KeyCode::Down]);
        assert_eq!(p.selected, 2 + GRID_COLUMNS);
        press(&mut p, &[KeyCode::Char('j')]);
        assert_eq!(p.selected, 2 + 2 * GRID_COLUMNS);
        press(&mut p, &[KeyCode::Char('k'), KeyCode::Up]);
        assert_eq!(p.selected, 2);
        press(&mut p, &[KeyCode::Up]);
        assert_eq!(p.selected, 2, "no row above the first");
        press(&mut p, &[KeyCode::Char('h'), KeyCode::Left, KeyCode::Left]);
        assert_eq!(p.selected, 0, "clamped at No colour");
    }

    #[test]
    fn down_onto_a_short_last_row_lands_on_its_last_cell() {
        let mut p = picker(None);
        let last = p.cells() - 1;
        p.selected = (last / GRID_COLUMNS) * GRID_COLUMNS - 1; // end of the row above
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
    fn paste_in_the_hex_row_inserts_at_the_caret() {
        let mut p = picker(None);
        press(&mut p, &[KeyCode::Char('#')]);
        p.paste("aabbcc");
        assert_eq!(p.hex.value(), "#aabbcc");
    }
}
