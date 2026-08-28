//! Theme override configuration
//!
//! Allows users to customise individual theme colors via `[theme]` in
//! `config.toml`.  Supports named ANSI colors, 256-color indices, and
//! 24-bit RGB hex values.

use std::fmt;

use diffgrid::style::Appearance;
use ratatui_core::style::Color;
use serde::de::{self, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

// ---------------------------------------------------------------------------
// ColorValue — a serde-friendly wrapper around ratatui_core::style::Color
// ---------------------------------------------------------------------------

/// A user-facing color value that deserializes from:
/// - Named colors: `"red"`, `"cyan"`, `"dark_gray"`, etc.
/// - Indexed (256): an integer like `117`
/// - RGB hex: `"#89b4fa"`
/// - Reset: `"reset"`
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ColorValue(pub Color);

impl From<ColorValue> for Color {
    fn from(cv: ColorValue) -> Self {
        cv.0
    }
}

impl From<Color> for ColorValue {
    fn from(c: Color) -> Self {
        Self(c)
    }
}

impl Serialize for ColorValue {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self.0 {
            Color::Reset => serializer.serialize_str("reset"),
            Color::Black => serializer.serialize_str("black"),
            Color::Red => serializer.serialize_str("red"),
            Color::Green => serializer.serialize_str("green"),
            Color::Yellow => serializer.serialize_str("yellow"),
            Color::Blue => serializer.serialize_str("blue"),
            Color::Magenta => serializer.serialize_str("magenta"),
            Color::Cyan => serializer.serialize_str("cyan"),
            Color::Gray => serializer.serialize_str("gray"),
            Color::DarkGray => serializer.serialize_str("dark_gray"),
            Color::LightRed => serializer.serialize_str("light_red"),
            Color::LightGreen => serializer.serialize_str("light_green"),
            Color::LightYellow => serializer.serialize_str("light_yellow"),
            Color::LightBlue => serializer.serialize_str("light_blue"),
            Color::LightMagenta => serializer.serialize_str("light_magenta"),
            Color::LightCyan => serializer.serialize_str("light_cyan"),
            Color::White => serializer.serialize_str("white"),
            Color::Indexed(i) => serializer.serialize_u8(i),
            Color::Rgb(r, g, b) => {
                serializer.serialize_str(&format!("#{:02x}{:02x}{:02x}", r, g, b))
            }
        }
    }
}

impl<'de> Deserialize<'de> for ColorValue {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(ColorValueVisitor)
    }
}

struct ColorValueVisitor;

impl<'de> Visitor<'de> for ColorValueVisitor {
    type Value = ColorValue;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter
            .write_str("a color name (\"red\"), an index (117), or an RGB hex string (\"#89b4fa\")")
    }

    fn visit_u64<E: de::Error>(self, v: u64) -> Result<Self::Value, E> {
        if v > 255 {
            return Err(de::Error::custom(format!(
                "color index {v} out of range 0..255"
            )));
        }
        Ok(ColorValue(Color::Indexed(v as u8)))
    }

    fn visit_i64<E: de::Error>(self, v: i64) -> Result<Self::Value, E> {
        if !(0..=255).contains(&v) {
            return Err(de::Error::custom(format!(
                "color index {v} out of range 0..255"
            )));
        }
        Ok(ColorValue(Color::Indexed(v as u8)))
    }

    fn visit_str<E: de::Error>(self, s: &str) -> Result<Self::Value, E> {
        parse_color_str(s).map_err(de::Error::custom)
    }
}

