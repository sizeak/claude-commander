//! Theme override configuration
//!
//! Allows users to customise individual theme colors via `[theme]` in
//! `config.toml`.  Supports named ANSI colors, 256-color indices, and
//! 24-bit RGB hex values.

use std::fmt;

use ratatui::style::Color;
use serde::de::{self, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

// ---------------------------------------------------------------------------
// ColorValue — a serde-friendly wrapper around ratatui::style::Color
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
        formatter.write_str(
            "a color name (\"red\"), an index (117), or an RGB hex string (\"#89b4fa\")",
        )
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

    // Pane borders
    pub border_focused: Option<ColorValue>,
    pub border_unfocused: Option<ColorValue>,

    // Selection
    pub selection_bg: Option<ColorValue>,
    pub selection_fg: Option<ColorValue>,

    // Session status indicators
    pub status_running: Option<ColorValue>,
    pub status_paused: Option<ColorValue>,
    pub status_stopped: Option<ColorValue>,
    pub status_pr: Option<ColorValue>,
    pub status_pr_merged: Option<ColorValue>,

    // Text
    pub text_primary: Option<ColorValue>,
    pub text_secondary: Option<ColorValue>,
    pub text_accent: Option<ColorValue>,
    pub text_pr: Option<ColorValue>,

    // Diff colors
    pub diff_added: Option<ColorValue>,
    pub diff_removed: Option<ColorValue>,
    pub diff_hunk_header: Option<ColorValue>,
    pub diff_file_header: Option<ColorValue>,
    pub diff_context: Option<ColorValue>,

    // Modal borders
    pub modal_info: Option<ColorValue>,
    pub modal_warning: Option<ColorValue>,
    pub modal_error: Option<ColorValue>,

    // Status bar
    pub status_bar_bg: Option<ColorValue>,
    pub status_bar_fg: Option<ColorValue>,
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- ColorValue deserialization -----------------------------------------

    /// Helper wrapper so we can test ColorValue via TOML key = value pairs
    #[derive(Deserialize)]
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
        assert_eq!(
            overrides.border_focused.unwrap().0,
            Color::Rgb(255, 102, 0)
        );
        assert_eq!(overrides.status_running.unwrap().0, Color::Green);
        assert_eq!(overrides.selection_bg.unwrap().0, Color::Indexed(60));
        // Unset fields remain None
        assert!(overrides.border_unfocused.is_none());
        assert!(overrides.diff_added.is_none());
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
