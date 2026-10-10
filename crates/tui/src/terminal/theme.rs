use app_core::config::{ThemeColor, ThemeColors};
use ratatui::style::Color;

/// Colours are 256-colour indexes from the built-in schemes, or exact RGB
/// and `Color::Reset` (the terminal's own) from `[theme.colors]`.
#[derive(Clone, Copy)]
pub struct RenderPalette {
    pub code_keyword: Color,
    pub code_string: Color,
    pub code_number: Color,
    pub code_comment: Color,
    pub code_function: Color,
    pub code_type: Color,
    pub variable: Color,
    pub search_match: Color,
    pub search_current: Color,
    pub primary: Color,
    pub surface_bg: Color,
    pub text_fg: Color,
    pub code_block_bg: Color,
    /// Set by `[theme.colors]`; otherwise derived from `surface_bg`.
    pub selection_bg: Option<Color>,
}

impl Default for RenderPalette {
    fn default() -> Self {
        Self::from_values(81, 114, 215, 244, 74, 183, 179, 141, 203)
    }
}

use app_core::theme::{
    accent_rgb, normalize_color_scheme, rgb_to_ansi_256, scheme_accent_rgb, scheme_indexes,
    scheme_is_light, scheme_surface_bg_rgb, scheme_text_fg_rgb,
};

impl RenderPalette {
    const fn from_values(
        code_keyword: u8,
        code_string: u8,
        code_number: u8,
        code_comment: u8,
        code_function: u8,
        code_type: u8,
        variable: u8,
        search_match: u8,
        search_current: u8,
    ) -> Self {
        Self {
            code_keyword: Color::Indexed(code_keyword),
            code_string: Color::Indexed(code_string),
            code_number: Color::Indexed(code_number),
            code_comment: Color::Indexed(code_comment),
            code_function: Color::Indexed(code_function),
            code_type: Color::Indexed(code_type),
            variable: Color::Indexed(variable),
            search_match: Color::Indexed(search_match),
            search_current: Color::Indexed(search_current),
            primary: Color::Indexed(code_keyword),
            surface_bg: Color::Indexed(236),
            text_fg: Color::Indexed(231),
            code_block_bg: Color::Indexed(237),
            selection_bg: None,
        }
    }

    pub fn for_color_scheme(color_scheme: &str) -> Self {
        let normalized = normalize_color_scheme(color_scheme);
        let i = scheme_indexes(&normalized);
        let mut palette = Self::from_values(
            i.code_keyword,
            i.code_string,
            i.code_number,
            i.code_comment,
            i.code_function,
            i.code_type,
            i.variable,
            i.search_match,
            i.search_current,
        );
        if let Some(accent) = scheme_accent_rgb(&normalized) {
            palette.primary = Color::Indexed(rgb_to_ansi_256(accent));
        }
        if let Some(surface_bg) = scheme_surface_bg_rgb(&normalized) {
            palette.surface_bg = Color::Indexed(rgb_to_ansi_256(surface_bg));
        }
        if let Some(text_fg) = scheme_text_fg_rgb(&normalized) {
            palette.text_fg = Color::Indexed(rgb_to_ansi_256(text_fg));
        }
        palette.code_block_bg = Color::Indexed(if scheme_is_light(&normalized) {
            252
        } else {
            237
        });
        palette
    }

    pub fn for_theme(color_scheme: &str, accent: &str, colors: &ThemeColors) -> Self {
        let mut palette = Self::for_color_scheme(color_scheme);
        if let Some(accent) = accent_rgb(accent) {
            palette.primary = Color::Indexed(rgb_to_ansi_256(accent));
        }
        palette.apply(colors);
        palette
    }

    /// Replaces the colours `[theme.colors]` sets.
    fn apply(&mut self, colors: &ThemeColors) {
        let to_color = |color: ThemeColor| match color {
            ThemeColor::Rgb(r, g, b) => Color::Rgb(r, g, b),
            ThemeColor::Terminal => Color::Reset,
        };
        for (slot, color) in [
            (&mut self.text_fg, colors.foreground),
            (&mut self.surface_bg, colors.background),
            (&mut self.primary, colors.accent),
            (&mut self.code_block_bg, colors.code_block_background),
            (&mut self.code_keyword, colors.keyword),
            (&mut self.code_string, colors.string),
            (&mut self.code_number, colors.number),
            (&mut self.code_comment, colors.comment),
            (&mut self.code_function, colors.function),
            (&mut self.code_type, colors.type_),
            (&mut self.variable, colors.variable),
            (&mut self.search_match, colors.search_match),
            (&mut self.search_current, colors.search_current),
        ] {
            if let Some(color) = color {
                *slot = to_color(color);
            }
        }
        if let Some(color) = colors.selection_background {
            self.selection_bg = Some(to_color(color));
        }
    }

