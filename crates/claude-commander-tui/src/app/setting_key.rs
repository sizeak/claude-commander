//! Typed identities for the settings modal's rows.
//!
//! Each [`SettingsRow`](super::SettingsRow) carries a [`SettingKey`] naming
//! what it edits, and the modal's handlers dispatch on it. The keys are enums
//! rather than strings so a misspelt key is a compile error, and the per-kind
//! enums make the handlers' matches exhaustive: a row added without an apply
//! arm does not build. Nothing here is persisted or sent anywhere — the TOML
//! spelling of each field lives on [`Config`](claude_commander_core::config::Config)
//! itself — so the variants are free to be renamed.
//!
//! The split follows what the handlers need, not the tabs alone:
//!
//! - [`ToggleKey`]: every two-state row on any tab, applied by
//!   `apply_bool_setting`. A toggle row can only be built from one, and a
//!   text row only from a [`TextRowKey`] — which a `ToggleKey` is not — so a
//!   toggle cannot reach the text-edit path by accident.
//! - [`GeneralKey`], [`VoiceKey`], [`ServerKey`]: the text rows of those tabs,
//!   applied by `apply_settings_edit`.
//! - [`ThemeKey`]: the Theme tab's rows, colour slots included ([`ThemeColour`]).
//! - [`BindableAction`]: the Keybindings tab, keyed by the action it rebinds.

use claude_commander_core::config::keybindings::BindableAction;
use claude_commander_core::config::theme::{ColorValue, ThemeOverrides};
use ratatui::style::Color;

/// What a settings row edits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingKey {
    /// A section heading or spacer, which edits nothing.
    None,
    /// A two-state row, on whichever tab.
    Toggle(ToggleKey),
    /// A General-tab text row.
    General(GeneralKey),
    /// A Voice-tab text row.
    Voice(VoiceKey),
    /// A Server-tab text row.
    Server(ServerKey),
    /// A Theme-tab row.
    Theme(ThemeKey),
    /// A Keybindings-tab row: the action whose keys it lists.
    Keybinding(BindableAction),
}

/// A boolean setting, flipped in place by its toggle row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToggleKey {
    // General
    PerRepoWorktreeDirs,
    FetchBeforeCreate,
    SkipLfsSmudge,
    ResumeSession,
    NixDevelop,
    HibernateEnabled,
    ProjectPullEnabled,
    InvertPrLabelColor,
    ShowSessionProgram,
    HideEmptySections,
    RoundedBorders,
    DimUnfocusedPreview,
    PrecomputeReviewCaches,
    AiSummaryEnabled,
    CommanderEnabled,
    TelemetryEnabled,
    // Voice
    SttEnabled,
    SttPauseMedia,
    ConversationEnabled,
    // Server
    ServerAutoStart,
}

/// A General-tab text setting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GeneralKey {
    BranchPrefix,
    ShellProgram,
    WorktreesDir,
    ProjectsDir,
    CloneTimeoutSecs,
    RepoListTimeoutSecs,
    HibernateIdleTimeoutSecs,
    HibernateCheckIntervalSecs,
    InProgressLimit,
    Editor,
    EditorGui,
    CodeHostProvider,
    GitlabHostname,
    PrCheckIntervalSecs,
    ProjectPullIntervalSecs,
    RecentSessionsLimit,
    DimUnfocusedOpacity,
    UiRefreshFps,
    MaxConcurrentTmux,
    SessionNumberDebounceMs,
    AiSummaryModel,
    CommanderProgram,
    CommanderDir,
}

/// A Voice-tab text setting (`[stt]` and `[conversation]`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoiceKey {
    SttBaseUrl,
    SttModel,
    SttLanguage,
    SttPrompt,
    SttInputDevice,
    SttDictationSubmit,
    ConversationBaseUrl,
    ConversationModel,
    ConversationVoice,
    ConversationFormat,
    ConversationSpeed,
    ConversationVolume,
    ConversationSpeakScope,
    ConversationName,
}

/// A Server-tab text setting (`[server]`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServerKey {
    Bind,
    Port,
    Token,
    CorsAllowedOrigins,
}

/// A Theme-tab row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThemeKey {
    /// Which theme the tab edits (shown once there are workspaces).
    Scope,
    /// "Reset to usual theme" (a workspace scope only).
    Reset,
    Preset,
    Appearance,
    /// One colour slot.
    Colour(ThemeColour),
}

