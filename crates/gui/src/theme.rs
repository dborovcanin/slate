//! Colors from the design canvas, and the mapping from the session's semantic
//! styles to GPUI text styles. Semantics stay in `note_session::display`; this
//! file only decides what each role looks like.
use app_core::config::{ThemeColor, ThemeConfig};
use app_core::theme as scheme;
use gpui::{
    px, rgb, FontStyle, FontWeight, HighlightStyle, Hsla, StrikethroughStyle, UnderlineStyle,
};
use note_session::display::semantic::{SemanticRole, SemanticStyle};
use serde::{Deserialize, Serialize};

/// Where the colours come from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThemeMode {
    /// `[theme]` in the config file, as in the terminal app.
    #[default]
    Config,
    /// The built-in dark and light looks of the design.
    Dark,
    Light,
}

#[derive(Clone, Copy)]
pub struct Theme {
    pub bg: Hsla,
    pub panel: Hsla,
    pub border: Hsla,
    pub text: Hsla,
    pub heading: Hsla,
    pub muted: Hsla,
    pub faint: Hsla,
    /// Calc results and formula values.
    pub amber: Hsla,
    /// Variables, links and the Normal mode pill.
    pub blue: Hsla,
    pub chip: Hsla,
    pub active: Hsla,
    pub on_accent: Hsla,
    pub code_bg: Hsla,
    pub keyword: Hsla,
    pub string: Hsla,
    pub comment: Hsla,
    pub number: Hsla,
    pub function: Hsla,
    pub type_: Hsla,
    pub variable: Hsla,
    pub search_match: Hsla,
    pub search_current: Hsla,
    /// Selected text background.
    pub selection: Hsla,
    /// Menu and popup panels and their border.
    pub menu: Hsla,
    pub menu_border: Hsla,
}

fn c(hex: u32) -> Hsla {
    rgb(hex).into()
}

fn from_rgb((r, g, b): (u8, u8, u8)) -> Hsla {
    c(u32::from(r) << 16 | u32::from(g) << 8 | u32::from(b))
}

/// `a` towards `b` by `amount`, in RGB.
fn mix(a: Hsla, b: Hsla, amount: f32) -> Hsla {
    let (a, b) = (a.to_rgb(), b.to_rgb());
    gpui::Rgba {
        r: a.r + (b.r - a.r) * amount,
        g: a.g + (b.g - a.g) * amount,
        b: a.b + (b.b - a.b) * amount,
        a: 1.0,
    }
    .into()
}

impl Theme {
    pub fn dark() -> Self {
        Self {
            bg: c(0x15171a),
            panel: c(0x1b1e22),
            border: c(0x2a2e35),
            text: c(0xe4e2dc),
            heading: c(0xf3f1ea),
            muted: c(0x9aa0a8),
            faint: c(0x6b717a),
            amber: c(0xe3a857),
            blue: c(0x7fa6d9),
            chip: c(0x262a30),
            active: c(0x252b33),
            on_accent: c(0x11161d),
            code_bg: c(0x1b1e22),
            keyword: c(0xc792ea),
            string: c(0xa5c58a),
            comment: c(0x6b717a),
            number: c(0xe3a857),
            function: c(0xe3a857),
            type_: c(0xc792ea),
            variable: c(0x7fa6d9),
            search_match: c(0xe3a857),
            search_current: c(0xe3a857),
            selection: c(0x7fa6d9).opacity(0.32),
            menu: c(0x20242a),
            menu_border: c(0x363b44),
        }
    }

    pub fn light() -> Self {
        Self {
            bg: c(0xfbfaf7),
            panel: c(0xf2f0eb),
            border: c(0xdedad2),
            text: c(0x23262b),
            heading: c(0x111317),
            muted: c(0x555b64),
            faint: c(0x7d828a),
            amber: c(0x94560a),
            blue: c(0x2c5c99),
            chip: c(0xe7e4dd),
            active: c(0xe4e9f0),
            on_accent: c(0xffffff),
            code_bg: c(0xf2f0eb),
            keyword: c(0x7a3e9d),
            string: c(0x3f6b21),
            comment: c(0x7d828a),
            number: c(0x94560a),
            function: c(0x94560a),
            type_: c(0x7a3e9d),
            variable: c(0x2c5c99),
            search_match: c(0x94560a),
            search_current: c(0x94560a),
            selection: c(0x2c5c99).opacity(0.32),
            menu: c(0xffffff),
            menu_border: c(0xd9d5cc),
        }
    }

