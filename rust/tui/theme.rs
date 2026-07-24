//! Declarative themes: named presets plus user-supplied `theme.json` files in
//! the oh-my-pi style — `{ "name": ..., "vars": {role-ish palette}, "colors":
//! {semantic role -> var or color} }`. Semantic roles are the six paintable
//! tokens below; TextPrimary always renders uncolored.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

static PROJECT_CWD: OnceLock<PathBuf> = OnceLock::new();

/// Record the session cwd so `from_env` can discover project-level
/// `.jeden/theme.json` even when it is called from cwd-less contexts.
pub(crate) fn init(cwd: &Path) {
    let _ = PROJECT_CWD.set(cwd.to_path_buf());
}

fn project_theme_file() -> Option<PathBuf> {
    PROJECT_CWD
        .get()
        .map(|cwd| cwd.join(".jeden/theme.json"))
        .filter(|path| path.exists())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SemanticColor {
    TextPrimary,
    TextMuted,
    Accent,
    Info,
    Success,
    Warning,
    Danger,
}

const ROLE_NAMES: [(&str, SemanticColor); 6] = [
    ("muted", SemanticColor::TextMuted),
    ("accent", SemanticColor::Accent),
    ("info", SemanticColor::Info),
    ("success", SemanticColor::Success),
    ("warning", SemanticColor::Warning),
    ("danger", SemanticColor::Danger),
];

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Emphasis {
    pub bold: bool,
    pub dim: bool,
    pub underline: bool,
    pub reverse: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedStyle {
    prefix: String,
}

impl ResolvedStyle {
    pub fn paint(&self, value: &str) -> String {
        if self.prefix.is_empty() {
            value.to_string()
        } else {
            format!("{}{}\x1b[0m", self.prefix, value)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Palette {
    /// ANSI escapes per role index (order of ROLE_NAMES).
    Colors([Option<String>; 6]),
    /// No role colors (mono / high-contrast).
    Plain,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Theme {
    palette: Palette,
    color: bool,
    high_contrast: bool,
}

// ---------------------------------------------------------------------------
// Embedded presets (oh-my-pi compatible palettes)

struct Preset {
    name: &'static str,
    vars: &'static [(&'static str, &'static str)],
    colors: &'static [(&'static str, &'static str)],
}

const PRESETS: &[Preset] = &[
    Preset {
        name: "graphite-dark",
        vars: &[
            ("accent", "38;5;215"),
            ("muted", "38;5;245"),
            ("info", "38;5;109"),
            ("success", "38;5;108"),
            ("warning", "38;5;215"),
            ("danger", "38;5;174"),
        ],
        colors: &[],
    },
    Preset {
        name: "paper-light",
        vars: &[
            ("accent", "38;5;130"),
            ("muted", "38;5;59"),
            ("info", "38;5;30"),
            ("success", "38;5;28"),
            ("warning", "38;5;130"),
            ("danger", "38;5;124"),
        ],
        colors: &[],
    },
    Preset {
        name: "titanium",
        vars: &[
            ("accent", "#00b4ff"),
            ("muted", "#9ca3b0"),
            ("info", "#0082b3"),
            ("success", "#00ff88"),
            ("warning", "#ffb347"),
            ("danger", "#ff4757"),
        ],
        colors: &[],
    },
    Preset {
        name: "nord",
        vars: &[
            ("accent", "#88c0d0"),
            ("muted", "#4c566a"),
            ("info", "#81a1c1"),
            ("success", "#a3be8c"),
            ("warning", "#ebcb8b"),
            ("danger", "#bf616a"),
        ],
        colors: &[],
    },
    Preset {
        name: "color-blind",
        vars: &[
            ("accent", "38;5;214"),
            ("muted", "38;5;245"),
            ("info", "38;5;39"),
            ("success", "38;5;39"),
            ("warning", "38;5;214"),
            ("danger", "38;5;201"),
        ],
        colors: &[],
    },
];

impl Preset {
    fn named(name: &str) -> Option<&'static Preset> {
        let name = name.trim().to_ascii_lowercase();
        PRESETS
            .iter()
            .find(|preset| preset.name == name.replace('_', "-"))
    }
    fn palette(&self) -> Palette {
        let mut roles: [Option<String>; 6] = Default::default();
        for (index, (role, _)) in ROLE_NAMES.iter().enumerate() {
            let var_name = self
                .colors
                .iter()
                .find(|(color_role, _)| *color_role == *role)
                .map(|(_, target)| *target)
                .unwrap_or(*role);
            roles[index] = self
                .vars
                .iter()
                .find(|(name, _)| *name == var_name)
                .and_then(|(_, value)| color_escape(value));
        }
        Palette::Colors(roles)
    }
}

// ---------------------------------------------------------------------------
// Color value parsing: #rrggbb (truecolor), "38;5;N", or bare 256 index.

fn color_escape(value: &str) -> Option<String> {
    let value = value.trim();
    if let Some(hex) = value.strip_prefix('#') {
        if hex.len() != 6 {
            return None;
        }
        let r = u8::from_str_radix(hex.get(0..2)?, 16).ok()?;
        let g = u8::from_str_radix(hex.get(2..4)?, 16).ok()?;
        let b = u8::from_str_radix(hex.get(4..6)?, 16).ok()?;
        return Some(format!("38;2;{r};{g};{b}"));
    }
    if value.starts_with("38;5;") {
        return value
            .strip_prefix("38;5;")
            .and_then(|index| index.parse::<u8>().ok())
            .map(|_| value.to_string());
    }
    value.parse::<u8>().ok().map(|_| format!("38;5;{value}"))
}

// ---------------------------------------------------------------------------
// theme.json loading

fn home_theme_file() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|home| Path::new(&home).join(".jeden/theme.json"))
}

fn read_theme_json(path: &Path) -> Result<Palette, String> {
    let text = std::fs::read_to_string(path).map_err(|error| error.to_string())?;
    let doc: serde_json::Value = serde_json::from_str(&text).map_err(|error| error.to_string())?;
    let vars = doc.get("vars").and_then(serde_json::Value::as_object);
    let colors = doc.get("colors").and_then(serde_json::Value::as_object);
    let mut roles: [Option<String>; 6] = Default::default();
    for (index, (role, _)) in ROLE_NAMES.iter().enumerate() {
        let target = colors
            .and_then(|colors| colors.get(*role))
            .and_then(serde_json::Value::as_str)
            .unwrap_or(*role);
        roles[index] = color_escape(target).or_else(|| {
            vars.and_then(|vars| vars.get(target))
                .and_then(serde_json::Value::as_str)
                .and_then(color_escape)
        });
    }
    Ok(Palette::Colors(roles))
}

// ---------------------------------------------------------------------------
// Public API (unchanged surface)

pub struct ThemeId;

impl ThemeId {
    pub const AUTO: &'static str = "auto";
}

impl Theme {
    /// Resolve the effective theme. Precedence: `JEDEN_THEME` (preset name or a
    /// path to a theme.json) > project `.jeden/theme.json` > user
    /// `~/.jeden/theme.json` > `ui.theme` config > auto (graphite-dark).
    pub fn from_env(color_requested: bool) -> Self {
        let no_color = std::env::var_os("NO_COLOR").is_some();
        let color = color_requested && !no_color;

        if let Ok(value) = std::env::var("JEDEN_THEME") {
            let value = value.trim().to_string();
            if let Some(theme) = Self::from_value(&value, color) {
                return theme;
            }
        }
        for candidate in [project_theme_file(), home_theme_file()]
            .into_iter()
            .flatten()
        {
            if candidate.exists() {
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
        if let Some(theme) = Self::from_value(&crate::cli::config::ui_theme(), color) {
            return theme;
        }
        Self::preset("graphite-dark", color)
    }

    fn from_value(value: &str, color: bool) -> Option<Self> {
        if value.is_empty() || value == ThemeId::AUTO {
            return Some(Self::preset("graphite-dark", color));
        }
        let path = Path::new(value);
        if value.ends_with(".json") && path.exists() {
            return read_theme_json(path).ok().map(|palette| Self {
                palette,
                color,
                high_contrast: false,
            });
        }
        match value {
            "mono" | "high-contrast" | "high_contrast" => Some(Self {
                palette: Palette::Plain,
                color,
                high_contrast: true,
            }),
            "custom" => None,
            name => Preset::named(name).map(|preset| Self {
                palette: preset.palette(),
                color,
                high_contrast: false,
            }),
        }
    }

    fn preset(name: &str, color: bool) -> Self {
        Preset::named(name)
            .map(|preset| Self {
                palette: preset.palette(),
                color,
                high_contrast: false,
            })
            .unwrap_or(Self {
                palette: Palette::Plain,
                color,
                high_contrast: true,
            })
    }

    pub fn resolve(self, token: SemanticColor, mut emphasis: Emphasis) -> ResolvedStyle {
        if !self.color {
            return ResolvedStyle {
                prefix: String::new(),
            };
        }
        if self.high_contrast {
            emphasis.dim = false;
        }
        let mut codes: Vec<&str> = Vec::with_capacity(3);
        if emphasis.bold {
            codes.push("1");
        }
        if emphasis.dim && self.color && !self.high_contrast {
            codes.push("2");
        }
        if emphasis.underline {
            codes.push("4");
        }
        if emphasis.reverse {
            codes.push("7");
        }
        let mut prefix = if codes.is_empty() {
            String::new()
        } else {
            format!("\x1b[{}m", codes.join(";"))
        };
        if !self.high_contrast {
            if let (Palette::Colors(roles), Some(index)) = (
                &self.palette,
                ROLE_NAMES.iter().position(|(_, role)| *role == token),
            ) {
                if let Some(Some(escape)) = roles.get(index) {
                    if prefix.is_empty() {
                        prefix = format!("\x1b[{escape}m");
                    } else {
                        prefix = format!("{};{}", prefix.trim_end_matches('m'), escape);
                        prefix.push('m');
                    }
                }
            }
        }
        ResolvedStyle { prefix }
    }

    pub fn paint(self, value: &str, token: SemanticColor, emphasis: Emphasis) -> String {
        self.resolve(token, emphasis).paint(value)
    }
}