/// Declares [`ThemeColour`] from `Variant => field, "Label"` triples, so the
/// row label, the [`ThemeOverrides`] slot and the resolved
/// [`Theme`](crate::theme::Theme) field of each colour are named once.
macro_rules! theme_colours {
    ($($variant:ident => $field:ident, $label:literal;)*) => {
        /// A Theme-tab colour row, in display order.
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum ThemeColour {
            $($variant,)*
        }

        impl ThemeColour {
            /// Every colour row, in the order the Theme tab lists them.
            pub const ALL: &[Self] = &[$(Self::$variant,)*];

            /// The row's label.
            pub fn label(self) -> &'static str {
                match self {
                    $(Self::$variant => $label,)*
                }
            }

            /// The override this colour row edits.
            pub fn slot(self, overrides: &mut ThemeOverrides) -> &mut Option<ColorValue> {
                match self {
                    $(Self::$variant => &mut overrides.$field,)*
                }
            }

            /// The scope's own value, if it sets one.
            pub fn own(self, overrides: &ThemeOverrides) -> Option<ColorValue> {
                match self {
                    $(Self::$variant => overrides.$field,)*
                }
            }

            /// The colour `theme` resolves this slot to.
            pub fn resolved(self, theme: &crate::theme::Theme) -> Color {
                match self {
                    $(Self::$variant => theme.$field,)*
                }
            }
        }
    };
}

theme_colours! {
    BorderFocused => border_focused, "Border Focused";
    BorderUnfocused => border_unfocused, "Border Unfocused";
    SelectionBg => selection_bg, "Selection BG";
    StatusRunning => status_running, "Status Running";
    StatusStopped => status_stopped, "Status Stopped";
    StatusPr => status_pr, "Status PR";
    StatusPrMerged => status_pr_merged, "Status PR Merged";
    PrOpen => pr_open, "PR Open";
    PrDraft => pr_draft, "PR Draft";
    PrClosed => pr_closed, "PR Closed";
    TextPrimary => text_primary, "Text Primary";
    TextSecondary => text_secondary, "Text Secondary";
    TextAccent => text_accent, "Text Accent";
    DiffAdded => diff_added, "Diff Added";
    DiffRemoved => diff_removed, "Diff Removed";
    DiffHunkHeader => diff_hunk_header, "Diff Hunk Header";
    DiffFileHeader => diff_file_header, "Diff File Header";
    ModalInfo => modal_info, "Modal Info";
    ModalWarning => modal_warning, "Modal Warning";
    ModalError => modal_error, "Modal Error";
    StatusBarBg => status_bar_bg, "Status Bar BG";
    StatusBarFg => status_bar_fg, "Status Bar FG";
    StatusBarAccent => status_bar_accent, "Status Bar Accent";
}

macro_rules! into_setting_key {
    ($($ty:ty => $variant:ident),* $(,)?) => {
        $(impl From<$ty> for SettingKey {
            fn from(key: $ty) -> Self {
                Self::$variant(key)
            }
        })*
    };
}

into_setting_key!(
    ToggleKey => Toggle,
    GeneralKey => General,
    VoiceKey => Voice,
    ServerKey => Server,
    ThemeKey => Theme,
    BindableAction => Keybinding,
);

impl From<ThemeColour> for SettingKey {
    fn from(colour: ThemeColour) -> Self {
        Self::Theme(ThemeKey::Colour(colour))
    }
}

/// A key a free-text row may carry: every kind but [`ToggleKey`], whose rows
/// flip in place and have no text editor to commit through.
pub trait TextRowKey: Into<SettingKey> {}

impl TextRowKey for GeneralKey {}
impl TextRowKey for VoiceKey {}
impl TextRowKey for ServerKey {}
impl TextRowKey for ThemeKey {}
impl TextRowKey for ThemeColour {}
impl TextRowKey for BindableAction {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn theme_colours_are_distinct_slots() {
        // Each colour row must edit its own override: two variants sharing a
        // field would make one row silently write the other's colour.
        let mut overrides = ThemeOverrides::default();
        for (i, colour) in ThemeColour::ALL.iter().enumerate() {
            *colour.slot(&mut overrides) = Some(ColorValue(Color::Indexed(i as u8)));
        }
        for (i, colour) in ThemeColour::ALL.iter().enumerate() {
            assert_eq!(
                colour.own(&overrides),
                Some(ColorValue(Color::Indexed(i as u8))),
                "{colour:?}"
            );
        }
    }

    #[test]
    fn a_colour_converts_to_its_theme_key() {
        assert_eq!(
            SettingKey::from(ThemeColour::TextAccent),
            SettingKey::Theme(ThemeKey::Colour(ThemeColour::TextAccent))
        );
    }
}
