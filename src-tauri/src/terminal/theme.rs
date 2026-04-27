#[derive(Clone, Copy)]
pub struct RenderPalette {
    pub code_keyword: u8,
    pub code_string: u8,
    pub code_number: u8,
    pub code_comment: u8,
    pub code_function: u8,
    pub code_type: u8,
    pub variable: u8,
    pub search_match: u8,
    pub search_current: u8,
    pub primary: u8,
}

impl Default for RenderPalette {
    fn default() -> Self {
        Self::from_values(81, 114, 215, 244, 74, 183, 179, 141, 203)
    }
}

fn normalize_color_scheme(value: &str) -> String {
    value.trim().to_ascii_lowercase().replace(['_', ' '], "-")
}

const ANSI_COLOR_CUBE_LEVELS: [u8; 6] = [0, 95, 135, 175, 215, 255];
const ACCENT_AMBER: &str = "#c7772f";
const ACCENT_SAGE: &str = "#4f8a64";
const ACCENT_ROSE: &str = "#b55a79";
const ACCENT_PLUM: &str = "#7a5fa8";
const ACCENT_COBALT: &str = "#4e7dd6";
const ACCENT_SLATE: &str = "#3e5266";

fn parse_hex_nibble(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}

fn parse_hex_byte_pair(high: u8, low: u8) -> Option<u8> {
    Some((parse_hex_nibble(high)? << 4) | parse_hex_nibble(low)?)
}

fn parse_hex_color(value: &str) -> Option<(u8, u8, u8)> {
    let trimmed = value.trim();
    let hex = trimmed.strip_prefix('#').unwrap_or(trimmed);
    let bytes = hex.as_bytes();
    match bytes.len() {
        3 => {
            let r = parse_hex_nibble(bytes[0])?;
            let g = parse_hex_nibble(bytes[1])?;
            let b = parse_hex_nibble(bytes[2])?;
            Some(((r << 4) | r, (g << 4) | g, (b << 4) | b))
        }
        6 => Some((
            parse_hex_byte_pair(bytes[0], bytes[1])?,
            parse_hex_byte_pair(bytes[2], bytes[3])?,
            parse_hex_byte_pair(bytes[4], bytes[5])?,
        )),
        _ => None,
    }
}

fn rgb_distance_sq(a: (u8, u8, u8), b: (u8, u8, u8)) -> i32 {
    let dr = a.0 as i32 - b.0 as i32;
    let dg = a.1 as i32 - b.1 as i32;
    let db = a.2 as i32 - b.2 as i32;
    dr * dr + dg * dg + db * db
}

fn nearest_cube_component(channel: u8) -> usize {
    let mut best_idx = 0usize;
    let mut best_distance = i32::MAX;
    for (idx, level) in ANSI_COLOR_CUBE_LEVELS.iter().copied().enumerate() {
        let distance = (channel as i32 - level as i32).abs();
        if distance < best_distance {
            best_distance = distance;
            best_idx = idx;
        }
    }
    best_idx
}

fn rgb_to_ansi_256(rgb: (u8, u8, u8)) -> u8 {
    let (r, g, b) = rgb;

    let r_idx = nearest_cube_component(r);
    let g_idx = nearest_cube_component(g);
    let b_idx = nearest_cube_component(b);
    let cube = (
        ANSI_COLOR_CUBE_LEVELS[r_idx],
        ANSI_COLOR_CUBE_LEVELS[g_idx],
        ANSI_COLOR_CUBE_LEVELS[b_idx],
    );
    let cube_idx = 16 + 36 * r_idx + 6 * g_idx + b_idx;

    let gray_avg = ((r as u16 + g as u16 + b as u16) / 3) as i32;
    let gray_step = ((gray_avg - 8) as f32 / 10.0).round() as i32;
    let gray_idx = gray_step.clamp(0, 23) as u8;
    let gray_value = 8 + gray_idx * 10;
    let gray = (gray_value, gray_value, gray_value);
    let gray_palette_idx = 232 + gray_idx as usize;

    if rgb_distance_sq(rgb, gray) < rgb_distance_sq(rgb, cube) {
        gray_palette_idx as u8
    } else {
        cube_idx as u8
    }
}

