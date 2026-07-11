#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThemeId {
    Auto,
    GraphiteDark,
    PaperLight,
    Mono,
    HighContrast,
    ColorBlind,
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
    CodeAdd,
    CodeRemove,
    Selection,
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

    pub fn prefix(&self) -> &str { &self.prefix }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Theme {
    pub id: ThemeId,
    pub color: bool,
}

impl Theme {
    pub fn from_env(color_requested: bool) -> Self {
        let no_color = std::env::var_os("NO_COLOR").is_some();
        let id = std::env::var("JEDEN_THEME")
            .ok()
            .and_then(|value| ThemeId::parse(&value))
            .unwrap_or(ThemeId::Auto);
        Self { id, color: color_requested && !no_color }
    }

    pub fn resolve(self, token: SemanticColor, mut emphasis: Emphasis) -> ResolvedStyle {
        if !self.color {
            return ResolvedStyle { prefix: String::new() };
        }
        if matches!(self.id, ThemeId::HighContrast) {
            emphasis.dim = false;
        }
        let mut codes: Vec<&str> = Vec::with_capacity(3);
        if emphasis.bold { codes.push("1"); }
        if emphasis.dim && self.color { codes.push("2"); }
        if emphasis.underline { codes.push("4"); }
        if emphasis.reverse { codes.push("7"); }
        if self.color && !matches!(self.id, ThemeId::Mono | ThemeId::HighContrast) {
            if let Some(code) = color_code(self.id, token) { codes.push(code); }
        }
        let prefix = if codes.is_empty() { String::new() } else { format!("\x1b[{}m", codes.join(";")) };
        ResolvedStyle { prefix }
    }

    pub fn paint(self, value: &str, token: SemanticColor, emphasis: Emphasis) -> String {
        self.resolve(token, emphasis).paint(value)
    }
}

impl ThemeId {
    fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "auto" => Some(Self::Auto),
            "graphite-dark" | "graphite_dark" => Some(Self::GraphiteDark),
            "paper-light" | "paper_light" => Some(Self::PaperLight),
            "mono" => Some(Self::Mono),
            "high-contrast" | "high_contrast" => Some(Self::HighContrast),
            "color-blind" | "color_blind" => Some(Self::ColorBlind),
            _ => None,
        }
    }
}

fn color_code(theme: ThemeId, token: SemanticColor) -> Option<&'static str> {
    use SemanticColor::*;
    let code = match theme {
        ThemeId::PaperLight => match token {
            Accent | Warning => "38;5;130",
            TextMuted => "38;5;59",
            Info => "38;5;30",
            Success | CodeAdd => "38;5;28",
            Danger | CodeRemove => "38;5;124",
            Selection => "7",
            TextPrimary => return None,
        },
        ThemeId::GraphiteDark | ThemeId::Auto => match token {
            Accent | Warning => "38;5;215",
            TextMuted => "38;5;245",
            Info => "38;5;109",
            Success | CodeAdd => "38;5;108",
            Danger | CodeRemove => "38;5;174",
            Selection => "7",
            TextPrimary => return None,
        },
        ThemeId::ColorBlind => match token {
            Accent | Warning => "38;5;214",
            TextMuted => "38;5;245",
            Info | Success | CodeAdd => "38;5;39",
            Danger | CodeRemove => "38;5;201",
            Selection => "7",
            TextPrimary => return None,
        },
        ThemeId::Mono | ThemeId::HighContrast => return None,
    };
    Some(code)
}
