use serde::Deserialize;

#[derive(Clone, Copy, Debug)]
pub(super) enum FontFace {
    Body,
    Bold,
    Italic,
    BoldItalic,
    Mono,
}

impl FontFace {
    pub(super) fn resource_name(self) -> &'static str {
        match self {
            Self::Body => "F1",
            Self::Bold => "F2",
            Self::Mono => "F3",
            Self::Italic => "F4",
            Self::BoldItalic => "F5",
        }
    }

    pub(super) fn width_factor(self) -> f32 {
        match self {
            Self::Body => 0.53,
            Self::Bold => 0.56,
            Self::Italic => 0.53,
            Self::BoldItalic => 0.56,
            Self::Mono => 0.60,
        }
    }

    pub(super) fn space_factor(self) -> f32 {
        match self {
            // Courier space glyph width matches regular glyph width.
            Self::Mono => 1.0,
            // Keep current visual balance for Helvetica variants.
            Self::Body | Self::Bold | Self::Italic | Self::BoldItalic => 0.8,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub struct PdfRgbColor {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl PdfRgbColor {
    pub(super) fn as_pdf_rgb(self) -> (f32, f32, f32) {
        (
            (self.r as f32) / 255.0,
            (self.g as f32) / 255.0,
            (self.b as f32) / 255.0,
        )
    }
}

#[cfg(test)]
pub(super) fn pdf_black() -> PdfRgbColor {
    PdfRgbColor { r: 0, g: 0, b: 0 }
}

pub(super) fn clamp_u8(value: f32) -> u8 {
    value.round().clamp(0.0, 255.0) as u8
}

pub(super) fn color_luma(color: PdfRgbColor) -> f32 {
    let r = color.r as f32 / 255.0;
    let g = color.g as f32 / 255.0;
    let b = color.b as f32 / 255.0;
    (0.2126 * r) + (0.7152 * g) + (0.0722 * b)
}

pub(super) fn blend_color(a: PdfRgbColor, b: PdfRgbColor, t: f32) -> PdfRgbColor {
    let t = t.clamp(0.0, 1.0);
    PdfRgbColor {
        r: clamp_u8((a.r as f32) * (1.0 - t) + (b.r as f32) * t),
        g: clamp_u8((a.g as f32) * (1.0 - t) + (b.g as f32) * t),
        b: clamp_u8((a.b as f32) * (1.0 - t) + (b.b as f32) * t),
    }
}

pub(super) fn normalize_for_paper(mut color: PdfRgbColor, min_luma: f32, max_luma: f32) -> PdfRgbColor {
    let mut luma = color_luma(color);
    if luma > max_luma && luma > 0.0 {
        let scale = max_luma / luma;
        color = PdfRgbColor {
            r: clamp_u8(color.r as f32 * scale),
            g: clamp_u8(color.g as f32 * scale),
            b: clamp_u8(color.b as f32 * scale),
        };
        luma = color_luma(color);
    }
    if luma < min_luma && luma < 1.0 {
        let mix = ((min_luma - luma) / (1.0 - luma)).clamp(0.0, 1.0);
        color = blend_color(
            color,
            PdfRgbColor {
                r: 255,
                g: 255,
                b: 255,
            },
            mix,
        );
    }
    color
}

pub(super) fn paper_body_color(palette: &PdfExportPalette) -> PdfRgbColor {
    normalize_for_paper(palette.fg, 0.10, 0.22)
}

pub(super) fn paper_heading_color(palette: &PdfExportPalette) -> PdfRgbColor {
    normalize_for_paper(blend_color(palette.fg, palette.accent, 0.15), 0.08, 0.20)
}

pub(super) fn paper_border_color(palette: &PdfExportPalette) -> PdfRgbColor {
    let mixed = blend_color(paper_body_color(palette), palette.accent, 0.22);
    normalize_for_paper(mixed, 0.16, 0.34)
}

pub(super) fn paper_unchecked_checkbox_color(palette: &PdfExportPalette) -> PdfRgbColor {
    normalize_for_paper(blend_color(palette.fg_dim, palette.fg, 0.35), 0.18, 0.38)
}

pub(super) fn paper_code_bg_color(palette: &PdfExportPalette) -> PdfRgbColor {
    let accent_lifted = normalize_for_paper(palette.accent, 0.18, 0.36);
    normalize_for_paper(
        blend_color(
            accent_lifted,
            PdfRgbColor {
                r: 255,
                g: 255,
                b: 255,
            },
            0.88,
        ),
        0.93,
        0.98,
    )
}

pub(super) fn paper_code_border_color(palette: &PdfExportPalette) -> PdfRgbColor {
    normalize_for_paper(
        blend_color(
            paper_border_color(palette),
            paper_code_bg_color(palette),
            0.25,
        ),
        0.35,
        0.70,
    )
}

#[derive(Debug, Clone, Deserialize)]
pub struct PdfExportPalette {
    #[allow(dead_code)]
    pub fg: PdfRgbColor,
    #[allow(dead_code)]
    pub fg_dim: PdfRgbColor,
    pub accent: PdfRgbColor,
    pub variable: PdfRgbColor,
    pub code_keyword: PdfRgbColor,
    pub code_string: PdfRgbColor,
    pub code_number: PdfRgbColor,
    pub code_comment: PdfRgbColor,
    pub code_function: PdfRgbColor,
    pub code_type: PdfRgbColor,
}

impl Default for PdfExportPalette {
    fn default() -> Self {
        Self {
            fg: PdfRgbColor {
                r: 30,
                g: 32,
                b: 36,
            },
            fg_dim: PdfRgbColor {
                r: 109,
                g: 102,
                b: 91,
            },
            accent: PdfRgbColor {
                r: 122,
                g: 90,
                b: 58,
            },
            variable: PdfRgbColor {
                r: 134,
                g: 99,
                b: 202,
            },
            code_keyword: PdfRgbColor {
                r: 96,
                g: 112,
                b: 181,
            },
            code_string: PdfRgbColor {
                r: 91,
                g: 158,
                b: 111,
            },
            code_number: PdfRgbColor {
                r: 214,
                g: 120,
                b: 67,
            },
            code_comment: PdfRgbColor {
                r: 126,
                g: 138,
                b: 149,
            },
            code_function: PdfRgbColor {
                r: 52,
                g: 122,
                b: 165,
            },
            code_type: PdfRgbColor {
                r: 134,
                g: 99,
                b: 202,
            },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct TextStyle {
    pub(super) mono: bool,
    pub(super) bold: bool,
    pub(super) italic: bool,
    pub(super) strikethrough: bool,
    pub(super) color: PdfRgbColor,
}

impl TextStyle {
    pub(super) fn body(color: PdfRgbColor) -> Self {
        Self {
            mono: false,
            bold: false,
            italic: false,
            strikethrough: false,
            color,
        }
    }

    pub(super) fn heading(color: PdfRgbColor) -> Self {
        Self {
            mono: false,
            bold: true,
            italic: false,
            strikethrough: false,
            color,
        }
    }

    pub(super) fn mono(color: PdfRgbColor) -> Self {
        Self {
            mono: true,
            bold: false,
            italic: false,
            strikethrough: false,
            color,
        }
    }

    pub(super) fn font_face(self) -> FontFace {
        if self.mono {
            return FontFace::Mono;
        }
        match (self.bold, self.italic) {
            (true, true) => FontFace::BoldItalic,
            (true, false) => FontFace::Bold,
            (false, true) => FontFace::Italic,
            (false, false) => FontFace::Body,
        }
    }
}

#[derive(Debug, Clone)]
pub(super) struct StyledChar {
    pub(super) ch: char,
    pub(super) style: TextStyle,
}

// Standard Helvetica AFM widths (1/1000 em) for printable ASCII 32-126.
// Source: Adobe Helvetica AFM, matches PDF 1.7 Annex D standard metrics.
#[rustfmt::skip]
pub(super) const HELVETICA_CHAR_WIDTHS: [u16; 95] = [
    278, 278, 355, 556, 556, 889, 667, 222, 333, 333, 389, 584, 278, 333, 278, 278, // 32-47
    556, 556, 556, 556, 556, 556, 556, 556, 556, 556, 278, 278, 584, 584, 584, 556, // 48-63
   1015, 667, 667, 722, 722, 667, 611, 778, 722, 278, 500, 667, 556, 833, 722, 778, // 64-79
    667, 778, 722, 667, 611, 722, 667, 944, 667, 667, 611, 278, 278, 278, 469, 556, // 80-95
    222, 556, 556, 500, 556, 556, 278, 556, 556, 222, 222, 500, 222, 833, 556, 556, // 96-111
    556, 556, 333, 500, 278, 556, 500, 722, 500, 500, 500, 334, 260, 334, 584,      // 112-126
];

// Standard Helvetica-Bold AFM widths (1/1000 em) for printable ASCII 32-126.
#[rustfmt::skip]
pub(super) const HELVETICA_BOLD_CHAR_WIDTHS: [u16; 95] = [
    278, 333, 474, 556, 556, 889, 722, 278, 333, 333, 389, 584, 278, 333, 278, 278, // 32-47
    556, 556, 556, 556, 556, 556, 556, 556, 556, 556, 333, 333, 584, 584, 584, 611, // 48-63
    975, 722, 722, 722, 722, 667, 611, 778, 722, 278, 556, 722, 611, 833, 722, 778, // 64-79
    667, 778, 722, 667, 611, 722, 667, 944, 667, 667, 611, 278, 278, 278, 584, 556, // 80-95
    333, 556, 611, 556, 611, 556, 333, 611, 611, 278, 278, 556, 278, 889, 611, 611, // 96-111
    611, 611, 389, 556, 333, 611, 556, 778, 556, 556, 500, 389, 280, 389, 584,      // 112-126
];

#[derive(Debug, Clone)]
pub(crate) struct PdfUnicodeFontAsset {
    pub(crate) bytes: Vec<u8>,
    pub(crate) units_per_em: u16,
    pub(crate) ascent: i16,
    pub(crate) descent: i16,
    pub(crate) cap_height: i16,
    pub(crate) bbox_min_x: i16,
    pub(crate) bbox_min_y: i16,
    pub(crate) bbox_max_x: i16,
    pub(crate) bbox_max_y: i16,
    pub(crate) flags: u32,
    pub(crate) stem_v: i32,
    pub(crate) fallback_gid: u16,
    /// glyph_id → horizontal advance width in font units. Indexed by GlyphId.0.
    pub(crate) glyph_advances: Vec<u16>,
    /// BMP codepoint → glyph ID (0 = not in font). 65536 entries.
    pub(crate) bmp_glyph_ids: Vec<u16>,
}

impl PdfUnicodeFontAsset {
    pub(super) fn glyph_advance_for_char(&self, ch: char) -> Option<f32> {
        let cp = ch as u32;
        if cp >= 65536 {
            return None;
        }
        let gid = self.bmp_glyph_ids[cp as usize];
        if gid == 0 {
            return None;
        }
        let adv = *self.glyph_advances.get(gid as usize)?;
        if adv == 0 {
            return None;
        }
        Some(adv as f32 / self.units_per_em as f32)
    }
}