fn accent_rgb(accent: &str) -> Option<(u8, u8, u8)> {
    let normalized = normalize_color_scheme(accent);
    if normalized.is_empty() || normalized == "auto" {
        return None;
    }

    let token_or_hex = match normalized.as_str() {
        "amber" => ACCENT_AMBER,
        "sage" => ACCENT_SAGE,
        "rose" => ACCENT_ROSE,
        "plum" => ACCENT_PLUM,
        "cobalt" => ACCENT_COBALT,
        "slate" => ACCENT_SLATE,
        _ => normalized.as_str(),
    };
    parse_hex_color(token_or_hex)
}

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
            code_keyword,
            code_string,
            code_number,
            code_comment,
            code_function,
            code_type,
            variable,
            search_match,
            search_current,
            primary: code_keyword,
        }
    }

    pub fn for_color_scheme(color_scheme: &str) -> Self {
        match normalize_color_scheme(color_scheme).as_str() {
            "catppuccin-mocha" => Self::from_values(111, 150, 217, 103, 117, 183, 180, 147, 211),
            "catppuccin-latte" => Self::from_values(33, 29, 166, 102, 25, 61, 130, 69, 160),
            "gruvbox-dark" => Self::from_values(214, 142, 208, 245, 109, 175, 172, 179, 167),
            "gruvbox-light" => Self::from_values(130, 64, 166, 102, 25, 95, 94, 136, 160),
            "dracula" => Self::from_values(177, 114, 221, 103, 117, 183, 222, 141, 204),
            "dark" => Self::from_values(75, 114, 215, 244, 74, 153, 152, 111, 203),
            "white" => Self::from_values(26, 28, 166, 102, 31, 61, 24, 69, 160),
            "solarized-dark" => Self::from_values(136, 64, 166, 102, 37, 61, 109, 144, 166),
            "solarized-light" => Self::from_values(166, 64, 130, 102, 31, 60, 65, 137, 160),
            "nord" => Self::from_values(110, 150, 180, 102, 81, 146, 152, 109, 203),
            "tokyo-night" => Self::from_values(111, 114, 216, 103, 75, 147, 153, 147, 204),
            "one-dark" => Self::from_values(75, 114, 180, 245, 74, 176, 152, 111, 203),
            _ => Self::default(),
        }
    }

    pub fn for_theme(color_scheme: &str, accent: &str) -> Self {
        let mut palette = Self::for_color_scheme(color_scheme);
        if let Some(accent) = accent_rgb(accent) {
            palette.primary = rgb_to_ansi_256(accent);
        }
        palette
    }

    pub fn primary(&self) -> u8 {
        self.primary
    }
}

#[cfg(test)]
mod tests {
    use super::RenderPalette;

    #[test]
    fn render_palette_supports_named_color_schemes() {
        let palette = RenderPalette::for_color_scheme("gruvbox-dark");
        assert_eq!(palette.code_keyword, 214);
        assert_eq!(palette.search_current, 167);

        let normalized = RenderPalette::for_color_scheme("Gruvbox Dark");
        assert_eq!(normalized.code_keyword, 214);
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
        let themed = RenderPalette::for_theme("gruvbox-dark", "auto");
        assert_eq!(themed.primary(), base.primary());
        assert_eq!(themed.code_keyword, base.code_keyword);
    }

    #[test]
    fn render_palette_for_theme_applies_named_accent_to_primary_only() {
        let base = RenderPalette::for_color_scheme("gruvbox-dark");
        let themed = RenderPalette::for_theme("gruvbox-dark", "rose");
        assert_ne!(themed.primary(), base.primary());
        assert_eq!(themed.code_keyword, base.code_keyword);
        assert_eq!(themed.search_match, base.search_match);
    }

    #[test]
    fn render_palette_for_theme_applies_custom_hex_accent() {
        let base = RenderPalette::for_color_scheme("dark");
        let themed = RenderPalette::for_theme("dark", "#4f7bd9");
        assert_ne!(themed.primary(), base.primary());
        assert_eq!(themed.code_keyword, base.code_keyword);
    }
}
