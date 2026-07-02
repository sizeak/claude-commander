//! Bracketed, clickable hotkey labels (dmux.ai-style).
//!
//! Surfaces an action's hotkey by bracketing the key letter inside its label
//! (`[n]ew session`), or appending the key when its letter isn't present in the
//! label (`view [Tab]`). The pure segmentation logic ([`segment_label`]) is kept
//! independent of ratatui styling so it can be unit-tested without a backend;
//! [`hotkey_spans`] turns a segmented label into styled spans at render time.
//!
//! Rendered labels can be clicked: [`ActionButton`] records the screen region a
//! button occupies alongside the action it dispatches, and [`button_at`] maps a
//! click back to that action.

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::Span;

use crate::config::keybindings::{BindableAction, KeyBinding, KeyBindings};
use crossterm::event::{KeyCode, KeyModifiers};

/// A label segmented for hotkey display.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HotkeyLabel {
    /// The key char occurs in the label: bracket the first case-insensitive
    /// match in place, rendered as `prefix` + `[` + `hit` + `]` + `suffix`.
    Inline {
        prefix: String,
        hit: char,
        suffix: String,
    },
    /// The key char is absent (or the key is non-char / modified): append the
    /// key's display form in brackets, rendered as `label` + ` [` + `key` + `]`.
    Append { label: String, key: String },
    /// The action has no bound key: render the bare label, unbracketed.
    Plain { label: String },
}

/// The key used to derive the bracketed hint: the first binding for `action`.
/// `None` when the action is unbound (palette-only).
pub fn primary_key(bindings: &KeyBindings, action: BindableAction) -> Option<&KeyBinding> {
    bindings.keys_for(action).first()
}

/// A key whose char can be highlighted inline in a label: a plain character
/// with only NONE or SHIFT modifiers (Shift is implied by the uppercase char
/// itself, cf. `Display for KeyBinding`). Ctrl/Alt-modified chars and all
/// non-char keys (Enter, Tab, arrows, F-keys) are not bracketable inline, and
/// space can't be visibly bracketed inside a word.
fn bracketable_char(kb: &KeyBinding) -> Option<char> {
    match kb.code {
        KeyCode::Char(c) if (kb.modifiers - KeyModifiers::SHIFT).is_empty() && c != ' ' => Some(c),
        _ => None,
    }
}

/// Segment `label` for hotkey display given the action's binding table.
///
/// - No binding → [`HotkeyLabel::Plain`].
/// - Bracketable char that appears in `label` (case-insensitive) →
///   [`HotkeyLabel::Inline`], bracketing the **first** matching char (original
///   case preserved).
/// - Bracketable char absent, or a non-char/modified key →
///   [`HotkeyLabel::Append`] with the key's `Display` string.
pub fn segment_label(label: &str, bindings: &KeyBindings, action: BindableAction) -> HotkeyLabel {
    segment_with_key(label, primary_key(bindings, action))
}

/// Like [`segment_label`] but for a directly-supplied key (e.g. the review
/// view's raw, non-`BindableAction` keys). `None` → [`HotkeyLabel::Plain`].
pub fn segment_with_key(label: &str, key: Option<&KeyBinding>) -> HotkeyLabel {
    let Some(kb) = key else {
        return HotkeyLabel::Plain {
            label: label.to_string(),
        };
    };

    if let Some(key_char) = bracketable_char(kb) {
        let needle = key_char.to_ascii_lowercase();
        if let Some((byte_idx, matched)) = label
            .char_indices()
            .find(|(_, c)| c.to_ascii_lowercase() == needle)
        {
            let prefix = label[..byte_idx].to_string();
            let suffix = label[byte_idx + matched.len_utf8()..].to_string();
            // Bracket the *key's* char (case included) — that's what the user
            // presses, so `N` (Shift-N) reads "[N]ew project" even in a
            // lowercase label.
            return HotkeyLabel::Inline {
                prefix,
                hit: key_char,
                suffix,
            };
        }
    }

    HotkeyLabel::Append {
        label: label.to_string(),
        key: kb.to_string(),
    }
}