fn parse_color_str(s: &str) -> Result<ColorValue, String> {
    // RGB hex
    if let Some(hex) = s.strip_prefix('#') {
        if hex.len() != 6 {
            return Err(format!("invalid hex color \"{s}\": expected 6 hex digits"));
        }
        let r =
            u8::from_str_radix(&hex[0..2], 16).map_err(|_| format!("invalid hex color \"{s}\""))?;
        let g =
            u8::from_str_radix(&hex[2..4], 16).map_err(|_| format!("invalid hex color \"{s}\""))?;
        let b =
            u8::from_str_radix(&hex[4..6], 16).map_err(|_| format!("invalid hex color \"{s}\""))?;
        return Ok(ColorValue(Color::Rgb(r, g, b)));
    }

    // Named colors (case-insensitive, underscores or hyphens)
    let normalized = s.to_lowercase().replace('-', "_");
    match normalized.as_str() {
        "reset" => Ok(ColorValue(Color::Reset)),
        "black" => Ok(ColorValue(Color::Black)),
        "red" => Ok(ColorValue(Color::Red)),
        "green" => Ok(ColorValue(Color::Green)),
        "yellow" => Ok(ColorValue(Color::Yellow)),
        "blue" => Ok(ColorValue(Color::Blue)),
        "magenta" => Ok(ColorValue(Color::Magenta)),
        "cyan" => Ok(ColorValue(Color::Cyan)),
        "gray" | "grey" => Ok(ColorValue(Color::Gray)),
        "dark_gray" | "dark_grey" | "darkgray" | "darkgrey" => Ok(ColorValue(Color::DarkGray)),
        "light_red" | "lightred" => Ok(ColorValue(Color::LightRed)),
        "light_green" | "lightgreen" => Ok(ColorValue(Color::LightGreen)),
        "light_yellow" | "lightyellow" => Ok(ColorValue(Color::LightYellow)),
        "light_blue" | "lightblue" => Ok(ColorValue(Color::LightBlue)),
        "light_magenta" | "lightmagenta" => Ok(ColorValue(Color::LightMagenta)),
        "light_cyan" | "lightcyan" => Ok(ColorValue(Color::LightCyan)),
        "white" => Ok(ColorValue(Color::White)),
        _ => Err(format!("unknown color name \"{s}\"")),
    }
}

// ---------------------------------------------------------------------------
// AgentWorkingStyle — a special colour value that can be "rainbow" or a solid
// colour. Used only for the Working spinner.
// ---------------------------------------------------------------------------

/// Rainbow palette cycled by the Working spinner.
pub const RAINBOW_PALETTE: &[Color] = &[
    Color::Rgb(255, 138, 128), // coral
    Color::Rgb(255, 189, 128), // peach
    Color::Rgb(249, 226, 138), // light yellow
    Color::Rgb(166, 227, 161), // mint
    Color::Rgb(138, 200, 255), // sky
    Color::Rgb(203, 166, 247), // lavender
];

/// Style used to colour the Working spinner.
///
/// Config strings: `"rainbow"` for the cycling palette, or any regular
/// [`ColorValue`] string for a solid colour (e.g. `"green"`, `"#a6e3a1"`, `156`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentWorkingStyle {
    /// Cycle through [`RAINBOW_PALETTE`] so each spinner tick is a new hue.
    Rainbow,
    /// A single static colour.
    Solid(Color),
}

impl AgentWorkingStyle {
    /// Colour to render at the given tick. For `Rainbow`, cycles through
    /// [`RAINBOW_PALETTE`].
    pub fn color_for_tick(&self, tick: u64) -> Color {
        match self {
            Self::Rainbow => RAINBOW_PALETTE[tick as usize % RAINBOW_PALETTE.len()],
            Self::Solid(c) => *c,
        }
    }
}

impl Serialize for AgentWorkingStyle {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Rainbow => serializer.serialize_str("rainbow"),
            Self::Solid(c) => ColorValue(*c).serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for AgentWorkingStyle {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(AgentWorkingStyleVisitor)
    }
}

struct AgentWorkingStyleVisitor;

