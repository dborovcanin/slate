//! Color schemes shared by the terminal and desktop apps: names, accents,
//! surface and text colours, and the 256-colour indexes the code and search
//! colours are defined by. Each front end turns these into its own colours.

pub fn normalize_color_scheme(value: &str) -> String {
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

pub fn parse_hex_color(value: &str) -> Option<(u8, u8, u8)> {
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

pub fn rgb_to_ansi_256(rgb: (u8, u8, u8)) -> u8 {
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

pub fn accent_rgb(accent: &str) -> Option<(u8, u8, u8)> {
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

pub fn scheme_accent_rgb(color_scheme: &str) -> Option<(u8, u8, u8)> {
    let hex = match normalize_color_scheme(color_scheme).as_str() {
        "slate" | "slate-light" => "#4f7bd9",
        "slate-dark" => "#3e5266",
        "catppuccin-mocha" => "#89b4fa",
        "catppuccin-latte" => "#1e66f5",
        "gruvbox-dark" => "#fabd2f",
        "gruvbox-light" => "#d65d0e",
        "dracula" => "#bd93f9",
        "dark" => "#4aa8ff",
        "white" => "#005cc5",
        "solarized-dark" => "#b58900",
        "solarized-light" => "#cb4b16",
        "nord" => "#88c0d0",
        "tokyo-night" => "#7aa2f7",
        "one-dark" => "#61afef",
        _ => return None,
    };
    parse_hex_color(hex)
}

pub fn scheme_surface_bg_rgb(color_scheme: &str) -> Option<(u8, u8, u8)> {
    let hex = match normalize_color_scheme(color_scheme).as_str() {
        "slate" | "slate-light" => "#f8f1e6",
        "slate-dark" => "#1d1c20",
        "catppuccin-mocha" => "#252536",
        "catppuccin-latte" => "#e6e9ef",
        "gruvbox-dark" => "#32302f",
        "gruvbox-light" => "#f2e5bc",
        "dracula" => "#313442",
        "dark" => "#1a1d23",
        "white" => "#f8f9fb",
        "solarized-dark" => "#073642",
        "solarized-light" => "#f5efdd",
        "nord" => "#3b4252",
        "tokyo-night" => "#212433",
        "one-dark" => "#2f343f",
        _ => return None,
    };
    parse_hex_color(hex)
}

pub fn scheme_text_fg_rgb(color_scheme: &str) -> Option<(u8, u8, u8)> {
    let hex = match normalize_color_scheme(color_scheme).as_str() {
        "slate" | "slate-light" => "#1f1b16",
        "slate-dark" => "#ece8e0",
        "catppuccin-mocha" => "#cdd6f4",
        "catppuccin-latte" => "#4c4f69",
        "gruvbox-dark" => "#ebdbb2",
        "gruvbox-light" => "#3c3836",
        "dracula" => "#f8f8f2",
        "dark" => "#e6edf3",
        "white" => "#1f2933",
        "solarized-dark" => "#93a1a1",
        "solarized-light" => "#586e75",
        "nord" => "#e5e9f0",
        "tokyo-night" => "#c0caf5",
        "one-dark" => "#abb2bf",
        _ => return None,
    };
    parse_hex_color(hex)
}

/// Colours of the code, variable and search roles, as xterm 256-colour
/// indexes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SchemeIndexes {
    pub code_keyword: u8,
    pub code_string: u8,
    pub code_number: u8,
    pub code_comment: u8,
    pub code_function: u8,
    pub code_type: u8,
    pub variable: u8,
    pub search_match: u8,
    pub search_current: u8,
}

impl SchemeIndexes {
    const fn new(values: [u8; 9]) -> Self {
        Self {
            code_keyword: values[0],
            code_string: values[1],
            code_number: values[2],
            code_comment: values[3],
            code_function: values[4],
            code_type: values[5],
            variable: values[6],
            search_match: values[7],
            search_current: values[8],
        }
    }
}

impl Default for SchemeIndexes {
    fn default() -> Self {
        Self::new([81, 114, 215, 244, 74, 183, 179, 141, 203])
    }
}

/// Indexes for a scheme name (any spelling); unknown names get the default.
pub fn scheme_indexes(color_scheme: &str) -> SchemeIndexes {
    SchemeIndexes::new(match normalize_color_scheme(color_scheme).as_str() {
        "catppuccin-mocha" => [111, 150, 217, 103, 117, 183, 180, 147, 211],
        "catppuccin-latte" => [33, 29, 166, 102, 25, 61, 130, 69, 160],
        "gruvbox-dark" => [214, 142, 208, 245, 109, 175, 172, 179, 167],
        "gruvbox-light" => [130, 64, 166, 102, 25, 95, 94, 136, 160],
        "dracula" => [177, 114, 221, 103, 117, 183, 222, 141, 204],
        "dark" => [75, 114, 215, 244, 74, 153, 152, 111, 203],
        "white" => [26, 28, 166, 102, 31, 61, 24, 69, 160],
        "solarized-dark" => [136, 64, 166, 102, 37, 61, 109, 144, 166],
        "solarized-light" => [166, 64, 130, 102, 31, 60, 65, 137, 160],
        "nord" => [110, 150, 180, 102, 81, 146, 152, 109, 203],
        "tokyo-night" => [111, 114, 216, 103, 75, 147, 153, 147, 204],
        "one-dark" => [75, 114, 180, 245, 74, 176, 152, 111, 203],
        _ => return SchemeIndexes::default(),
    })
}

/// Light schemes put dark text on a light surface.
pub fn scheme_is_light(color_scheme: &str) -> bool {
    matches!(
        normalize_color_scheme(color_scheme).as_str(),
        "slate"
            | "slate-light"
            | "catppuccin-latte"
            | "gruvbox-light"
            | "white"
            | "solarized-light"
    )
}

/// The RGB of an xterm 256-colour index.
pub fn ansi_256_to_rgb(index: u8) -> (u8, u8, u8) {
    match index {
        0..=15 => [
            (0, 0, 0),
            (128, 0, 0),
            (0, 128, 0),
            (128, 128, 0),
            (0, 0, 128),
            (128, 0, 128),
            (0, 128, 128),
            (192, 192, 192),
            (128, 128, 128),
            (255, 0, 0),
            (0, 255, 0),
            (255, 255, 0),
            (0, 0, 255),
            (255, 0, 255),
            (0, 255, 255),
            (255, 255, 255),
        ][index as usize],
        16..=231 => {
            let i = (index - 16) as usize;
            (
                ANSI_COLOR_CUBE_LEVELS[i / 36],
                ANSI_COLOR_CUBE_LEVELS[i / 6 % 6],
                ANSI_COLOR_CUBE_LEVELS[i % 6],
            )
        }
        232..=255 => {
            let v = 8 + (index - 232) * 10;
            (v, v, v)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scheme_names_normalize_and_unknown_get_defaults() {
        assert_eq!(scheme_indexes("Gruvbox Dark").code_keyword, 214);
        assert_eq!(scheme_indexes("nope"), SchemeIndexes::default());
        assert!(scheme_is_light("Gruvbox_Light"));
        assert!(!scheme_is_light("nord"));
    }

    #[test]
    fn accents_resolve_names_hex_and_auto() {
        assert_eq!(accent_rgb("auto"), None);
        assert_eq!(accent_rgb("rose"), Some((0xb5, 0x5a, 0x79)));
        assert_eq!(accent_rgb("#4f7bd9"), Some((0x4f, 0x7b, 0xd9)));
        assert_eq!(accent_rgb("not-a-color"), None);
    }

    #[test]
    fn xterm_indexes_convert_to_rgb() {
        assert_eq!(ansi_256_to_rgb(16), (0, 0, 0));
        assert_eq!(ansi_256_to_rgb(231), (255, 255, 255));
        assert_eq!(ansi_256_to_rgb(232), (8, 8, 8));
        assert_eq!(ansi_256_to_rgb(rgb_to_ansi_256((255, 0, 0))), (255, 0, 0));
    }
}