/// Convert a segmented label into styled spans. The bracketed hotkey char is
/// emphasized with `accent` + BOLD; brackets and the rest use `base`. All span
/// content is owned, so the result is independent of `seg`'s lifetime.
pub fn hotkey_spans(seg: &HotkeyLabel, base: Style, accent: Style) -> Vec<Span<'static>> {
    match seg {
        HotkeyLabel::Inline {
            prefix,
            hit,
            suffix,
        } => vec![
            Span::styled(prefix.clone(), base),
            Span::styled("[", base),
            Span::styled(hit.to_string(), accent.add_modifier(Modifier::BOLD)),
            Span::styled("]", base),
            Span::styled(suffix.clone(), base),
        ],
        HotkeyLabel::Append { label, key } => vec![
            Span::styled(format!("{label} ["), base),
            Span::styled(key.clone(), accent.add_modifier(Modifier::BOLD)),
            Span::styled("]", base),
        ],
        HotkeyLabel::Plain { label } => vec![Span::styled(label.clone(), base)],
    }
}

/// A rendered, clickable action button: the screen region it occupies paired
/// with the action a click dispatches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ActionButton {
    pub rect: Rect,
    pub action: BindableAction,
}

/// Lay out buttons left-to-right within `area`, separated by `sep_width` cells,
/// and return the clickable region of each. `items` is `(action, width)` in
/// priority order. A button (plus its leading separator) that would overflow
/// `area` is dropped whole, along with every lower-priority button after it —
/// never a half-clipped label. The kept buttons are therefore always a prefix
/// of `items`, so callers can render `items[..result.len()]` to stay in sync.
pub fn layout_buttons(
    items: &[(BindableAction, u16)],
    area: Rect,
    sep_width: u16,
) -> Vec<ActionButton> {
    let mut out = Vec::new();
    let mut x = area.x;
    for (i, &(action, width)) in items.iter().enumerate() {
        let lead = if i == 0 { 0 } else { sep_width };
        if x.saturating_add(lead).saturating_add(width) > area.right() {
            break;
        }
        if i != 0 {
            x += sep_width;
        }
        out.push(ActionButton {
            rect: Rect {
                x,
                y: area.y,
                width,
                height: 1,
            },
            action,
        });
        x += width;
    }
    out
}

