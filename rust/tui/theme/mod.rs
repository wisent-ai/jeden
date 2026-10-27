//! Declarative themes: named presets plus user-supplied `theme.json` files in
//! the oh-my-pi style — `{ "name": ..., "vars": {role-ish palette}, "colors":
//! {semantic role -> var or color} }`. Semantic roles are the six paintable
//! tokens below; TextPrimary always renders uncolored. Styles are crossterm's,
//! so this module writes no escape sequence of its own.

mod palette;

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use crossterm::style::{Attribute, ContentStyle, Stylize};
use palette::{preset_palette, read_theme_json, Palette};
pub(crate) use palette::preset_names;

static PROJECT_CWD: OnceLock<PathBuf> = OnceLock::new();

/// Record the session cwd so `from_env` can discover project-level
/// `.jeden/theme.json` even when it is called from cwd-less contexts. The
/// first session's cwd stands for the process.
pub(crate) fn init(cwd: &Path) {
    PROJECT_CWD.get_or_init(|| cwd.to_path_buf());
}

fn project_theme_file() -> Option<PathBuf> {
    PROJECT_CWD.get().map(|cwd| cwd.join(".jeden/theme.json"))
}

/// The user's theme under the one home lookup; with HOME unset it was skipped
/// and the user's colours silently replaced by the defaults.
fn home_theme_file() -> Option<PathBuf> {
    Some(crate::slash::common::dirs_home().join(".jeden/theme.json"))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SemanticColor {
    TextPrimary,
    TextMuted,
    Accent,
    Info,
    Success,
    Warning,
    Danger,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Emphasis {
    pub bold: bool,
    pub dim: bool,
    pub underline: bool,
    pub reverse: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedStyle {
    style: ContentStyle,
}

impl ResolvedStyle {
    pub fn paint(&self, value: &str) -> String {
        self.style.apply(value).to_string()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Theme {
    palette: Palette,
    color: bool,
    high_contrast: bool,
}

/// The preset theme `auto` and an unchosen theme stand for.
const DEFAULT_PRESET: &str = "graphite-dark";

pub struct ThemeId;

impl ThemeId {
    pub const AUTO: &'static str = "auto";
}

impl Theme {
    /// Resolve the effective theme. Precedence: `JEDEN_THEME` (preset name or a
    /// path to a theme.json) > project `.jeden/theme.json` > user
    /// `~/.jeden/theme.json` > `ui.theme` config > auto (graphite-dark). A
    /// choice that cannot be used is said on stderr, naming it, before the next.
    pub fn from_env(color_requested: bool) -> Self {
        let no_color = std::env::var_os("NO_COLOR").is_some();
        let color = color_requested && !no_color;

        match std::env::var("JEDEN_THEME") {
            Ok(value) => match Self::from_value(value.trim(), color) {
                Ok(Some(theme)) => return theme,
                Ok(None) => {}
                Err(error) => eprintln!("jeden: ignoring JEDEN_THEME={value:?}: {error}"),
            },
            Err(std::env::VarError::NotPresent) => {}
            Err(error) => eprintln!("jeden: ignoring JEDEN_THEME: {error}"),
        }
        for candidate in [project_theme_file(), home_theme_file()]
            .into_iter()
            .flatten()
        {
            // A theme file that cannot be examined is said like one that
            // cannot be read, never passed over as absent.
            let present = match crate::tool_runtime::runtime_ops::platform::presence::present(&candidate) {
                Ok(present) => present,
                Err(error) => {
                    eprintln!("jeden: ignoring theme file {}: {error}", candidate.display());
                    false
                }
            };
            if present {
                match read_theme_json(&candidate) {
                    Ok(palette) => {
                        return Self {
                            palette,
                            color,
                            high_contrast: false,
                        }
                    }
                    Err(error) => {
                        eprintln!(
                            "jeden: ignoring invalid theme file {}: {error}",
                            candidate.display()
                        );
                    }
                }
            }
        }
        let configured = crate::cli::config::ui_theme();
        match Self::from_value(&configured, color) {
            Ok(Some(theme)) => return theme,
            Ok(None) => {}
            Err(error) => eprintln!("jeden: ignoring ui.theme {configured:?}: {error}"),
        }
        Self::preset(DEFAULT_PRESET, color)
    }

    /// The theme `value` chooses; `None` for `custom`, which defers to the
    /// theme files; a name or file that is no theme is refused naming it.
    fn from_value(value: &str, color: bool) -> Result<Option<Self>, String> {
        if value.is_empty() || value == ThemeId::AUTO {
            return Ok(Some(Self::preset(DEFAULT_PRESET, color)));
        }
        let path = Path::new(value);
        if value.ends_with(".json") && path.exists() {
            let palette = read_theme_json(path)?;
            return Ok(Some(Self {
                palette,
                color,
                high_contrast: false,
            }));
        }
        match value {
            "mono" | "high-contrast" | "high_contrast" => Ok(Some(Self {
                palette: Palette::Plain,
                color,
                high_contrast: true,
            })),
            "custom" => Ok(None),
            name => match preset_palette(name) {
                Some(palette) => Ok(Some(Self {
                    palette,
                    color,
                    high_contrast: false,
                })),
                None => Err(format!("no theme preset or theme file is named {name}")),
            },
        }
    }

    /// A compiled-in preset by name; the names this module passes are its own.
    fn preset(name: &str, color: bool) -> Self {
        match preset_palette(name) {
            Some(palette) => Self {
                palette,
                color,
                high_contrast: false,
            },
            None => panic!("no compiled-in theme preset is named {name}"),
        }
    }

    pub fn resolve(self, token: SemanticColor, mut emphasis: Emphasis) -> ResolvedStyle {
        let mut style = ContentStyle::new();
        if !self.color {
            return ResolvedStyle { style };
        }
        if self.high_contrast {
            emphasis.dim = false;
        }
        if emphasis.bold {
            style = style.attribute(Attribute::Bold);
        }
        if emphasis.dim {
            style = style.attribute(Attribute::Dim);
        }
        if emphasis.underline {
            style = style.attribute(Attribute::Underlined);
        }
        if emphasis.reverse {
            style = style.attribute(Attribute::Reverse);
        }
        if let (false, Palette::Colors(roles)) = (self.high_contrast, &self.palette) {
            if let Some(color) = roles.get(&token) {
                style = style.with(*color);
            }
        }
        ResolvedStyle { style }
    }

    pub fn paint(self, value: &str, token: SemanticColor, emphasis: Emphasis) -> String {
        self.resolve(token, emphasis).paint(value)
    }
}
