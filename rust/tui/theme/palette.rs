//! What a theme paints each role in: the compiled-in presets and the
//! `theme.json` documents operators write.
//!
//! A color is a CSS color (`#rrggbb` or a name) or a terminal palette index,
//! written as a number or as its digits.

use std::collections::BTreeMap;
use std::path::Path;

use crossterm::style::Color;
use serde_json::Value;

use super::SemanticColor;

/// The paintable roles and the names a theme file gives them.
const ROLE_NAMES: &[(&str, SemanticColor)] = &[
    ("muted", SemanticColor::TextMuted),
    ("accent", SemanticColor::Accent),
    ("info", SemanticColor::Info),
    ("success", SemanticColor::Success),
    ("warning", SemanticColor::Warning),
    ("danger", SemanticColor::Danger),
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Palette {
    /// The color each role the theme colors is painted in.
    Colors(BTreeMap<SemanticColor, Color>),
    /// No role colors (mono / high-contrast).
    Plain,
}

const PRESET_JSON: &[(&str, &str)] = &[
    ("graphite-dark", include_str!("../themes/graphite-dark.json")),
    ("paper-light", include_str!("../themes/paper-light.json")),
    ("titanium", include_str!("../themes/titanium.json")),
    ("nord", include_str!("../themes/nord.json")),
    ("color-blind", include_str!("../themes/color-blind.json")),
];

/// The names of the compiled-in presets, in the order they ship.
pub(crate) fn preset_names() -> impl Iterator<Item = &'static str> {
    PRESET_JSON.iter().map(|(name, _)| *name)
}

/// A compiled-in preset's palette; `None` when no preset has that name. The
/// presets ship inside the binary, so one that does not parse is a defect of
/// this build and stops it naming the preset.
pub(super) fn preset_palette(name: &str) -> Option<Palette> {
    let name = name.trim().to_ascii_lowercase().replace('_', "-");
    let (preset, text) = PRESET_JSON.iter().find(|(preset, _)| *preset == name)?;
    match parse_theme_json(text) {
        Ok(palette) => Some(palette),
        Err(error) => panic!("the compiled-in theme preset {preset} is invalid: {error}"),
    }
}

/// A color a theme value states: a palette index (a number, or its digits) or
/// a CSS color; `None` when the value is neither, so it may name a var.
fn color_of(value: &Value) -> Result<Option<Color>, String> {
    match value {
        Value::Number(number) => match number.as_u64().map(u8::try_from) {
            Some(Ok(index)) => Ok(Some(Color::AnsiValue(index))),
            Some(Err(cause)) => Err(format!("{number} is not a terminal palette index: {cause}")),
            None => Err(format!("{number} is not a terminal palette index")),
        },
        Value::String(text) => {
            let text = text.trim();
            if !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit()) {
                return match text.parse::<u8>() {
                    Ok(index) => Ok(Some(Color::AnsiValue(index))),
                    Err(error) => Err(format!("{text} is not a terminal palette index: {error}")),
                };
            }
            match csscolorparser::parse(text) {
                Ok(parsed) => {
                    let [r, g, b, _alpha] = parsed.to_rgba8();
                    Ok(Some(Color::Rgb { r, g, b }))
                }
                Err(_not_a_css_color) => Ok(None),
            }
        }
        other => Err(format!("{other} is neither a color nor a var name")),
    }
}

/// The color one role is painted in. A role the theme does not map is looked
/// up by its own name in `vars`; one neither stated nor in `vars` goes
/// uncolored. A value that is neither a color nor a var holding one is refused.
fn role_color(
    name: &str,
    stated: Option<&Value>,
    vars: Option<&serde_json::Map<String, Value>>,
) -> Result<Option<Color>, String> {
    let target = match stated {
        Some(value) => value.clone(),
        None => Value::String(name.to_string()),
    };
    if let Some(color) = color_of(&target).map_err(|error| format!("{name}: {error}"))? {
        return Ok(Some(color));
    }
    let (Some(var), Some(vars)) = (target.as_str(), vars) else {
        return Ok(None);
    };
    match vars.get(var) {
        Some(value) => match color_of(value).map_err(|error| format!("{name} (var {var}): {error}"))? {
            Some(color) => Ok(Some(color)),
            None => Err(format!("{name}: var {var} holds no color: {value}")),
        },
        None if stated.is_some() => Err(format!("{name}: {var} is neither a color nor a var")),
        None => Ok(None),
    }
}

/// A theme document's palette, or why it is no theme.
pub(super) fn parse_theme_json(text: &str) -> Result<Palette, String> {
    let doc: Value = serde_json::from_str(text).map_err(|error| error.to_string())?;
    let vars = doc.get("vars").and_then(Value::as_object);
    let colors = doc.get("colors").and_then(Value::as_object);
    let mut roles = BTreeMap::new();
    for (name, role) in ROLE_NAMES {
        let stated = colors.and_then(|colors| colors.get(*name));
        if let Some(color) = role_color(name, stated, vars)? {
            roles.insert(*role, color);
        }
    }
    Ok(Palette::Colors(roles))
}

pub(super) fn read_theme_json(path: &Path) -> Result<Palette, String> {
    let text = std::fs::read_to_string(path).map_err(|error| error.to_string())?;
    parse_theme_json(&text)
}