impl<'de> Visitor<'de> for AgentWorkingStyleVisitor {
    type Value = AgentWorkingStyle;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str(
            "\"rainbow\", a color name (\"red\"), an index (117), or an RGB hex string (\"#89b4fa\")",
        )
    }

    fn visit_str<E: de::Error>(self, s: &str) -> Result<Self::Value, E> {
        if s.eq_ignore_ascii_case("rainbow") {
            return Ok(AgentWorkingStyle::Rainbow);
        }
        parse_color_str(s)
            .map(|cv| AgentWorkingStyle::Solid(cv.0))
            .map_err(de::Error::custom)
    }

    fn visit_u64<E: de::Error>(self, v: u64) -> Result<Self::Value, E> {
        if v > 255 {
            return Err(de::Error::custom(format!(
                "color index {v} out of range 0..255"
            )));
        }
        Ok(AgentWorkingStyle::Solid(Color::Indexed(v as u8)))
    }

    fn visit_i64<E: de::Error>(self, v: i64) -> Result<Self::Value, E> {
        if !(0..=255).contains(&v) {
            return Err(de::Error::custom(format!(
                "color index {v} out of range 0..255"
            )));
        }
        Ok(AgentWorkingStyle::Solid(Color::Indexed(v as u8)))
    }
}

// ---------------------------------------------------------------------------
// AppearanceValue — light/dark terminal background, declared by the user
// ---------------------------------------------------------------------------

/// Whether the terminal this runs in draws light text on a dark background or
/// the other way round.
///
/// Nothing detects this: the terminal only reports its background via `OSC 11`,
/// which is deliberately out of scope, so it is a claim the user makes about
/// their own terminal. It matters because a derived *fill* (the review diff
/// view's line bands) has to be blended toward the surface it sits on —
/// blending toward black on a light terminal gives a near-black band under dark
/// text.
///
/// Spelled out rather than reusing [`diffgrid::style::Appearance`] directly so
/// the TOML spelling is ours and core needn't turn on diffgrid's `serde`
/// feature for one enum — the same reason [`ColorValue`] wraps
/// `ratatui::style::Color`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AppearanceValue {
    /// Light text on a dark background — the assumption every preset ships with.
    #[default]
    Dark,
    /// Dark text on a light background.
    Light,
}

impl AppearanceValue {
    /// The config spelling, as written in `config.toml` and shown in settings.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Dark => "dark",
            Self::Light => "light",
        }
    }

    /// Parse a config spelling, case-insensitively. `None` for anything else.
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_lowercase().as_str() {
            "dark" => Some(Self::Dark),
            "light" => Some(Self::Light),
            _ => None,
        }
    }
}

impl From<AppearanceValue> for Appearance {
    fn from(v: AppearanceValue) -> Self {
        match v {
            AppearanceValue::Dark => Appearance::Dark,
            AppearanceValue::Light => Appearance::Light,
        }
    }
}

impl fmt::Display for AppearanceValue {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

// ---------------------------------------------------------------------------
// ThemeOverrides — optional per-field overrides loaded from [theme]
// ---------------------------------------------------------------------------

/// User-supplied theme overrides.  Every field is optional; only `Some`
/// values replace the base theme color.
///
/// The `project_colors: Vec<(Color, Color)>` field from `Theme` is
/// intentionally omitted — paired-tuple arrays are awkward in TOML and
/// the feature has minimal user demand.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ThemeOverrides {
    /// Force a base palette: "basic", "indexed", or "truecolor".
    /// When set, the named palette is used instead of auto-detection.
    pub preset: Option<String>,

    /// Declare the terminal background as `"dark"` or `"light"`.
    ///
    /// Unset means "whatever the preset says", which today is dark for every
    /// one of them — so the default behaviour is unchanged. Set it to
    /// `"light"` on a light terminal and derived fills (the review diff view's
    /// add/remove bands) blend toward white instead of black.
    pub appearance: Option<AppearanceValue>,

    // Pane borders
    pub border_focused: Option<ColorValue>,
    pub border_unfocused: Option<ColorValue>,