    pub fn primary(&self) -> Color {
        self.primary
    }

    pub fn surface_bg(&self) -> Color {
        self.surface_bg
    }

    pub fn text_fg(&self) -> Color {
        self.text_fg
    }
}

#[cfg(test)]
mod tests {
    use super::{Color, RenderPalette, ThemeColor, ThemeColors};

    #[test]
    fn render_palette_supports_named_color_schemes() {
        let palette = RenderPalette::for_color_scheme("gruvbox-dark");
        assert_eq!(palette.code_keyword, Color::Indexed(214));
        assert_eq!(palette.search_current, Color::Indexed(167));

        let normalized = RenderPalette::for_color_scheme("Gruvbox Dark");
        assert_eq!(normalized.code_keyword, Color::Indexed(214));
    }

    #[test]
    fn render_palette_falls_back_to_default_for_unknown_scheme() {
        let palette = RenderPalette::for_color_scheme("unknown-scheme");
        let default_palette = RenderPalette::default();
        assert_eq!(palette.code_keyword, default_palette.code_keyword);
        assert_eq!(palette.search_match, default_palette.search_match);
    }

    #[test]
    fn render_palette_for_theme_keeps_auto_accent_primary() {
        let base = RenderPalette::for_color_scheme("gruvbox-dark");
        let themed = RenderPalette::for_theme("gruvbox-dark", "auto", &ThemeColors::default());
        assert_eq!(themed.primary(), base.primary());
        assert_eq!(themed.code_keyword, base.code_keyword);
    }

    #[test]
    fn render_palette_for_theme_applies_named_accent_to_primary_only() {
        let base = RenderPalette::for_color_scheme("gruvbox-dark");
        let themed = RenderPalette::for_theme("gruvbox-dark", "rose", &ThemeColors::default());
        assert_ne!(themed.primary(), base.primary());
        assert_eq!(themed.code_keyword, base.code_keyword);
        assert_eq!(themed.search_match, base.search_match);
        assert_eq!(themed.surface_bg(), base.surface_bg());
    }

    #[test]
    fn render_palette_for_theme_applies_custom_hex_accent() {
        let base = RenderPalette::for_color_scheme("dark");
        let themed = RenderPalette::for_theme("dark", "#4f7bd9", &ThemeColors::default());
        assert_ne!(themed.primary(), base.primary());
        assert_eq!(themed.code_keyword, base.code_keyword);
    }

    #[test]
    fn render_palette_for_theme_falls_back_to_scheme_accent_when_accent_is_empty_or_invalid() {
        let base = RenderPalette::for_color_scheme("gruvbox-light");
        let empty = RenderPalette::for_theme("gruvbox-light", "", &ThemeColors::default());
        let invalid =
            RenderPalette::for_theme("gruvbox-light", "not-a-color", &ThemeColors::default());
        assert_eq!(empty.primary(), base.primary());
        assert_eq!(invalid.primary(), base.primary());
        assert_eq!(empty.surface_bg(), base.surface_bg());
        assert_eq!(invalid.surface_bg(), base.surface_bg());
    }

    #[test]
    fn theme_colors_replace_only_what_they_set() {
        let base = RenderPalette::for_color_scheme("nord");
        let colors = ThemeColors {
            foreground: Some(ThemeColor::Rgb(0xec, 0xef, 0xf4)),
            background: Some(ThemeColor::Terminal),
            keyword: Some(ThemeColor::Rgb(0x81, 0xa1, 0xc1)),
            selection_background: Some(ThemeColor::Rgb(0x43, 0x4c, 0x5e)),
            ..ThemeColors::default()
        };
        let themed = RenderPalette::for_theme("nord", "rose", &colors);
        assert_eq!(themed.text_fg(), Color::Rgb(0xec, 0xef, 0xf4));
        assert_eq!(themed.surface_bg(), Color::Reset);
        assert_eq!(themed.code_keyword, Color::Rgb(0x81, 0xa1, 0xc1));
        assert_eq!(themed.selection_bg, Some(Color::Rgb(0x43, 0x4c, 0x5e)));
        assert_eq!(themed.code_string, base.code_string);
        assert_eq!(themed.code_block_bg, base.code_block_bg);
        assert_ne!(themed.primary(), base.primary(), "accent still applies");
        assert_eq!(base.selection_bg, None);
    }

    #[test]
    fn render_palette_uses_light_surface_background_for_light_schemes() {
        use crate::terminal::canvas::contrast_fg_for_bg;

        let light = RenderPalette::for_color_scheme("gruvbox-light");
        let dark = RenderPalette::for_color_scheme("gruvbox-dark");
        assert_eq!(contrast_fg_for_bg(light.surface_bg()), Color::Indexed(16));
        assert_eq!(contrast_fg_for_bg(dark.surface_bg()), Color::Indexed(231));
    }
}