    /// The colours of `[theme]` in the config: the scheme, the accent and
    /// `[theme.colors]`, the same inputs the terminal app uses.
    pub fn from_config(config: &ThemeConfig) -> Self {
        let name = scheme::normalize_color_scheme(&config.color_scheme);
        let light = scheme::scheme_is_light(&name);
        let mut t = if light { Self::light() } else { Self::dark() };
        let idx = scheme::scheme_indexes(&name);
        let index = |i: u8| from_rgb(scheme::ansi_256_to_rgb(i));
        let known = scheme::scheme_surface_bg_rgb(&name).is_some();
        let colors = &config.colors;
        let over = |color: Option<ThemeColor>| match color {
            Some(ThemeColor::Rgb(r, g, b)) => Some(from_rgb((r, g, b))),
            // `none` means the terminal's own colour; a window has none.
            _ => None,
        };
        if known {
            t.keyword = index(idx.code_keyword);
            t.string = index(idx.code_string);
            t.number = index(idx.code_number);
            t.comment = index(idx.code_comment);
            t.function = index(idx.code_function);
            t.type_ = index(idx.code_type);
            t.variable = index(idx.variable);
            t.search_match = index(idx.search_match);
            t.search_current = index(idx.search_current);
            t.amber = t.number;
            if let Some(rgb) = scheme::scheme_surface_bg_rgb(&name) {
                t.bg = from_rgb(rgb);
            }
            if let Some(rgb) = scheme::scheme_text_fg_rgb(&name) {
                t.text = from_rgb(rgb);
            }
            if let Some(rgb) = scheme::scheme_accent_rgb(&name) {
                t.blue = from_rgb(rgb);
            }
        }
        if let Some(rgb) = scheme::accent_rgb(&config.accent) {
            t.blue = from_rgb(rgb);
        }
        for (slot, color) in [
            (&mut t.text, colors.foreground),
            (&mut t.bg, colors.background),
            (&mut t.blue, colors.accent),
            (&mut t.keyword, colors.keyword),
            (&mut t.string, colors.string),
            (&mut t.number, colors.number),
            (&mut t.comment, colors.comment),
            (&mut t.function, colors.function),
            (&mut t.type_, colors.type_),
            (&mut t.variable, colors.variable),
            (&mut t.search_match, colors.search_match),
            (&mut t.search_current, colors.search_current),
        ] {
            if let Some(color) = over(color) {
                *slot = color;
            }
        }
        if colors.number.is_some() {
            t.amber = t.number;
        }
        t.derive_surfaces(light);
        t.code_bg = over(colors.code_block_background).unwrap_or(mix(t.bg, t.text, 0.07));
        t.selection = over(colors.selection_background).unwrap_or(t.blue.opacity(0.32));
        t
    }

    /// Panels, rules and muted text follow from the background, text and
    /// accent, so any scheme or custom colours stay readable.
    fn derive_surfaces(&mut self, light: bool) {
        let (bg, text) = (self.bg, self.text);
        self.panel = mix(bg, text, 0.05);
        self.border = mix(bg, text, 0.14);
        self.chip = mix(bg, text, 0.1);
        self.active = mix(bg, self.blue, 0.2);
        self.muted = mix(text, bg, 0.38);
        self.faint = mix(text, bg, 0.6);
        self.heading = mix(text, if light { c(0x000000) } else { c(0xffffff) }, 0.2);
        // Popups sit a step above the panels: brighter than the editor on
        // dark schemes, near-white on light ones.
        self.menu = if light {
            mix(bg, c(0xffffff), 0.7)
        } else {
            mix(bg, text, 0.09)
        };
        self.menu_border = mix(self.menu, text, 0.18);
        self.on_accent = if self.blue.l > 0.6 {
            c(0x11161d)
        } else {
            c(0xffffff)
        };
    }

    pub fn for_mode(mode: ThemeMode, config: &ThemeConfig) -> Self {
        match mode {
            ThemeMode::Config => Self::from_config(config),
            ThemeMode::Dark => Self::dark(),
            ThemeMode::Light => Self::light(),
        }
    }