    // Selection
    pub selection_bg: Option<ColorValue>,
    pub selection_fg: Option<ColorValue>,

    // Session status indicators
    pub status_creating: Option<ColorValue>,
    pub status_running: Option<ColorValue>,
    pub status_stopped: Option<ColorValue>,
    pub status_pr: Option<ColorValue>,
    pub status_pr_merged: Option<ColorValue>,

    // PR badge text colours
    pub pr_open: Option<ColorValue>,
    pub pr_draft: Option<ColorValue>,
    pub pr_closed: Option<ColorValue>,

    // PR label pill backgrounds (used when invert_pr_label_color = false)
    pub pr_pill_open_bg: Option<ColorValue>,
    pub pr_pill_draft_bg: Option<ColorValue>,
    pub pr_pill_closed_bg: Option<ColorValue>,
    pub pr_pill_review_bg: Option<ColorValue>,
    pub pr_pill_merged_bg: Option<ColorValue>,
    /// Foreground text colour for PR label pills.
    pub pr_pill_text: Option<ColorValue>,

    // Agent state and notification indicators
    pub agent_working: Option<AgentWorkingStyle>,
    pub agent_waiting: Option<ColorValue>,
    pub unread_indicator: Option<ColorValue>,

    // Text
    pub text_primary: Option<ColorValue>,
    pub text_secondary: Option<ColorValue>,
    pub text_accent: Option<ColorValue>,

    // Diff colors
    pub diff_added: Option<ColorValue>,
    pub diff_removed: Option<ColorValue>,
    pub diff_hunk_header: Option<ColorValue>,
    pub diff_file_header: Option<ColorValue>,
    pub diff_context: Option<ColorValue>,
    pub diff_expand_bg: Option<ColorValue>,
    pub diff_hunk_header_bg: Option<ColorValue>,

    // Modal borders
    pub modal_info: Option<ColorValue>,
    pub modal_warning: Option<ColorValue>,
    pub modal_error: Option<ColorValue>,

    // Quick-switch palette command rows
    pub palette_command_bg: Option<ColorValue>,
    pub palette_command_fg: Option<ColorValue>,

    // Status bar
    pub status_bar_bg: Option<ColorValue>,
    pub status_bar_fg: Option<ColorValue>,
    /// Accent for the hotkey letter in `[n]ew session` and the board's top-bar
    /// title. Distinct from `text_accent` because both are painted *on the status
    /// bar*, so they must contrast with `status_bar_bg` rather than the canvas.
    pub status_bar_accent: Option<ColorValue>,
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- ColorValue deserialization -----------------------------------------

    /// Helper wrapper so we can test ColorValue via TOML key = value pairs
    #[derive(Deserialize, Serialize)]
    struct Wrap {
        c: ColorValue,
    }

    fn parse_color(toml_val: &str) -> ColorValue {
        let input = format!("c = {toml_val}");
        toml::from_str::<Wrap>(&input).unwrap().c
    }

    #[test]
    fn test_color_value_named() {
        assert_eq!(parse_color("\"red\"").0, Color::Red);
    }

    #[test]
    fn test_color_value_named_dark_gray() {
        assert_eq!(parse_color("\"dark_gray\"").0, Color::DarkGray);
    }

    #[test]
    fn test_color_value_hex() {
        assert_eq!(parse_color("\"#89b4fa\"").0, Color::Rgb(137, 180, 250));
    }

    #[test]
    fn test_color_value_indexed() {
        assert_eq!(parse_color("117").0, Color::Indexed(117));
    }

    #[test]
    fn test_color_value_reset() {
        assert_eq!(parse_color("\"reset\"").0, Color::Reset);
    }