/// The action whose button contains the point `(col, row)`, or `None`.
/// First match wins (buttons never overlap).
pub fn button_at(buttons: &[ActionButton], col: u16, row: u16) -> Option<BindableAction> {
    buttons.iter().find_map(|b| {
        let r = b.rect;
        (col >= r.x && col < r.x + r.width && row >= r.y && row < r.y + r.height)
            .then_some(b.action)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::keybindings::KeyBindings;

    /// Build a KeyBindings with a single action bound to one key.
    fn bound(action: BindableAction, code: KeyCode, modifiers: KeyModifiers) -> KeyBindings {
        let mut kb = KeyBindings::default();
        kb.set_keys_for(action, vec![KeyBinding::new(code, modifiers)]);
        kb
    }

    #[test]
    fn segment_inline_brackets_first_matching_char() {
        let kb = bound(
            BindableAction::NewSession,
            KeyCode::Char('n'),
            KeyModifiers::NONE,
        );
        assert_eq!(
            segment_label("new session", &kb, BindableAction::NewSession),
            HotkeyLabel::Inline {
                prefix: String::new(),
                hit: 'n',
                suffix: "ew session".to_string(),
            }
        );
    }

    #[test]
    fn segment_inline_matches_case_insensitively_shows_key_case() {
        // Shift-N matches the lowercase 'n' in the label case-insensitively, but
        // the bracketed char shows the key the user presses ('N').
        let kb = bound(
            BindableAction::NewProject,
            KeyCode::Char('N'),
            KeyModifiers::SHIFT,
        );
        assert_eq!(
            segment_label("new project", &kb, BindableAction::NewProject),
            HotkeyLabel::Inline {
                prefix: String::new(),
                hit: 'N',
                suffix: "ew project".to_string(),
            }
        );
    }

    #[test]
    fn segment_inline_matches_char_mid_label() {
        // 't' first occurs mid-word in "stacked".
        let kb = bound(
            BindableAction::NewStackedSession,
            KeyCode::Char('t'),
            KeyModifiers::NONE,
        );
        assert_eq!(
            segment_label("stacked", &kb, BindableAction::NewStackedSession),
            HotkeyLabel::Inline {
                prefix: "s".to_string(),
                hit: 't',
                suffix: "acked".to_string(),
            }
        );
    }

    #[test]
    fn segment_append_when_char_absent() {
        // 'g' is not present in "summary".
        let kb = bound(
            BindableAction::GenerateSummary,
            KeyCode::Char('g'),
            KeyModifiers::NONE,
        );
        assert_eq!(
            segment_label("summary", &kb, BindableAction::GenerateSummary),
            HotkeyLabel::Append {
                label: "summary".to_string(),
                key: "g".to_string(),
            }
        );
    }

    #[test]
    fn segment_append_for_non_char_keys() {
        let tab = bound(BindableAction::TogglePane, KeyCode::Tab, KeyModifiers::NONE);
        assert_eq!(
            segment_label("view", &tab, BindableAction::TogglePane),
            HotkeyLabel::Append {
                label: "view".to_string(),
                key: "Tab".to_string(),
            }
        );

        let enter = bound(BindableAction::Select, KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(
            segment_label("attach", &enter, BindableAction::Select),
            HotkeyLabel::Append {
                label: "attach".to_string(),
                key: "Enter".to_string(),
            }
        );

        let up = bound(BindableAction::NavigateUp, KeyCode::Up, KeyModifiers::NONE);
        assert_eq!(
            segment_label("up", &up, BindableAction::NavigateUp),
            HotkeyLabel::Append {
                label: "up".to_string(),
                key: "Up".to_string(),
            }
        );
    }

    #[test]
    fn segment_append_for_ctrl_modified_even_if_char_in_label() {
        // Ctrl-c must never bracket inline, even though 'c' is in "clear".
        let kb = bound(
            BindableAction::Quit,
            KeyCode::Char('c'),
            KeyModifiers::CONTROL,
        );
        assert_eq!(
            segment_label("clear", &kb, BindableAction::Quit),
            HotkeyLabel::Append {
                label: "clear".to_string(),
                key: "Ctrl-c".to_string(),
            }
        );
    }

    #[test]
    fn segment_plain_when_unbound() {
        let mut kb = KeyBindings::default();
        kb.set_keys_for(BindableAction::RenameSession, vec![]);
        assert_eq!(
            segment_label("rename", &kb, BindableAction::RenameSession),
            HotkeyLabel::Plain {
                label: "rename".to_string(),
            }
        );
    }

    #[test]
    fn primary_key_returns_first_binding() {
        // NavigateUp's default first binding is 'k'.
        let kb = KeyBindings::default();
        let first = primary_key(&kb, BindableAction::NavigateUp).unwrap();
        assert_eq!(first.code, KeyCode::Char('k'));
    }

    #[test]
    fn bracketable_char_rejects_space_and_nonchar() {
        assert_eq!(
            bracketable_char(&KeyBinding::new(KeyCode::Char(' '), KeyModifiers::NONE)),
            None
        );
        assert_eq!(
            bracketable_char(&KeyBinding::new(KeyCode::Enter, KeyModifiers::NONE)),
            None
        );
        assert_eq!(
            bracketable_char(&KeyBinding::new(KeyCode::F(1), KeyModifiers::NONE)),
            None
        );
        assert_eq!(
            bracketable_char(&KeyBinding::new(KeyCode::Char('k'), KeyModifiers::NONE)),
            Some('k')
        );
        // Shift is allowed (uppercase implies shift).
        assert_eq!(
            bracketable_char(&KeyBinding::new(KeyCode::Char('N'), KeyModifiers::SHIFT)),
            Some('N')
        );
        // Ctrl is not.
        assert_eq!(
            bracketable_char(&KeyBinding::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
            None
        );
    }

    fn btn(x: u16, y: u16, w: u16, action: BindableAction) -> ActionButton {
        ActionButton {
            rect: Rect {
                x,
                y,
                width: w,
                height: 1,
            },
            action,
        }
    }

    /// Render a segmented label back to a plain bracketed string (for asserting
    /// on the visible text without inspecting per-span styling).
    fn rendered_text(seg: &HotkeyLabel) -> String {
        match seg {
            HotkeyLabel::Inline {
                prefix,
                hit,
                suffix,
            } => format!("{prefix}[{hit}]{suffix}"),
            HotkeyLabel::Append { label, key } => format!("{label} [{key}]"),
            HotkeyLabel::Plain { label } => label.clone(),
        }
    }

    #[test]
    fn default_bindings_render_expected_status_bar_labels() {
        // Pins what the user sees for the status bar's session-list action set
        // with the shipped default keybindings.
        let kb = KeyBindings::default();
        let cases = [
            (BindableAction::NewSession, "[n]ew session"),
            (BindableAction::NewStackedSession, "s[t]acked"),
            (BindableAction::DeleteSession, "[d]elete"),
            (BindableAction::OpenReviewDiff, "[r]eview"),
            (BindableAction::OpenInEditor, "edit [.]"),
            (BindableAction::NewProject, "[N]ew project"),
        ];
        for (action, expected) in cases {
            let seg = segment_label(action.button_label(), &kb, action);
            assert_eq!(rendered_text(&seg), expected, "for {action:?}");
        }
    }

    #[test]
    fn button_at_maps_click_to_action() {
        let buttons = vec![btn(2, 5, 6, BindableAction::NewSession)];
        assert_eq!(button_at(&buttons, 4, 5), Some(BindableAction::NewSession));
    }

    #[test]
    fn button_at_click_outside_is_none() {
        let buttons = vec![btn(2, 5, 6, BindableAction::NewSession)];
        assert_eq!(button_at(&buttons, 0, 5), None); // left of button
        assert_eq!(button_at(&buttons, 8, 5), None); // right of button (x+width)
        assert_eq!(button_at(&buttons, 4, 4), None); // wrong row
    }

    #[test]
    fn button_at_first_match_among_multiple() {
        let buttons = vec![
            btn(0, 5, 5, BindableAction::NewSession),
            btn(5, 5, 5, BindableAction::DeleteSession),
        ];
        assert_eq!(
            button_at(&buttons, 7, 5),
            Some(BindableAction::DeleteSession)
        );
        assert_eq!(button_at(&buttons, 2, 5), Some(BindableAction::NewSession));
    }

    #[test]
    fn button_at_empty_is_none() {
        assert_eq!(button_at(&[], 4, 5), None);
    }

    fn area(x: u16, y: u16, w: u16) -> Rect {
        Rect {
            x,
            y,
            width: w,
            height: 1,
        }
    }

    #[test]
    fn layout_buttons_positions_left_to_right_non_overlapping() {
        let items = [
            (BindableAction::NewSession, 5),
            (BindableAction::DeleteSession, 6),
        ];
        let out = layout_buttons(&items, area(2, 9, 40), 3);
        assert_eq!(out.len(), 2);
        // First at area.x; second after first + separator.
        assert_eq!(out[0].rect.x, 2);
        assert_eq!(out[0].rect.width, 5);
        assert_eq!(out[1].rect.x, 2 + 5 + 3);
        assert_eq!(out[1].rect.width, 6);
        // Non-overlapping and in-bounds.
        assert!(out[0].rect.x + out[0].rect.width <= out[1].rect.x);
        assert!(out[1].rect.x + out[1].rect.width <= area(2, 9, 40).right());
    }

    #[test]
    fn layout_buttons_drops_whole_buttons_that_overflow() {
        let items = [
            (BindableAction::NewSession, 5),
            (BindableAction::DeleteSession, 6),
            (BindableAction::OpenReviewDiff, 6),
        ];
        // Room for the first (5) but not "sep+second" (3+6=9) → only one kept.
        let out = layout_buttons(&items, area(0, 0, 8), 3);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].action, BindableAction::NewSession);
        assert!(out[0].rect.x + out[0].rect.width <= 8);
    }

    #[test]
    fn layout_buttons_empty_when_no_room() {
        let items = [(BindableAction::NewSession, 5)];
        // Width 3 can't fit a 5-wide button.
        let out = layout_buttons(&items, area(0, 0, 3), 3);
        assert!(out.is_empty());
    }

    #[test]
    fn click_maps_to_command_via_from() {
        use crate::tui::event::UserCommand;
        let buttons = vec![btn(2, 5, 6, BindableAction::NewSession)];
        let action = button_at(&buttons, 4, 5).unwrap();
        assert!(matches!(UserCommand::from(action), UserCommand::NewSession));
    }
}