    pub fn role(&self, role: SemanticRole) -> Hsla {
        match role {
            SemanticRole::Text | SemanticRole::Surface => self.text,
            SemanticRole::Heading => self.heading,
            SemanticRole::HiddenMarker => self.faint,
            SemanticRole::CalcResult => self.amber,
            SemanticRole::CodeBlock => self.code_bg,
            SemanticRole::Variable => self.variable,
            SemanticRole::Primary => self.blue,
            SemanticRole::CodeKeyword => self.keyword,
            SemanticRole::CodeType => self.type_,
            SemanticRole::CodeString => self.string,
            SemanticRole::CodeNumber => self.number,
            SemanticRole::CodeFunction => self.function,
            SemanticRole::CodeComment => self.comment,
            SemanticRole::SearchMatch => self.search_match,
            SemanticRole::SearchCurrent => self.search_current,
        }
    }

    /// Text style for one run. Surface backgrounds are left to the line.
    pub fn highlight(&self, style: SemanticStyle) -> HighlightStyle {
        let mut color = style.fg.map(|r| self.role(r)).unwrap_or(self.text);
        if style.dim {
            color = self.faint;
        }
        let background_color = match style.bg {
            Some(SemanticRole::Surface) | None => None,
            Some(SemanticRole::CodeBlock) => None,
            Some(role) => Some(self.role(role).opacity(0.25)),
        };
        HighlightStyle {
            color: Some(color),
            font_weight: style.bold.then_some(FontWeight::SEMIBOLD),
            font_style: style.italic.then_some(FontStyle::Italic),
            background_color,
            underline: style.underline.then(|| UnderlineStyle {
                thickness: px(1.0),
                color: Some(color),
                wavy: false,
            }),
            strikethrough: style.strikethrough.then(|| StrikethroughStyle {
                thickness: px(1.0),
                color: Some(color),
            }),
            fade_out: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use app_core::config::ThemeColors;

    fn config(scheme: &str, accent: &str, colors: ThemeColors) -> ThemeConfig {
        ThemeConfig {
            color_scheme: scheme.into(),
            accent: accent.into(),
            colors,
            ..ThemeConfig::default()
        }
    }

    fn rgb_of(h: Hsla) -> (u8, u8, u8) {
        let c = h.to_rgb();
        (
            (c.r * 255.0).round() as u8,
            (c.g * 255.0).round() as u8,
            (c.b * 255.0).round() as u8,
        )
    }

    #[test]
    fn schemes_set_surface_text_and_accent() {
        let t = Theme::from_config(&config("Gruvbox Dark", "auto", Default::default()));
        assert_eq!(rgb_of(t.bg), (0x32, 0x30, 0x2f));
        assert_eq!(rgb_of(t.text), (0xeb, 0xdb, 0xb2));
        assert_eq!(rgb_of(t.blue), (0xfa, 0xbd, 0x2f));
        let light = Theme::from_config(&config("gruvbox-light", "auto", Default::default()));
        assert!(light.bg.l > 0.7 && t.bg.l < 0.3);
    }

    #[test]
    fn accent_and_theme_colors_override_in_order() {
        let colors = ThemeColors {
            background: Some(ThemeColor::Rgb(1, 2, 3)),
            keyword: Some(ThemeColor::Rgb(9, 8, 7)),
            accent: None,
            foreground: Some(ThemeColor::Terminal),
            ..ThemeColors::default()
        };
        let t = Theme::from_config(&config("nord", "#102030", colors));
        assert_eq!(rgb_of(t.bg), (1, 2, 3));
        assert_eq!(rgb_of(t.keyword), (9, 8, 7));
        assert_eq!(rgb_of(t.blue), (0x10, 0x20, 0x30));
        // `none` has no meaning in a window: the scheme's text stays.
        assert_eq!(rgb_of(t.text), (0xe5, 0xe9, 0xf0));
    }

    #[test]
    fn unknown_schemes_keep_the_built_in_dark_look() {
        let t = Theme::from_config(&config("nope", "auto", Default::default()));
        assert_eq!(rgb_of(t.bg), (0x15, 0x17, 0x1a));
    }
}