    /// Every form a `[theme]` value may take, pinned in one place with its
    /// serialized spelling.
    ///
    /// `config.toml` is never rewritten, so each of these four forms is
    /// permanently load-bearing: a user's file written years ago must still
    /// parse. `ColorValue`'s `Serialize`/`Deserialize` are hand-written over a
    /// `Color` owned by an external crate, which makes them exactly the kind of
    /// thing a dependency swap can silently change — this asserts round-trip
    /// stability so such a change fails loudly instead.
    ///
    /// The parse side is covered per-form above; what this adds is the
    /// *serialize* direction (notably `Reset`, which no other round-trip test
    /// reaches) and proof that parse and serialize agree.
    #[test]
    fn test_color_value_all_config_forms_roundtrip() {
        // (TOML literal as it may appear in config.toml, parsed Color)
        let cases = [
            ("\"reset\"", Color::Reset),
            ("\"dark_gray\"", Color::DarkGray),
            ("117", Color::Indexed(117)),
            ("\"#89b4fa\"", Color::Rgb(137, 180, 250)),
        ];
        for (literal, expected) in cases {
            let parsed = parse_color(literal);
            assert_eq!(parsed.0, expected, "parsing {literal}");

            // Serializing must reproduce the same TOML literal, so a rewritten
            // value re-parses identically.
            let emitted = toml::to_string(&Wrap { c: parsed }).expect("serialize");
            let emitted_value = emitted
                .trim()
                .strip_prefix("c = ")
                .unwrap_or_else(|| panic!("unexpected TOML shape: {emitted:?}"))
                .to_string();
            assert_eq!(emitted_value, literal, "serializing {expected:?}");
            assert_eq!(
                parse_color(&emitted_value).0,
                expected,
                "re-parsing {emitted_value}"
            );
        }
    }

    // ---- AgentWorkingStyle deserialization ----------------------------------

    #[derive(Deserialize)]
    struct AwWrap {
        c: AgentWorkingStyle,
    }

    fn parse_aw(toml_val: &str) -> AgentWorkingStyle {
        let input = format!("c = {toml_val}");
        toml::from_str::<AwWrap>(&input).unwrap().c
    }

    #[test]
    fn test_agent_working_rainbow() {
        assert_eq!(parse_aw("\"rainbow\""), AgentWorkingStyle::Rainbow);
    }

    #[test]
    fn test_agent_working_rainbow_case_insensitive() {
        assert_eq!(parse_aw("\"Rainbow\""), AgentWorkingStyle::Rainbow);
        assert_eq!(parse_aw("\"RAINBOW\""), AgentWorkingStyle::Rainbow);
    }

    #[test]
    fn test_agent_working_solid_named() {
        assert_eq!(parse_aw("\"red\""), AgentWorkingStyle::Solid(Color::Red));
    }

    #[test]
    fn test_agent_working_solid_hex() {
        assert_eq!(
            parse_aw("\"#89b4fa\""),
            AgentWorkingStyle::Solid(Color::Rgb(137, 180, 250))
        );
    }

    #[test]
    fn test_agent_working_solid_indexed() {
        assert_eq!(
            parse_aw("156"),
            AgentWorkingStyle::Solid(Color::Indexed(156))
        );
    }

    #[test]
    fn test_agent_working_roundtrip_rainbow() {
        #[derive(Serialize)]
        struct W {
            c: AgentWorkingStyle,
        }
        let w = W {
            c: AgentWorkingStyle::Rainbow,
        };
        let s = toml::to_string(&w).unwrap();
        assert!(s.contains("rainbow"));
    }

    #[test]
    fn test_agent_working_color_for_tick_rainbow_cycles() {
        let r = AgentWorkingStyle::Rainbow;
        assert_eq!(r.color_for_tick(0), RAINBOW_PALETTE[0]);
        assert_eq!(
            r.color_for_tick(RAINBOW_PALETTE.len() as u64),
            RAINBOW_PALETTE[0]
        );
        assert_eq!(r.color_for_tick(1), RAINBOW_PALETTE[1]);
    }

    #[test]
    fn test_agent_working_color_for_tick_solid_constant() {
        let s = AgentWorkingStyle::Solid(Color::Red);
        assert_eq!(s.color_for_tick(0), Color::Red);
        assert_eq!(s.color_for_tick(42), Color::Red);
    }

    // ---- ThemeOverrides deserialization --------------------------------------

    #[test]
    fn test_theme_overrides_empty() {
        let overrides: ThemeOverrides = toml::from_str("").unwrap();
        assert!(overrides.preset.is_none());
        assert!(overrides.border_focused.is_none());
        assert!(overrides.status_running.is_none());
    }

    #[test]
    fn test_theme_overrides_partial() {
        let toml_str = r##"
            preset = "truecolor"
            border_focused = "#ff6600"
            status_running = "green"
            selection_bg = 60
        "##;
        let overrides: ThemeOverrides = toml::from_str(toml_str).unwrap();
        assert_eq!(overrides.preset.as_deref(), Some("truecolor"));
        assert_eq!(overrides.border_focused.unwrap().0, Color::Rgb(255, 102, 0));
        assert_eq!(overrides.status_running.unwrap().0, Color::Green);
        assert_eq!(overrides.selection_bg.unwrap().0, Color::Indexed(60));
        // Unset fields remain None
        assert!(overrides.border_unfocused.is_none());
        assert!(overrides.diff_added.is_none());
    }

    #[test]
    fn test_theme_appearance_parses_from_toml() {
        let overrides: ThemeOverrides = toml::from_str(r#"appearance = "light""#).unwrap();
        assert_eq!(overrides.appearance, Some(AppearanceValue::Light));
        assert_eq!(
            Appearance::from(overrides.appearance.unwrap()),
            Appearance::Light
        );

        // Unset is the common case and must not imply a surface of its own —
        // the preset's declaration wins.
        let none: ThemeOverrides = toml::from_str("").unwrap();
        assert!(none.appearance.is_none());
    }

    #[test]
    fn test_appearance_value_parse_is_case_insensitive_and_strict() {
        assert_eq!(
            AppearanceValue::parse("Light"),
            Some(AppearanceValue::Light)
        );
        assert_eq!(
            AppearanceValue::parse(" dark "),
            Some(AppearanceValue::Dark)
        );
        assert_eq!(AppearanceValue::parse("solarized"), None);
        assert_eq!(AppearanceValue::Light.as_str(), "light");
    }

    // ---- TOML round-trip ----------------------------------------------------

    #[test]
    fn test_theme_overrides_roundtrip() {
        let original = ThemeOverrides {
            preset: Some("indexed".to_string()),
            border_focused: Some(ColorValue(Color::Rgb(255, 0, 128))),
            status_running: Some(ColorValue(Color::Green)),
            selection_bg: Some(ColorValue(Color::Indexed(60))),
            ..Default::default()
        };
        let serialized = toml::to_string_pretty(&original).unwrap();
        let deserialized: ThemeOverrides = toml::from_str(&serialized).unwrap();
        assert_eq!(deserialized.preset, original.preset);
        assert_eq!(deserialized.border_focused, original.border_focused);
        assert_eq!(deserialized.status_running, original.status_running);
        assert_eq!(deserialized.selection_bg, original.selection_bg);
        assert!(deserialized.border_unfocused.is_none());
    }

    // ---- Backwards compatibility --------------------------------------------

    #[test]
    fn test_missing_theme_section_is_default() {
        // A config file with no [theme] section should parse with all defaults
        let config_toml = r#"
            default_program = "claude"
            branch_prefix = ""
        "#;
        // ThemeOverrides uses #[serde(default)] so missing section = all None
        let overrides: ThemeOverrides = toml::from_str("").unwrap();
        assert!(overrides.preset.is_none());
        assert!(overrides.border_focused.is_none());

        // Also verify that full Config parsing works when [theme] is absent
        let _val: toml::Value = toml::from_str(config_toml).unwrap();
    }
}
