use app_core::calc::{CalcEngine, NoteEvaluationOptions};
use app_core::note_sources::NoteSourceService;
#[cfg(feature = "gui")]
use app_core::AppCore;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use base64::Engine as _;
use editor_core::calc_plan;
use editor_core::markdown_tokens::{self, CodeTokenType, InlineTokenType};
use editor_core::table::{is_table_line, split_table_cells_for_logical_row};
use flate2::{write::ZlibEncoder, Compression};
use image::GenericImageView as _;
use regex::Regex;
use serde::Deserialize;
use rustc_hash::FxHashMap;
use std::collections::BTreeSet;
use std::env;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
#[cfg(feature = "gui")]
use tauri::State;
use ttf_parser::Face;
use url::Url;

const PDF_PAGE_WIDTH_PT: f32 = 595.0;
const PDF_PAGE_HEIGHT_PT: f32 = 842.0;
const PDF_MARGIN_LEFT_PT: f32 = 40.0;
const PDF_MARGIN_RIGHT_PT: f32 = 40.0;
const PDF_MARGIN_TOP_PT: f32 = 42.0;
const PDF_MARGIN_BOTTOM_PT: f32 = 42.0;

const BODY_FONT_SIZE_PT: f32 = 11.0;
const BODY_LINE_HEIGHT_PT: f32 = 14.0;
const HEADING_LINE_GAP_PT: f32 = 6.0;
const PARAGRAPH_GAP_PT: f32 = 5.0;

const TABLE_FONT_SIZE_PT: f32 = 10.0;
const TABLE_LINE_HEIGHT_PT: f32 = 12.0;
const TABLE_CELL_PAD_X_PT: f32 = 4.0;
const TABLE_CELL_PAD_Y_PT: f32 = 4.0;
const TABLE_BORDER_WIDTH_PT: f32 = 0.7;
const CODE_BLOCK_BORDER_WIDTH_PT: f32 = 0.7;
const CODE_BLOCK_PAD_X_PT: f32 = 6.0;
const CODE_BLOCK_PAD_Y_PT: f32 = 6.0;

const IMAGE_MAX_HEIGHT_PT: f32 = 280.0;
const CHECKBOX_SIZE_PT: f32 = 9.0;

const MAX_PDF_IMAGE_BYTES: u64 = 16 * 1024 * 1024;
const MAX_PDF_TOTAL_IMAGE_BYTES: u64 = 64 * 1024 * 1024;
const PDF_TAB_WIDTH: usize = 4;

#[derive(Clone, Copy, Debug)]
enum FontFace {
    Body,
    Bold,
    Italic,
    BoldItalic,
    Mono,
}

impl FontFace {
    fn resource_name(self) -> &'static str {
        match self {
            Self::Body => "F1",
            Self::Bold => "F2",
            Self::Mono => "F3",
            Self::Italic => "F4",
            Self::BoldItalic => "F5",
        }
    }

    fn width_factor(self) -> f32 {
        match self {
            Self::Body => 0.53,
            Self::Bold => 0.56,
            Self::Italic => 0.53,
            Self::BoldItalic => 0.56,
            Self::Mono => 0.60,
        }
    }

    fn space_factor(self) -> f32 {
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
    fn as_pdf_rgb(self) -> (f32, f32, f32) {
        (
            (self.r as f32) / 255.0,
            (self.g as f32) / 255.0,
            (self.b as f32) / 255.0,
        )
    }
}

#[cfg(test)]
fn pdf_black() -> PdfRgbColor {
    PdfRgbColor { r: 0, g: 0, b: 0 }
}

fn clamp_u8(value: f32) -> u8 {
    value.round().clamp(0.0, 255.0) as u8
}

fn color_luma(color: PdfRgbColor) -> f32 {
    let r = color.r as f32 / 255.0;
    let g = color.g as f32 / 255.0;
    let b = color.b as f32 / 255.0;
    (0.2126 * r) + (0.7152 * g) + (0.0722 * b)
}

fn blend_color(a: PdfRgbColor, b: PdfRgbColor, t: f32) -> PdfRgbColor {
    let t = t.clamp(0.0, 1.0);
    PdfRgbColor {
        r: clamp_u8((a.r as f32) * (1.0 - t) + (b.r as f32) * t),
        g: clamp_u8((a.g as f32) * (1.0 - t) + (b.g as f32) * t),
        b: clamp_u8((a.b as f32) * (1.0 - t) + (b.b as f32) * t),
    }
}

fn normalize_for_paper(mut color: PdfRgbColor, min_luma: f32, max_luma: f32) -> PdfRgbColor {
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

fn paper_body_color(palette: &PdfExportPalette) -> PdfRgbColor {
    normalize_for_paper(palette.fg, 0.10, 0.22)
}

fn paper_heading_color(palette: &PdfExportPalette) -> PdfRgbColor {
    normalize_for_paper(blend_color(palette.fg, palette.accent, 0.15), 0.08, 0.20)
}

fn paper_border_color(palette: &PdfExportPalette) -> PdfRgbColor {
    let mixed = blend_color(paper_body_color(palette), palette.accent, 0.22);
    normalize_for_paper(mixed, 0.16, 0.34)
}

fn paper_unchecked_checkbox_color(palette: &PdfExportPalette) -> PdfRgbColor {
    normalize_for_paper(blend_color(palette.fg_dim, palette.fg, 0.35), 0.18, 0.38)
}

fn paper_code_bg_color(palette: &PdfExportPalette) -> PdfRgbColor {
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

fn paper_code_border_color(palette: &PdfExportPalette) -> PdfRgbColor {
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
struct TextStyle {
    mono: bool,
    bold: bool,
    italic: bool,
    strikethrough: bool,
    color: PdfRgbColor,
}

impl TextStyle {
    fn body(color: PdfRgbColor) -> Self {
        Self {
            mono: false,
            bold: false,
            italic: false,
            strikethrough: false,
            color,
        }
    }

    fn heading(color: PdfRgbColor) -> Self {
        Self {
            mono: false,
            bold: true,
            italic: false,
            strikethrough: false,
            color,
        }
    }

    fn mono(color: PdfRgbColor) -> Self {
        Self {
            mono: true,
            bold: false,
            italic: false,
            strikethrough: false,
            color,
        }
    }

    fn font_face(self) -> FontFace {
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
struct StyledChar {
    ch: char,
    style: TextStyle,
}

#[derive(Debug, Clone)]
struct PdfImageObject {
    width: u32,
    height: u32,
    compressed_rgb: Vec<u8>,
}

#[derive(Debug, Clone)]
enum DrawOp {
    Text {
        font: FontFace,
        size: f32,
        x: f32,
        y: f32,
        color: PdfRgbColor,
        text: String,
    },
    Line {
        width: f32,
        x1: f32,
        y1: f32,
        x2: f32,
        y2: f32,
        color: PdfRgbColor,
    },
    Image {
        name: String,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
    },
    Rect {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        fill: Option<PdfRgbColor>,
        stroke_width: Option<f32>,
        stroke: Option<PdfRgbColor>,
    },
}

#[derive(Debug, Clone, Default)]
struct Page {
    ops: Vec<DrawOp>,
    used_images: BTreeSet<String>,
}

#[derive(Debug, Clone)]
struct PdfUnicodeFontAsset {
    bytes: Vec<u8>,
    units_per_em: u16,
    ascent: i16,
    descent: i16,
    cap_height: i16,
    bbox_min_x: i16,
    bbox_min_y: i16,
    bbox_max_x: i16,
    bbox_max_y: i16,
    flags: u32,
    stem_v: i32,
    fallback_gid: u16,
}

fn resolve_export_path(path: &str) -> Result<PathBuf, String> {
    let raw = path.trim().trim_matches(|c| c == '"' || c == '\'');
    if raw.is_empty() {
        return Err("export path is empty".to_string());
    }

    let resolved = if raw == "~" || raw.starts_with("~/") || raw.starts_with("~\\") {
        let home = resolve_home_dir().ok_or_else(|| {
            "home directory is not set; cannot expand '~' in export path".to_string()
        })?;
        if raw == "~" {
            home
        } else {
            let rest = &raw[2..];
            home.join(rest)
        }
    } else if raw.len() >= "file://".len() && raw[.."file://".len()].eq_ignore_ascii_case("file://")
    {
        let uri =
            Url::parse(raw).map_err(|e| format!("invalid file URL export path '{raw}': {e}"))?;
        uri.to_file_path()
            .map_err(|_| format!("invalid file URL export path '{raw}'"))?
    } else {
        PathBuf::from(raw)
    };

    if resolved.is_dir() {
        return Err(format!(
            "export path points to a directory: {}",
            resolved.display()
        ));
    }

    let parent = resolved
        .parent()
        .filter(|value| !value.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    if !parent.exists() {
        return Err(format!(
            "export directory does not exist: {}",
            parent.display()
        ));
    }
    if !parent.is_dir() {
        return Err(format!(
            "export parent is not a directory: {}",
            parent.display()
        ));
    }

    Ok(resolved)
}

fn resolve_home_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").filter(|v| !v.is_empty());
    if home.is_some() {
        return home.map(PathBuf::from);
    }
    let profile = std::env::var_os("USERPROFILE").filter(|v| !v.is_empty());
    if profile.is_some() {
        return profile.map(PathBuf::from);
    }
    let drive = std::env::var_os("HOMEDRIVE").filter(|v| !v.is_empty());
    let path = std::env::var_os("HOMEPATH").filter(|v| !v.is_empty());
    match (drive, path) {
        (Some(drive), Some(path)) => {
            let mut full = PathBuf::from(drive);
            full.push(path);
            Some(full)
        }
        _ => None,
    }
}

fn default_unicode_font_candidates() -> Vec<PathBuf> {
    vec![
        PathBuf::from("/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf"),
        PathBuf::from("/usr/share/fonts/TTF/DejaVuSans.ttf"),
        PathBuf::from("/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf"),
        PathBuf::from("/usr/share/fonts/noto/NotoSans-Regular.ttf"),
        PathBuf::from("/Library/Fonts/Arial Unicode.ttf"),
        PathBuf::from("/Library/Fonts/Arial Unicode MS.ttf"),
        PathBuf::from("/System/Library/Fonts/Supplemental/Arial Unicode.ttf"),
        PathBuf::from("C:\\Windows\\Fonts\\arialuni.ttf"),
        PathBuf::from("C:\\Windows\\Fonts\\seguiemj.ttf"),
        PathBuf::from("C:\\Windows\\Fonts\\segoeui.ttf"),
    ]
}

fn load_unicode_pdf_font_asset() -> Option<PdfUnicodeFontAsset> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(path) = env::var_os("SLATE_PDF_UNICODE_FONT") {
        let candidate = PathBuf::from(path);
        if !candidate.as_os_str().is_empty() {
            candidates.push(candidate);
        }
    }
    candidates.extend(default_unicode_font_candidates());

    for path in candidates {
        let Ok(bytes) = fs::read(&path) else {
            continue;
        };
        let Ok(face) = Face::parse(&bytes, 0) else {
            continue;
        };
        let bbox = face.global_bounding_box();
        let mut flags = 32u32; // Nonsymbolic
        if face.is_monospaced() {
            flags |= 1;
        }
        if face.is_italic() {
            flags |= 64;
        }
        let fallback_gid = face.glyph_index('?').map(|id| id.0).unwrap_or(0);
        let units_per_em = face.units_per_em();
        let ascent = face.ascender();
        let descent = face.descender();
        let cap_height = face.capital_height().unwrap_or(face.ascender());
        return Some(PdfUnicodeFontAsset {
            bytes,
            units_per_em,
            ascent,
            descent,
            cap_height,
            bbox_min_x: bbox.x_min,
            bbox_min_y: bbox.y_min,
            bbox_max_x: bbox.x_max,
            bbox_max_y: bbox.y_max,
            flags,
            stem_v: 80,
            fallback_gid,
        });
    }
    None
}

fn resolve_unicode_pdf_font_asset() -> Option<PdfUnicodeFontAsset> {
    static CACHE: OnceLock<Option<PdfUnicodeFontAsset>> = OnceLock::new();
    CACHE.get_or_init(load_unicode_pdf_font_asset).clone()
}

#[cfg(feature = "gui")]
#[tauri::command]
pub async fn export_to_file(path: String, content: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || export_to_file_blocking(&path, &content))
        .await
        .map_err(|e| format!("export task join failed: {e}"))?
}

pub fn export_to_file_blocking(path: &str, content: &str) -> Result<(), String> {
    let resolved = resolve_export_path(path)?;
    fs::write(&resolved, content)
        .map_err(|e| format!("Failed to write file '{}': {e}", resolved.display()))
}

#[cfg(feature = "gui")]
#[tauri::command]
pub async fn export_to_pdf(
    core: State<'_, AppCore>,
    note_id: String,
    path: String,
    content: String,
    palette: PdfExportPalette,
) -> Result<(), String> {
    let note_sources = core.note_sources().clone();
    tauri::async_runtime::spawn_blocking(move || {
        export_markdown_to_pdf_file(&note_sources, &note_id, &path, &content, &palette)
    })
    .await
    .map_err(|e| format!("PDF export task join failed: {e}"))?
}

pub fn export_markdown_to_pdf_file(
    note_sources: &NoteSourceService,
    note_id: &str,
    path: &str,
    content: &str,
    palette: &PdfExportPalette,
) -> Result<(), String> {
    let resolved = resolve_export_path(path)?;
    let pdf_bytes = build_markdown_pdf(content, palette, |src| {
        resolve_markdown_image_bytes(note_sources, note_id, src)
            .ok()
            .flatten()
    })?;
    fs::write(&resolved, pdf_bytes)
        .map_err(|e| format!("Failed to write PDF '{}': {e}", resolved.display()))
}

fn resolve_markdown_image_bytes(
    note_sources: &NoteSourceService,
    note_id: &str,
    src: &str,
) -> Result<Option<Vec<u8>>, String> {
    let resolved = note_sources.resolve_image_markdown_source_by_id(note_id, src)?;
    let Some(value) = resolved else {
        return Ok(None);
    };

    if value.starts_with("data:") {
        let bytes = decode_data_url_image_bytes(&value)?;
        if bytes.len() as u64 > MAX_PDF_IMAGE_BYTES {
            return Ok(None);
        }
        return Ok(Some(bytes));
    }

    let size = fs::metadata(&value).map(|m| m.len()).unwrap_or(0);
    if size > MAX_PDF_IMAGE_BYTES {
        return Ok(None);
    }

    fs::read(&value)
        .map(Some)
        .map_err(|e| format!("Failed to read image '{value}': {e}"))
}

fn decode_data_url_image_bytes(data_url: &str) -> Result<Vec<u8>, String> {
    let Some(payload) = data_url.strip_prefix("data:") else {
        return Err("invalid data URL image prefix".to_string());
    };
    let Some((meta, encoded)) = payload.split_once(',') else {
        return Err("invalid data URL image payload".to_string());
    };
    if !meta.to_ascii_lowercase().contains(";base64") {
        return Err("unsupported non-base64 image data URL".to_string());
    }
    BASE64_STANDARD
        .decode(encoded.trim())
        .map_err(|e| format!("invalid base64 image payload: {e}"))
}

fn build_markdown_pdf(
    content: &str,
    palette: &PdfExportPalette,
    mut resolve_image: impl FnMut(&str) -> Option<Vec<u8>>,
) -> Result<Vec<u8>, String> {
    let image_sources = collect_image_sources(content);
    let mut image_name_by_src: FxHashMap<String, String> = FxHashMap::default();
    let mut image_assets: Vec<(String, PdfImageObject)> = Vec::new();
    let mut total_image_bytes: u64 = 0;

    for src in image_sources {
        if total_image_bytes >= MAX_PDF_TOTAL_IMAGE_BYTES {
            break;
        }
        let Some(image_bytes) = resolve_image(src.as_str()) else {
            continue;
        };
        let byte_len = image_bytes.len() as u64;
        if total_image_bytes.saturating_add(byte_len) > MAX_PDF_TOTAL_IMAGE_BYTES {
            continue;
        }
        let Ok(image_obj) = decode_pdf_image(&image_bytes) else {
            continue;
        };
        total_image_bytes += byte_len;
        let name = format!("Im{}", image_assets.len() + 1);
        image_name_by_src.insert(src, name.clone());
        image_assets.push((name, image_obj));
    }

    let pages = render_markdown_to_pages(content, palette, &image_name_by_src, &image_assets);
    serialize_pdf(pages, image_assets)
}

fn decode_pdf_image(bytes: &[u8]) -> Result<PdfImageObject, String> {
    let image = image::load_from_memory(bytes)
        .map_err(|e| format!("failed to decode image bytes for PDF export: {e}"))?;
    let (width, height) = image.dimensions();
    if width == 0 || height == 0 {
        return Err("image has invalid dimensions".to_string());
    }
    let rgb = image.to_rgb8();
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
    encoder
        .write_all(rgb.as_raw())
        .map_err(|e| format!("failed to encode image pixels for PDF: {e}"))?;
    let compressed = encoder
        .finish()
        .map_err(|e| format!("failed to finalize image compression for PDF: {e}"))?;

    Ok(PdfImageObject {
        width,
        height,
        compressed_rgb: compressed,
    })
}

fn render_markdown_to_pages(
    content: &str,
    palette: &PdfExportPalette,
    image_name_by_src: &FxHashMap<String, String>,
    image_assets: &[(String, PdfImageObject)],
) -> Vec<Page> {
    let mut pages = vec![Page::default()];
    let mut y_top = PDF_MARGIN_TOP_PT;
    let max_top = PDF_PAGE_HEIGHT_PT - PDF_MARGIN_BOTTOM_PT;

    let normalized = content.replace("\r\n", "\n").replace('\r', "\n");
    let owned_lines = normalized
        .split('\n')
        .map(|line| line.to_string())
        .collect::<Vec<_>>();
    let lines: Vec<&str> = owned_lines.iter().map(String::as_str).collect();
    let variable_names = calc_plan::collect_assignment_names(&owned_lines);
    let table_formula_values = collect_table_formula_display_values(&owned_lines);
    let mut i = 0usize;

    while i < lines.len() {
        let line = lines[i];
        let trimmed = line.trim();

        if trimmed.is_empty() {
            ensure_space(&mut pages, &mut y_top, BODY_LINE_HEIGHT_PT * 0.6, max_top);
            y_top += BODY_LINE_HEIGHT_PT * 0.6;
            i += 1;
            continue;
        }

        if let Some((alt, src)) = parse_image_only_line(trimmed) {
            let mut rendered = false;
            if let Some(image_name) = image_name_by_src.get(&src) {
                if let Some((_, image_obj)) =
                    image_assets.iter().find(|(name, _)| name == image_name)
                {
                    render_image(&mut pages, &mut y_top, max_top, image_name, image_obj);
                    rendered = true;
                }
            }
            if !rendered {
                render_plain_wrapped_text(
                    &mut pages,
                    &mut y_top,
                    max_top,
                    format!("[image unavailable: {}]", alt_if_empty(&alt)).as_str(),
                    TextStyle::mono(paper_body_color(palette)),
                    BODY_FONT_SIZE_PT,
                    BODY_LINE_HEIGHT_PT,
                    PDF_MARGIN_LEFT_PT,
                    content_width(),
                );
                y_top += PARAGRAPH_GAP_PT;
            }
            i += 1;
            continue;
        }

        if let Some((level, heading_text)) = parse_heading(line) {
            let size = heading_font_size(level);
            let heading_style = TextStyle::heading(paper_heading_color(palette));
            let chars = styled_chars_from_inline(
                heading_text.as_str(),
                &variable_names,
                palette,
                heading_style,
            );
            render_styled_block(
                &mut pages,
                &mut y_top,
                max_top,
                &chars,
                size,
                size + 4.0,
                PDF_MARGIN_LEFT_PT,
                content_width(),
            );
            y_top += HEADING_LINE_GAP_PT;
            i += 1;
            continue;
        }

        if is_fence_start(trimmed) {
            let (block_lines, code_lang, next) = collect_code_fence(&lines, i);
            render_code_block(
                &mut pages,
                &mut y_top,
                max_top,
                &block_lines,
                code_lang.as_deref(),
                palette,
            );
            i = next;
            continue;
        }

        if is_markdown_table_start(&lines, i) {
            let (table_lines, next) = collect_table_block(&lines, i);
            render_table_block(
                &mut pages,
                &mut y_top,
                max_top,
                &table_lines,
                i,
                palette,
                &variable_names,
                &table_formula_values,
            );
            i = next;
            continue;
        }

        if is_unordered_list_item(line) || is_ordered_list_item(line) {
            let (items, ordered, next) = collect_list_block(&lines, i);
            render_list_block(
                &mut pages,
                &mut y_top,
                max_top,
                &items,
                ordered,
                palette,
                &variable_names,
            );
            i = next;
            continue;
        }

        if trimmed.starts_with('>') {
            let (quote_lines, next) = collect_blockquote(&lines, i);
            render_blockquote_block(
                &mut pages,
                &mut y_top,
                max_top,
                &quote_lines,
                palette,
                &variable_names,
            );
            i = next;
            continue;
        }

        let (paragraph_lines, next) = collect_paragraph(&lines, i);
        if should_preserve_hard_linebreaks(&paragraph_lines) {
            render_hard_line_paragraph_block(
                &mut pages,
                &mut y_top,
                max_top,
                &paragraph_lines,
                palette,
                &variable_names,
            );
        } else {
            render_paragraph_block(
                &mut pages,
                &mut y_top,
                max_top,
                paragraph_lines.join(" ").as_str(),
                palette,
                &variable_names,
            );
        }
        i = next;
    }

    pages
}

fn content_width() -> f32 {
    PDF_PAGE_WIDTH_PT - PDF_MARGIN_LEFT_PT - PDF_MARGIN_RIGHT_PT
}

fn ensure_space(pages: &mut Vec<Page>, y_top: &mut f32, needed_height: f32, max_top: f32) {
    if *y_top + needed_height <= max_top {
        return;
    }
    pages.push(Page::default());
    *y_top = PDF_MARGIN_TOP_PT;
}

fn current_page_mut(pages: &mut Vec<Page>) -> &mut Page {
    pages
        .last_mut()
        .expect("renderer must keep at least one page")
}

fn push_text_op(page: &mut Page, style: TextStyle, size: f32, x: f32, y: f32, text: String) {
    if text.is_empty() {
        return;
    }
    page.ops.push(DrawOp::Text {
        font: style.font_face(),
        size,
        x,
        y,
        color: style.color,
        text,
    });
}

fn push_line_op(
    page: &mut Page,
    width: f32,
    x1: f32,
    y1: f32,
    x2: f32,
    y2: f32,
    color: PdfRgbColor,
) {
    page.ops.push(DrawOp::Line {
        width,
        x1,
        y1,
        x2,
        y2,
        color,
    });
}

fn push_rect_op(
    page: &mut Page,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    fill: Option<PdfRgbColor>,
    stroke_width: Option<f32>,
    stroke: Option<PdfRgbColor>,
) {
    page.ops.push(DrawOp::Rect {
        x,
        y,
        w,
        h,
        fill,
        stroke_width,
        stroke,
    });
}

fn normalize_styled_whitespace(input: &[StyledChar]) -> Vec<StyledChar> {
    let mut out = Vec::with_capacity(input.len());
    let mut pending_space: Option<TextStyle> = None;
    for item in input {
        if item.ch.is_whitespace() {
            if !out.is_empty() && pending_space.is_none() {
                pending_space = Some(item.style);
            }
            continue;
        }
        if let Some(space_style) = pending_space.take() {
            out.push(StyledChar {
                ch: ' ',
                style: space_style,
            });
        }
        out.push(item.clone());
    }
    while out.last().is_some_and(|item| item.ch.is_whitespace()) {
        out.pop();
    }
    out
}

fn is_variable_word_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

fn has_variable_word_boundaries(bytes: &[u8], start: usize, end: usize) -> bool {
    let left_ok = start == 0 || !is_variable_word_byte(bytes[start - 1]);
    let right_ok = end == bytes.len() || !is_variable_word_byte(bytes[end]);
    left_ok && right_ok
}

fn eq_ascii_case_insensitive_bytes(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter()
        .zip(b.iter())
        .all(|(left, right)| left.eq_ignore_ascii_case(right))
}

fn find_variable_ranges(text: &str, variable_names: &[String]) -> Vec<(usize, usize)> {
    if text.is_empty() || variable_names.is_empty() {
        return Vec::new();
    }

    let bytes = text.as_bytes();
    let mut matches: Vec<(usize, usize)> = Vec::new();

    for raw in variable_names {
        let needle_text = raw.trim();
        if needle_text.is_empty() {
            continue;
        }
        let needle = needle_text.as_bytes();
        if needle.len() > bytes.len() {
            continue;
        }
        let mut idx = 0usize;
        while idx + needle.len() <= bytes.len() {
            let end = idx + needle.len();
            if eq_ascii_case_insensitive_bytes(&bytes[idx..end], needle)
                && has_variable_word_boundaries(bytes, idx, end)
            {
                matches.push((idx, end));
            }
            idx += 1;
        }
    }

    if matches.len() <= 1 {
        return matches;
    }

    matches.sort_by(|a, b| a.0.cmp(&b.0).then((b.1 - b.0).cmp(&(a.1 - a.0))));
    let mut deduped: Vec<(usize, usize)> = Vec::new();
    for candidate in matches {
        let Some(last) = deduped.last() else {
            deduped.push(candidate);
            continue;
        };
        if candidate.0 < last.1 {
            continue;
        }
        deduped.push(candidate);
    }
    deduped
}

fn byte_to_char_idx(text: &str, byte_idx: usize) -> usize {
    text[..byte_idx.min(text.len())].chars().count()
}

fn styled_chars_from_inline(
    text: &str,
    variable_names: &[String],
    palette: &PdfExportPalette,
    base: TextStyle,
) -> Vec<StyledChar> {
    let chars: Vec<char> = text.chars().collect();
    if chars.is_empty() {
        return Vec::new();
    }

    let mut styles = vec![base; chars.len()];
    let mut hidden = vec![false; chars.len()];

    let tokens = markdown_tokens::tokenize_inline_markdown(text);
    for token in &tokens {
        let from = token.from.min(chars.len());
        let to = token.to.min(chars.len());
        if to <= from {
            continue;
        }
        match token.kind {
            InlineTokenType::Strong => {
                for style in styles.iter_mut().take(to).skip(from) {
                    style.bold = true;
                }
            }
            InlineTokenType::Emphasis => {
                for style in styles.iter_mut().take(to).skip(from) {
                    style.italic = true;
                }
            }
            InlineTokenType::Strikethrough => {
                for style in styles.iter_mut().take(to).skip(from) {
                    style.strikethrough = true;
                }
            }
            InlineTokenType::Code => {
                for style in styles.iter_mut().take(to).skip(from) {
                    style.mono = true;
                    style.color = palette.code_type;
                }
            }
            InlineTokenType::CodeMarker => {
                for mark in hidden.iter_mut().take(to).skip(from) {
                    *mark = true;
                }
            }
            InlineTokenType::ImageAlt => {
                for style in styles.iter_mut().take(to).skip(from) {
                    style.italic = true;
                    style.bold = true;
                }
            }
            InlineTokenType::ImageSrc
            | InlineTokenType::ImageMarker
            | InlineTokenType::LinkUrl
            | InlineTokenType::LinkMarker
            | InlineTokenType::WikiLinkMarker
            | InlineTokenType::WikiLinkId
            | InlineTokenType::WikiLinkSep
            | InlineTokenType::WikiLinkAnchor => {
                for mark in hidden.iter_mut().take(to).skip(from) {
                    *mark = true;
                }
            }
            InlineTokenType::LinkText | InlineTokenType::WikiLinkTitle => {
                for style in styles.iter_mut().take(to).skip(from) {
                    style.bold = true;
                }
            }
        }
    }

    for (start, end) in find_variable_ranges(text, variable_names) {
        let from = byte_to_char_idx(text, start).min(chars.len());
        let to = byte_to_char_idx(text, end).min(chars.len());
        if to <= from {
            continue;
        }
        for idx in from..to {
            if hidden[idx] || styles[idx].mono {
                continue;
            }
            styles[idx].bold = true;
            styles[idx].color = palette.variable;
        }
    }

    let mut out = Vec::with_capacity(chars.len());
    for (idx, ch) in chars.into_iter().enumerate() {
        if hidden[idx] {
            continue;
        }
        out.push(StyledChar {
            ch,
            style: styles[idx],
        });
    }
    normalize_styled_whitespace(&out)
}

fn styled_chars_for_code_line(
    text: &str,
    lang: Option<&str>,
    palette: &PdfExportPalette,
) -> Vec<StyledChar> {
    let expanded = expand_tabs_for_pdf(text, PDF_TAB_WIDTH);
    let chars: Vec<char> = expanded.chars().collect();
    let mut styles = vec![TextStyle::mono(paper_body_color(palette)); chars.len()];
    let tokens = markdown_tokens::tokenize_code_line(expanded.as_str(), lang);

    for token in tokens {
        let from = token.from.min(chars.len());
        let to = token.to.min(chars.len());
        if to <= from {
            continue;
        }
        for style in styles.iter_mut().take(to).skip(from) {
            style.color = match token.kind {
                CodeTokenType::Keyword => palette.code_keyword,
                CodeTokenType::String => palette.code_string,
                CodeTokenType::Number => palette.code_number,
                CodeTokenType::Comment => palette.code_comment,
                CodeTokenType::Function => palette.code_function,
                CodeTokenType::Type => palette.code_type,
            };
            if matches!(token.kind, CodeTokenType::Comment) {
                style.italic = true;
            }
        }
    }

    chars
        .into_iter()
        .enumerate()
        .map(|(idx, ch)| StyledChar {
            ch,
            style: styles[idx],
        })
        .collect()
}

fn char_draw_width(ch: char, size: f32, style: TextStyle) -> f32 {
    let font = style.font_face();
    if ch == ' ' {
        size * font.width_factor() * font.space_factor()
    } else {
        size * font.width_factor()
    }
}

fn text_draw_width(text: &str, size: f32, style: TextStyle) -> f32 {
    text.chars()
        .map(|ch| char_draw_width(ch, size, style))
        .sum::<f32>()
}

fn recalc_line_metrics(line: &[StyledChar], size: f32) -> (f32, Option<usize>) {
    let mut width = 0.0f32;
    let mut last_space = None;
    for (idx, item) in line.iter().enumerate() {
        width += char_draw_width(item.ch, size, item.style);
        if item.ch == ' ' {
            last_space = Some(idx);
        }
    }
    (width, last_space)
}

fn trim_styled_line(mut line: Vec<StyledChar>) -> Vec<StyledChar> {
    while line.last().is_some_and(|item| item.ch == ' ') {
        line.pop();
    }
    line
}

fn wrap_styled_chars(chars: &[StyledChar], max_width: f32, size: f32) -> Vec<Vec<StyledChar>> {
    if chars.is_empty() {
        return vec![Vec::new()];
    }
    let mut out = Vec::new();
    let mut current: Vec<StyledChar> = Vec::new();
    let mut current_width = 0.0f32;
    let mut last_space = None;

    for item in chars {
        let item_w = char_draw_width(item.ch, size, item.style);
        if !current.is_empty() && current_width + item_w > max_width {
            if let Some(space_idx) = last_space {
                let line = trim_styled_line(current[..space_idx].to_vec());
                out.push(line);
                current = current[space_idx + 1..].to_vec();
            } else {
                out.push(trim_styled_line(current.clone()));
                current.clear();
            }
            let (next_width, next_space) = recalc_line_metrics(&current, size);
            current_width = next_width;
            last_space = next_space;
        }
        if current.is_empty() && item.ch == ' ' {
            continue;
        }
        current.push(item.clone());
        current_width += item_w;
        if item.ch == ' ' {
            last_space = Some(current.len() - 1);
        }
    }

    if !current.is_empty() {
        out.push(trim_styled_line(current));
    }
    if out.is_empty() {
        out.push(Vec::new());
    }
    out
}

fn render_styled_line(pages: &mut Vec<Page>, x: f32, y: f32, size: f32, line: &[StyledChar]) {
    if line.is_empty() {
        return;
    }
    let mut run_style = line[0].style;
    let mut run_text = String::new();
    let mut cursor_x = x;

    let flush =
        |page: &mut Page, run_style: TextStyle, run_text: &mut String, cursor_x: &mut f32| {
            if run_text.is_empty() {
                return;
            }
            let text = std::mem::take(run_text);
            push_text_op(page, run_style, size, *cursor_x, y, text.clone());
            let run_width = text_draw_width(text.as_str(), size, run_style);
            if run_style.strikethrough {
                // Draw strike only over non-space spans to avoid long bars
                // across collapsed/hidden markdown whitespace.
                let strike_y = y + (size * 0.35);
                let mut cursor = *cursor_x;
                let mut segment_start: Option<f32> = None;
                for ch in text.chars() {
                    let ch_width = char_draw_width(ch, size, run_style);
                    if ch.is_whitespace() {
                        if let Some(start) = segment_start.take() {
                            push_line_op(
                                page,
                                0.7,
                                start,
                                strike_y,
                                cursor,
                                strike_y,
                                run_style.color,
                            );
                        }
                    } else if segment_start.is_none() {
                        segment_start = Some(cursor);
                    }
                    cursor += ch_width;
                }
                if let Some(start) = segment_start {
                    push_line_op(
                        page,
                        0.7,
                        start,
                        strike_y,
                        cursor,
                        strike_y,
                        run_style.color,
                    );
                }
            }
            *cursor_x += run_width;
        };

    for item in line {
        if item.style != run_style {
            let page = current_page_mut(pages);
            flush(page, run_style, &mut run_text, &mut cursor_x);
            run_style = item.style;
        }
        run_text.push(item.ch);
    }
    let page = current_page_mut(pages);
    flush(page, run_style, &mut run_text, &mut cursor_x);
}

fn render_styled_block(
    pages: &mut Vec<Page>,
    y_top: &mut f32,
    max_top: f32,
    chars: &[StyledChar],
    size: f32,
    line_height: f32,
    x: f32,
    max_width: f32,
) {
    let lines = wrap_styled_chars(chars, max_width, size);
    for line in lines {
        ensure_space(pages, y_top, line_height, max_top);
        let y_pdf = PDF_PAGE_HEIGHT_PT - *y_top - size;
        render_styled_line(pages, x, y_pdf, size, &line);
        *y_top += line_height;
    }
}

fn render_plain_wrapped_text(
    pages: &mut Vec<Page>,
    y_top: &mut f32,
    max_top: f32,
    text: &str,
    style: TextStyle,
    size: f32,
    line_height: f32,
    x: f32,
    max_width: f32,
) {
    let chars = text
        .chars()
        .map(|ch| StyledChar { ch, style })
        .collect::<Vec<_>>();
    let chars = normalize_styled_whitespace(&chars);
    render_styled_block(
        pages,
        y_top,
        max_top,
        &chars,
        size,
        line_height,
        x,
        max_width,
    );
}

fn render_paragraph_block(
    pages: &mut Vec<Page>,
    y_top: &mut f32,
    max_top: f32,
    text: &str,
    palette: &PdfExportPalette,
    variable_names: &[String],
) {
    let chars = styled_chars_from_inline(
        text,
        variable_names,
        palette,
        TextStyle::body(paper_body_color(palette)),
    );
    render_styled_block(
        pages,
        y_top,
        max_top,
        &chars,
        BODY_FONT_SIZE_PT,
        BODY_LINE_HEIGHT_PT,
        PDF_MARGIN_LEFT_PT,
        content_width(),
    );
    *y_top += PARAGRAPH_GAP_PT;
}

fn render_hard_line_paragraph_block(
    pages: &mut Vec<Page>,
    y_top: &mut f32,
    max_top: f32,
    lines: &[String],
    palette: &PdfExportPalette,
    variable_names: &[String],
) {
    for line in lines {
        let chars = styled_chars_from_inline(
            line,
            variable_names,
            palette,
            TextStyle::body(paper_body_color(palette)),
        );
        render_styled_block(
            pages,
            y_top,
            max_top,
            &chars,
            BODY_FONT_SIZE_PT,
            BODY_LINE_HEIGHT_PT,
            PDF_MARGIN_LEFT_PT,
            content_width(),
        );
    }
    *y_top += PARAGRAPH_GAP_PT;
}

fn render_code_block(
    pages: &mut Vec<Page>,
    y_top: &mut f32,
    max_top: f32,
    code_lines: &[String],
    code_lang: Option<&str>,
    palette: &PdfExportPalette,
) {
    let code_width = content_width() - (CODE_BLOCK_PAD_X_PT * 2.0);
    let mut wrapped_lines: Vec<Vec<StyledChar>> = Vec::new();
    for line in code_lines {
        let chars = styled_chars_for_code_line(line, code_lang, palette);
        let wrapped = wrap_styled_chars(&chars, code_width, BODY_FONT_SIZE_PT);
        if wrapped.is_empty() {
            wrapped_lines.push(Vec::new());
        } else {
            wrapped_lines.extend(wrapped);
        }
    }
    if wrapped_lines.is_empty() {
        wrapped_lines.push(Vec::new());
    }

    let block_height =
        (wrapped_lines.len() as f32) * BODY_LINE_HEIGHT_PT + (CODE_BLOCK_PAD_Y_PT * 2.0);
    ensure_space(pages, y_top, block_height + PARAGRAPH_GAP_PT, max_top);

    let block_left = PDF_MARGIN_LEFT_PT;
    let block_bottom_pdf = PDF_PAGE_HEIGHT_PT - (*y_top + block_height);
    let block_bg = paper_code_bg_color(palette);
    let block_border = paper_code_border_color(palette);

    push_rect_op(
        current_page_mut(pages),
        block_left,
        block_bottom_pdf,
        content_width(),
        block_height,
        Some(block_bg),
        Some(CODE_BLOCK_BORDER_WIDTH_PT),
        Some(block_border),
    );

    let mut line_top = *y_top + CODE_BLOCK_PAD_Y_PT;
    for line_chars in wrapped_lines {
        let y_pdf = PDF_PAGE_HEIGHT_PT - line_top - BODY_FONT_SIZE_PT;
        render_styled_line(
            pages,
            block_left + CODE_BLOCK_PAD_X_PT,
            y_pdf,
            BODY_FONT_SIZE_PT,
            &line_chars,
        );
        line_top += BODY_LINE_HEIGHT_PT;
    }
    *y_top += block_height;

    *y_top += PARAGRAPH_GAP_PT;
}

fn parse_checklist_item(item: &str) -> Option<(bool, String)> {
    static CHECKLIST_RE: OnceLock<Regex> = OnceLock::new();
    let re = CHECKLIST_RE
        .get_or_init(|| Regex::new(r"^\[(x|X| )\]\s+(.*)$").expect("checklist regex must compile"));
    let caps = re.captures(item.trim())?;
    let checked = caps
        .get(1)
        .is_some_and(|entry| matches!(entry.as_str(), "x" | "X"));
    let content = caps
        .get(2)
        .map(|entry| entry.as_str().trim().to_string())
        .unwrap_or_default();
    Some((checked, content))
}

fn render_checklist_box(
    pages: &mut Vec<Page>,
    x: f32,
    y_top: f32,
    size: f32,
    checked: bool,
    palette: &PdfExportPalette,
) {
    let top = PDF_PAGE_HEIGHT_PT - y_top - ((BODY_LINE_HEIGHT_PT - size) * 0.5);
    let bottom = top - size;
    let left = x;
    let right = x + size;
    let color = if checked {
        normalize_for_paper(palette.accent, 0.16, 0.46)
    } else {
        paper_unchecked_checkbox_color(palette)
    };

    let page = current_page_mut(pages);
    push_line_op(page, 0.9, left, top, right, top, color);
    push_line_op(page, 0.9, right, top, right, bottom, color);
    push_line_op(page, 0.9, right, bottom, left, bottom, color);
    push_line_op(page, 0.9, left, bottom, left, top, color);

    if checked {
        let mid_x = left + (size * 0.42);
        let mid_y = bottom + (size * 0.30);
        let end_x = right - (size * 0.20);
        let end_y = top - (size * 0.28);
        let start_x = left + (size * 0.20);
        let start_y = bottom + (size * 0.52);
        push_line_op(page, 1.1, start_x, start_y, mid_x, mid_y, color);
        push_line_op(page, 1.1, mid_x, mid_y, end_x, end_y, color);
    }
}

fn render_list_block(
    pages: &mut Vec<Page>,
    y_top: &mut f32,
    max_top: f32,
    items: &[String],
    ordered: bool,
    palette: &PdfExportPalette,
    variable_names: &[String],
) {
    for (idx, item) in items.iter().enumerate() {
        let (is_checklist, checked, content) =
            if let Some((checked, content)) = parse_checklist_item(item) {
                (true, checked, content)
            } else {
                (false, false, item.clone())
            };

        let mut base = TextStyle::body(paper_body_color(palette));
        if checked {
            base.strikethrough = true;
        }
        let chars = styled_chars_from_inline(content.as_str(), variable_names, palette, base);
        let body_x = if is_checklist {
            PDF_MARGIN_LEFT_PT + 16.0
        } else {
            PDF_MARGIN_LEFT_PT + 18.0
        };
        let body_width = content_width() - (body_x - PDF_MARGIN_LEFT_PT);
        let wrapped = wrap_styled_chars(&chars, body_width, BODY_FONT_SIZE_PT);
        if wrapped.is_empty() {
            continue;
        }

        for (line_idx, line_chars) in wrapped.iter().enumerate() {
            ensure_space(pages, y_top, BODY_LINE_HEIGHT_PT, max_top);
            let y_pdf = PDF_PAGE_HEIGHT_PT - *y_top - BODY_FONT_SIZE_PT;
            if line_idx == 0 {
                if is_checklist {
                    render_checklist_box(
                        pages,
                        PDF_MARGIN_LEFT_PT + 2.0,
                        *y_top,
                        CHECKBOX_SIZE_PT,
                        checked,
                        palette,
                    );
                } else {
                    let bullet = if ordered {
                        format!("{}. ", idx + 1)
                    } else {
                        "- ".to_string()
                    };
                    push_text_op(
                        current_page_mut(pages),
                        TextStyle::body(paper_body_color(palette)),
                        BODY_FONT_SIZE_PT,
                        PDF_MARGIN_LEFT_PT,
                        y_pdf,
                        bullet,
                    );
                }
            }
            render_styled_line(pages, body_x, y_pdf, BODY_FONT_SIZE_PT, line_chars);
            *y_top += BODY_LINE_HEIGHT_PT;
        }
    }
    *y_top += PARAGRAPH_GAP_PT;
}

fn render_blockquote_block(
    pages: &mut Vec<Page>,
    y_top: &mut f32,
    max_top: f32,
    lines: &[String],
    palette: &PdfExportPalette,
    variable_names: &[String],
) {
    let quote_x = PDF_MARGIN_LEFT_PT + 10.0;
    let text_x = PDF_MARGIN_LEFT_PT + 16.0;
    let mut wrapped_lines: Vec<Vec<StyledChar>> = Vec::new();
    for line in lines {
        let mut base = TextStyle::body(paper_body_color(palette));
        base.italic = true;
        let chars = styled_chars_from_inline(line, variable_names, palette, base);
        wrapped_lines.extend(wrap_styled_chars(
            &chars,
            content_width() - 16.0,
            BODY_FONT_SIZE_PT,
        ));
    }

    if wrapped_lines.is_empty() {
        return;
    }

    let block_height = (wrapped_lines.len() as f32) * BODY_LINE_HEIGHT_PT;
    ensure_space(pages, y_top, block_height + PARAGRAPH_GAP_PT, max_top);

    let y1 = PDF_PAGE_HEIGHT_PT - *y_top;
    let y2 = PDF_PAGE_HEIGHT_PT - (*y_top + block_height);
    push_line_op(
        current_page_mut(pages),
        1.2,
        quote_x,
        y1,
        quote_x,
        y2,
        paper_border_color(palette),
    );

    for line in wrapped_lines {
        let y_pdf = PDF_PAGE_HEIGHT_PT - *y_top - BODY_FONT_SIZE_PT;
        render_styled_line(pages, text_x, y_pdf, BODY_FONT_SIZE_PT, &line);
        *y_top += BODY_LINE_HEIGHT_PT;
    }

    *y_top += PARAGRAPH_GAP_PT;
}

fn render_table_block(
    pages: &mut Vec<Page>,
    y_top: &mut f32,
    max_top: f32,
    table_lines: &[String],
    table_start_line_idx: usize,
    palette: &PdfExportPalette,
    variable_names: &[String],
    table_formula_values: &FxHashMap<(usize, usize), String>,
) {
    if table_lines.len() < 2 {
        render_paragraph_block(
            pages,
            y_top,
            max_top,
            &table_lines.join(" "),
            palette,
            variable_names,
        );
        return;
    }

    let mut rows: Vec<(usize, Vec<String>)> = Vec::new();
    for (line_offset, line) in table_lines.iter().enumerate() {
        if line_offset == 1 && is_table_delimiter(line) {
            continue;
        }
        let source_line_idx = table_start_line_idx + line_offset;
        let mut cells = split_table_cells_for_logical_row(line)
            .into_iter()
            .map(|cell| cell.trim().to_string())
            .collect::<Vec<_>>();
        for (cell_idx, cell) in cells.iter_mut().enumerate() {
            if let Some(value) = table_formula_values.get(&(source_line_idx, cell_idx)) {
                *cell = value.clone();
            }
        }
        rows.push((source_line_idx, cells));
    }

    let col_count = rows.iter().map(|(_, row)| row.len()).max().unwrap_or(0);
    if col_count == 0 {
        return;
    }

    let table_width = content_width();
    let col_width = table_width / (col_count as f32);
    let table_left = PDF_MARGIN_LEFT_PT;

    for (row_idx, (_, row)) in rows.iter().enumerate() {
        let mut cell_wrapped: Vec<Vec<Vec<StyledChar>>> = Vec::with_capacity(col_count);
        let mut max_line_count = 1usize;
        for col in 0..col_count {
            let text = row.get(col).cloned().unwrap_or_default();
            let mut base = TextStyle::body(paper_body_color(palette));
            if row_idx == 0 {
                base.bold = true;
            }
            let chars = styled_chars_from_inline(text.as_str(), variable_names, palette, base);
            let wrapped = wrap_styled_chars(
                &chars,
                col_width - (TABLE_CELL_PAD_X_PT * 2.0),
                TABLE_FONT_SIZE_PT,
            );
            max_line_count = max_line_count.max(wrapped.len().max(1));
            cell_wrapped.push(if wrapped.is_empty() {
                vec![Vec::new()]
            } else {
                wrapped
            });
        }

        let row_height =
            (max_line_count as f32) * TABLE_LINE_HEIGHT_PT + (TABLE_CELL_PAD_Y_PT * 2.0);
        let page_count_before = pages.len();
        ensure_space(pages, y_top, row_height + TABLE_BORDER_WIDTH_PT, max_top);
        let started_new_page = pages.len() != page_count_before;

        let top_y_pdf = PDF_PAGE_HEIGHT_PT - *y_top;
        let bottom_y_pdf = PDF_PAGE_HEIGHT_PT - (*y_top + row_height);

        let border = paper_border_color(palette);
        if row_idx == 0 || started_new_page {
            push_line_op(
                current_page_mut(pages),
                TABLE_BORDER_WIDTH_PT,
                table_left,
                top_y_pdf,
                table_left + table_width,
                top_y_pdf,
                border,
            );
        }
        push_line_op(
            current_page_mut(pages),
            TABLE_BORDER_WIDTH_PT,
            table_left,
            bottom_y_pdf,
            table_left + table_width,
            bottom_y_pdf,
            border,
        );

        for col in 0..=col_count {
            let x = table_left + (col as f32) * col_width;
            push_line_op(
                current_page_mut(pages),
                TABLE_BORDER_WIDTH_PT,
                x,
                top_y_pdf,
                x,
                bottom_y_pdf,
                border,
            );
        }

        for col in 0..col_count {
            let text_lines = &cell_wrapped[col];
            for (line_idx, line_chars) in text_lines.iter().enumerate() {
                let y_text = PDF_PAGE_HEIGHT_PT
                    - *y_top
                    - TABLE_CELL_PAD_Y_PT
                    - (line_idx as f32) * TABLE_LINE_HEIGHT_PT
                    - TABLE_FONT_SIZE_PT;
                render_styled_line(
                    pages,
                    table_left + (col as f32) * col_width + TABLE_CELL_PAD_X_PT,
                    y_text,
                    TABLE_FONT_SIZE_PT,
                    line_chars,
                );
            }
        }

        *y_top += row_height;
    }

    *y_top += PARAGRAPH_GAP_PT;
}

fn collect_table_formula_display_values(lines: &[String]) -> FxHashMap<(usize, usize), String> {
    let options = NoteEvaluationOptions {
        variables_enabled: true,
        table_enabled: true,
        eval_range: None,
    };
    let result = CalcEngine::new().evaluate_note_context(lines, options);
    let mut out = FxHashMap::default();
    for (line_idx, row) in result.table_cell_results.iter().enumerate() {
        for cell in row {
            let value = calc_plan::format_formula_display_value(&cell.value);
            if !value.trim().is_empty() {
                out.insert((line_idx, cell.cell_index), value);
            }
        }
    }
    out
}

fn render_image(
    pages: &mut Vec<Page>,
    y_top: &mut f32,
    max_top: f32,
    image_name: &str,
    image: &PdfImageObject,
) {
    let max_width = content_width();
    let natural_w = image.width as f32;
    let natural_h = image.height as f32;

    let scale = (max_width / natural_w)
        .min(IMAGE_MAX_HEIGHT_PT / natural_h)
        .min(1.0);

    let draw_w = natural_w * scale;
    let draw_h = natural_h * scale;

    ensure_space(pages, y_top, draw_h + PARAGRAPH_GAP_PT, max_top);

    let x = PDF_MARGIN_LEFT_PT + ((max_width - draw_w) / 2.0);
    let y = PDF_PAGE_HEIGHT_PT - *y_top - draw_h;

    current_page_mut(pages).ops.push(DrawOp::Image {
        name: image_name.to_string(),
        x,
        y,
        w: draw_w,
        h: draw_h,
    });
    current_page_mut(pages)
        .used_images
        .insert(image_name.to_string());

    *y_top += draw_h + PARAGRAPH_GAP_PT;
}

fn heading_font_size(level: usize) -> f32 {
    match level {
        1 => 24.0,
        2 => 20.0,
        3 => 17.0,
        4 => 15.0,
        5 => 13.0,
        _ => 12.0,
    }
}

fn parse_heading(line: &str) -> Option<(usize, String)> {
    let trimmed = line.trim_start();
    let mut hashes = 0usize;
    for ch in trimmed.chars() {
        if ch == '#' {
            hashes += 1;
        } else {
            break;
        }
    }
    if hashes == 0 || hashes > 6 {
        return None;
    }
    let rest = trimmed.get(hashes..)?.trim_start();
    if rest.is_empty() {
        return None;
    }
    Some((hashes, rest.to_string()))
}

fn is_fence_start(trimmed: &str) -> bool {
    trimmed.starts_with("```")
}

fn collect_code_fence(lines: &[&str], start: usize) -> (Vec<String>, Option<String>, usize) {
    let code_lang = markdown_tokens::parse_fence_language(lines[start]);
    let mut out = Vec::new();
    let mut i = start + 1;
    while i < lines.len() {
        let line = lines[i];
        if line.trim_start().starts_with("```") {
            return (out, code_lang, i + 1);
        }
        out.push(line.to_string());
        i += 1;
    }
    (out, code_lang, i)
}

fn is_markdown_table_start(lines: &[&str], index: usize) -> bool {
    if index + 1 >= lines.len() {
        return false;
    }
    let first = lines[index];
    let second = lines[index + 1];
    is_table_line(first) && is_table_delimiter(second)
}

fn collect_table_block(lines: &[&str], start: usize) -> (Vec<String>, usize) {
    let mut out = Vec::new();
    let mut i = start;
    while i < lines.len() {
        let line = lines[i];
        if !is_table_line(line) {
            break;
        }
        out.push(line.to_string());
        i += 1;
    }
    (out, i)
}

fn is_table_delimiter(line: &str) -> bool {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"^\s*\|?\s*:?-{3,}:?\s*(\|\s*:?-{3,}:?\s*)+\|?\s*$")
            .expect("table delimiter regex must compile")
    })
    .is_match(line)
}

fn is_unordered_list_item(line: &str) -> bool {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^\s*[-*+]\s+").expect("unordered list regex"))
        .is_match(line)
}

fn is_ordered_list_item(line: &str) -> bool {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^\s*\d+\.\s+").expect("ordered list regex"))
        .is_match(line)
}

fn strip_list_marker(line: &str) -> String {
    static UNORDERED: OnceLock<Regex> = OnceLock::new();
    static ORDERED: OnceLock<Regex> = OnceLock::new();
    let unordered =
        UNORDERED.get_or_init(|| Regex::new(r"^\s*[-*+]\s+").expect("unordered marker regex"));
    if unordered.is_match(line) {
        return unordered.replace(line, "").to_string();
    }
    let ordered =
        ORDERED.get_or_init(|| Regex::new(r"^\s*\d+\.\s+").expect("ordered marker regex"));
    if ordered.is_match(line) {
        return ordered.replace(line, "").to_string();
    }
    line.trim().to_string()
}

fn collect_list_block(lines: &[&str], start: usize) -> (Vec<String>, bool, usize) {
    let ordered = is_ordered_list_item(lines[start]);
    let mut out = Vec::new();
    let mut i = start;
    while i < lines.len() {
        let line = lines[i];
        let same_kind = if ordered {
            is_ordered_list_item(line)
        } else {
            is_unordered_list_item(line)
        };
        if !same_kind {
            break;
        }
        out.push(strip_list_marker(line));
        i += 1;
    }
    (out, ordered, i)
}

fn collect_blockquote(lines: &[&str], start: usize) -> (Vec<String>, usize) {
    let mut out = Vec::new();
    let mut i = start;
    while i < lines.len() {
        let line = lines[i].trim_start();
        if !line.starts_with('>') {
            break;
        }
        out.push(line.trim_start_matches('>').trim_start().to_string());
        i += 1;
    }
    (out, i)
}

fn looks_like_assignment_line(line: &str) -> bool {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return false;
    }
    let Some(assign_idx) = trimmed.find(":=") else {
        return false;
    };
    let (name_part, rhs_part) = trimmed.split_at(assign_idx);
    let name = name_part.trim();
    if name.is_empty()
        || !name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
    {
        return false;
    }
    let rhs = rhs_part[2..].trim();
    !rhs.is_empty()
}

fn should_preserve_hard_linebreaks(lines: &[String]) -> bool {
    if lines.len() < 2 {
        return false;
    }
    let assignment_lines = lines
        .iter()
        .filter(|line| looks_like_assignment_line(line))
        .count();
    assignment_lines * 2 >= lines.len()
}

fn collect_paragraph(lines: &[&str], start: usize) -> (Vec<String>, usize) {
    let mut out = Vec::new();
    let mut i = start;
    while i < lines.len() {
        let line = lines[i];
        let trimmed = line.trim();
        if trimmed.is_empty()
            || parse_heading(line).is_some()
            || is_fence_start(trimmed)
            || is_markdown_table_start(lines, i)
            || is_unordered_list_item(line)
            || is_ordered_list_item(line)
            || trimmed.starts_with('>')
            || parse_image_only_line(trimmed).is_some()
        {
            break;
        }
        out.push(line.trim().to_string());
        i += 1;
    }
    (out, i)
}

fn parse_image_only_line(line: &str) -> Option<(String, String)> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re =
        RE.get_or_init(|| Regex::new(r"^\s*!\[([^\]]*)\]\(([^)]+)\)\s*$").expect("image regex"));
    let caps = re.captures(line)?;
    let alt = caps.get(1)?.as_str().trim().to_string();
    let src = caps.get(2)?.as_str().trim().to_string();
    Some((alt, src))
}

fn alt_if_empty(alt: &str) -> String {
    if alt.trim().is_empty() {
        "image".to_string()
    } else {
        alt.to_string()
    }
}

fn collect_image_sources(content: &str) -> Vec<String> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"!\[[^\]]*\]\(([^)]+)\)").expect("image source regex"));
    let mut set = BTreeSet::new();
    for caps in re.captures_iter(content) {
        if let Some(src) = caps.get(1) {
            let value = src.as_str().trim();
            if !value.is_empty() {
                set.insert(value.to_string());
            }
        }
    }
    set.into_iter().collect()
}

fn serialize_pdf(
    pages: Vec<Page>,
    image_assets: Vec<(String, PdfImageObject)>,
) -> Result<Vec<u8>, String> {
    let unicode_font = resolve_unicode_pdf_font_asset();
    let page_count = pages.len().max(1);
    let actual_pages = if pages.is_empty() {
        vec![Page::default()]
    } else {
        pages
    };

    let mut next_object_id = 8usize;
    let unicode_ids = if unicode_font.is_some() {
        let ids = (
            next_object_id,
            next_object_id + 1,
            next_object_id + 2,
            next_object_id + 3,
            next_object_id + 4,
        );
        next_object_id += 5;
        Some(ids)
    } else {
        None
    };

    let image_count = image_assets.len();
    let image_first_id = next_object_id;
    let page_first_id = image_first_id + image_count;

    let mut image_obj_id_by_name: FxHashMap<String, usize> = FxHashMap::default();
    for (idx, (name, _)) in image_assets.iter().enumerate() {
        image_obj_id_by_name.insert(name.clone(), image_first_id + idx);
    }

    let mut objects: Vec<Vec<u8>> = Vec::new();

    objects.push(b"<< /Type /Catalog /Pages 2 0 R >>\n".to_vec());

    let mut kids = String::new();
    for i in 0..page_count {
        let page_obj_id = page_first_id + (i * 2);
        kids.push_str(&format!("{page_obj_id} 0 R "));
    }
    objects.push(
        format!(
            "<< /Type /Pages /Kids [{}] /Count {} >>\n",
            kids, page_count
        )
        .into_bytes(),
    );

    objects.push(b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>\n".to_vec());
    objects.push(b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica-Bold >>\n".to_vec());
    objects.push(b"<< /Type /Font /Subtype /Type1 /BaseFont /Courier >>\n".to_vec());
    objects.push(b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica-Oblique >>\n".to_vec());
    objects.push(b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica-BoldOblique >>\n".to_vec());

    let mut unicode_cmap: FxHashMap<u16, char> = FxHashMap::default();
    let streams: Vec<String> = actual_pages
        .iter()
        .map(|page| page_stream(page, unicode_font.as_ref(), &mut unicode_cmap))
        .collect();

    if let (Some(font), Some((type0_id, cid_id, descriptor_id, font_file_id, to_unicode_id))) =
        (unicode_font.as_ref(), unicode_ids)
    {
        let base_font_name = "SlateUnicode";
        objects.push(
            format!(
                "<< /Type /Font /Subtype /Type0 /BaseFont /{} /Encoding /Identity-H /DescendantFonts [{} 0 R] /ToUnicode {} 0 R >>\n",
                base_font_name, cid_id, to_unicode_id
            )
            .into_bytes(),
        );
        let default_width = 1000i32;
        objects.push(
            format!(
                "<< /Type /Font /Subtype /CIDFontType2 /BaseFont /{} /CIDSystemInfo << /Registry (Adobe) /Ordering (Identity) /Supplement 0 >> /FontDescriptor {} 0 R /CIDToGIDMap /Identity /DW {} >>\n",
                base_font_name, descriptor_id, default_width
            )
            .into_bytes(),
        );
        let scale = 1000.0f32 / (font.units_per_em as f32);
        let ascent = (font.ascent as f32 * scale).round() as i32;
        let descent = (font.descent as f32 * scale).round() as i32;
        let cap_height = (font.cap_height as f32 * scale).round() as i32;
        let bbox_min_x = (font.bbox_min_x as f32 * scale).round() as i32;
        let bbox_min_y = (font.bbox_min_y as f32 * scale).round() as i32;
        let bbox_max_x = (font.bbox_max_x as f32 * scale).round() as i32;
        let bbox_max_y = (font.bbox_max_y as f32 * scale).round() as i32;
        objects.push(
            format!(
                "<< /Type /FontDescriptor /FontName /{} /Flags {} /FontBBox [{} {} {} {}] /ItalicAngle 0 /Ascent {} /Descent {} /CapHeight {} /StemV {} /FontFile2 {} 0 R >>\n",
                base_font_name,
                font.flags,
                bbox_min_x,
                bbox_min_y,
                bbox_max_x,
                bbox_max_y,
                ascent,
                descent,
                cap_height,
                font.stem_v,
                font_file_id
            )
            .into_bytes(),
        );
        let mut font_stream = format!(
            "<< /Length {} /Length1 {} >>\nstream\n",
            font.bytes.len(),
            font.bytes.len()
        )
        .into_bytes();
        font_stream.extend_from_slice(&font.bytes);
        font_stream.extend_from_slice(b"\nendstream\n");
        objects.push(font_stream);
        let to_unicode_stream = build_to_unicode_cmap(&unicode_cmap);
        let mut to_unicode_obj =
            format!("<< /Length {} >>\nstream\n", to_unicode_stream.len()).into_bytes();
        to_unicode_obj.extend_from_slice(&to_unicode_stream);
        to_unicode_obj.extend_from_slice(b"\nendstream\n");
        objects.push(to_unicode_obj);

        debug_assert_eq!(type0_id, 8);
        debug_assert_eq!(cid_id, 9);
    }

    for (_, image) in &image_assets {
        let mut body = format!(
            "<< /Type /XObject /Subtype /Image /Width {} /Height {} /ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /FlateDecode /Length {} >>\nstream\n",
            image.width,
            image.height,
            image.compressed_rgb.len()
        )
        .into_bytes();
        body.extend_from_slice(&image.compressed_rgb);
        body.extend_from_slice(b"\nendstream\n");
        objects.push(body);
    }

    for (idx, page) in actual_pages.iter().enumerate() {
        let page_obj_id = page_first_id + (idx * 2);
        let content_obj_id = page_obj_id + 1;

        let mut xobjects = String::new();
        for name in &page.used_images {
            if let Some(id) = image_obj_id_by_name.get(name) {
                xobjects.push_str(&format!("/{name} {id} 0 R "));
            }
        }
        let xobject_section = if xobjects.is_empty() {
            String::new()
        } else {
            format!(" /XObject << {xobjects} >>")
        };
        let unicode_font_section = if let Some((type0_id, _, _, _, _)) = unicode_ids {
            format!(" /F6 {} 0 R", type0_id)
        } else {
            String::new()
        };

        objects.push(
            format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {PDF_PAGE_WIDTH_PT:.0} {PDF_PAGE_HEIGHT_PT:.0}] /Resources << /Font << /F1 3 0 R /F2 4 0 R /F3 5 0 R /F4 6 0 R /F5 7 0 R{unicode_font_section} >>{xobject_section} >> /Contents {content_obj_id} 0 R >>\n"
            )
            .into_bytes(),
        );

        let stream = &streams[idx];
        let mut content_obj = format!("<< /Length {} >>\nstream\n", stream.len()).into_bytes();
        content_obj.extend_from_slice(stream.as_bytes());
        content_obj.extend_from_slice(b"\nendstream\n");
        objects.push(content_obj);
    }

    debug_assert_eq!(actual_pages.len(), page_count);
    serialize_objects(objects)
}

fn page_stream(
    page: &Page,
    unicode_font: Option<&PdfUnicodeFontAsset>,
    unicode_cmap: &mut FxHashMap<u16, char>,
) -> String {
    let mut out = String::new();
    for op in &page.ops {
        match op {
            DrawOp::Text {
                font,
                size,
                x,
                y,
                color,
                text,
            } => {
                let (r, g, b) = color.as_pdf_rgb();
                let (font_resource, encoded) = if let Some(unicode) = unicode_font {
                    if text_requires_unicode_font(text) {
                        (
                            "F6",
                            encode_pdf_unicode_text_bytes(text, unicode, unicode_cmap),
                        )
                    } else {
                        (font.resource_name(), encode_pdf_text_bytes(text))
                    }
                } else {
                    (font.resource_name(), encode_pdf_text_bytes(text))
                };
                if encoded.is_empty() {
                    continue;
                }
                let hex = encode_pdf_hex_string(&encoded);
                out.push_str("BT\n");
                out.push_str(&format!("/{} {:.2} Tf\n", font_resource, size));
                out.push_str(&format!("{r:.4} {g:.4} {b:.4} rg\n"));
                out.push_str(&format!("1 0 0 1 {:.2} {:.2} Tm <{}> Tj\n", x, y, hex));
                out.push_str("ET\n");
            }
            DrawOp::Line {
                width,
                x1,
                y1,
                x2,
                y2,
                color,
            } => {
                let (r, g, b) = color.as_pdf_rgb();
                out.push_str(&format!(
                    "{r:.4} {g:.4} {b:.4} RG\n{width:.2} w\n{x1:.2} {y1:.2} m\n{x2:.2} {y2:.2} l\nS\n"
                ));
            }
            DrawOp::Image { name, x, y, w, h } => {
                out.push_str("q\n");
                out.push_str(&format!("{w:.2} 0 0 {h:.2} {x:.2} {y:.2} cm\n/{name} Do\n"));
                out.push_str("Q\n");
            }
            DrawOp::Rect {
                x,
                y,
                w,
                h,
                fill,
                stroke_width,
                stroke,
            } => {
                if let Some(fill_color) = fill {
                    let (r, g, b) = fill_color.as_pdf_rgb();
                    out.push_str("q\n");
                    out.push_str(&format!(
                        "{r:.4} {g:.4} {b:.4} rg\n{x:.2} {y:.2} {w:.2} {h:.2} re f\n"
                    ));
                    out.push_str("Q\n");
                }
                if let (Some(width), Some(stroke_color)) = (stroke_width, stroke) {
                    let (r, g, b) = stroke_color.as_pdf_rgb();
                    out.push_str("q\n");
                    out.push_str(&format!(
                        "{r:.4} {g:.4} {b:.4} RG\n{width:.2} w\n{x:.2} {y:.2} {w:.2} {h:.2} re S\n"
                    ));
                    out.push_str("Q\n");
                }
            }
        }
    }
    out
}

fn serialize_objects(objects: Vec<Vec<u8>>) -> Result<Vec<u8>, String> {
    let mut bytes: Vec<u8> = Vec::new();
    bytes.extend_from_slice(b"%PDF-1.4\n%\xE2\xE3\xCF\xD3\n");

    let mut offsets: Vec<usize> = Vec::with_capacity(objects.len() + 1);
    offsets.push(0);

    for (idx, object_body) in objects.iter().enumerate() {
        offsets.push(bytes.len());
        let object_id = idx + 1;
        bytes.extend_from_slice(format!("{object_id} 0 obj\n").as_bytes());
        bytes.extend_from_slice(object_body);
        bytes.extend_from_slice(b"endobj\n");
    }

    let xref_offset = bytes.len();
    bytes.extend_from_slice(format!("xref\n0 {}\n", objects.len() + 1).as_bytes());
    bytes.extend_from_slice(b"0000000000 65535 f \n");

    for offset in offsets.iter().skip(1) {
        bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }

    bytes.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{}\n%%EOF\n",
            objects.len() + 1,
            xref_offset
        )
        .as_bytes(),
    );

    Ok(bytes)
}

fn encode_pdf_hex_string(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0F) as usize] as char);
    }
    out
}

fn encode_pdf_text_bytes(input: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(input.len());
    for ch in input.chars() {
        if let Some(byte) = unicode_to_winansi_byte(ch) {
            out.push(byte);
            continue;
        }
        match ch {
            '\t' => out.extend_from_slice(b"    "),
            '\u{00a0}' => out.push(b' '), // nbsp
            '☐' => out.extend_from_slice(b"[ ]"),
            '☑' | '☒' | '✅' => out.extend_from_slice(b"[x]"),
            _ => out.push(b'?'),
        }
    }
    out
}

fn unicode_to_winansi_byte(ch: char) -> Option<u8> {
    let code = ch as u32;
    if (0x20..=0x7e).contains(&code) {
        return Some(code as u8);
    }
    if (0xa0..=0xff).contains(&code) {
        return Some(code as u8);
    }
    match code {
        0x20ac => Some(0x80), // €
        0x201a => Some(0x82), // ‚
        0x0192 => Some(0x83), // ƒ
        0x201e => Some(0x84), // „
        0x2026 => Some(0x85), // …
        0x2020 => Some(0x86), // †
        0x2021 => Some(0x87), // ‡
        0x02c6 => Some(0x88), // ˆ
        0x2030 => Some(0x89), // ‰
        0x0160 => Some(0x8a), // Š
        0x2039 => Some(0x8b), // ‹
        0x0152 => Some(0x8c), // Œ
        0x017d => Some(0x8e), // Ž
        0x2018 => Some(0x91), // ‘
        0x2019 => Some(0x92), // ’
        0x201c => Some(0x93), // “
        0x201d => Some(0x94), // ”
        0x2022 => Some(0x95), // •
        0x2013 => Some(0x96), // –
        0x2014 => Some(0x97), // —
        0x02dc => Some(0x98), // ˜
        0x2122 => Some(0x99), // ™
        0x0161 => Some(0x9a), // š
        0x203a => Some(0x9b), // ›
        0x0153 => Some(0x9c), // œ
        0x017e => Some(0x9e), // ž
        0x0178 => Some(0x9f), // Ÿ
        _ => None,
    }
}

fn text_requires_unicode_font(text: &str) -> bool {
    text.chars()
        .any(|ch| ch != '\t' && unicode_to_winansi_byte(ch).is_none() && !ch.is_ascii_control())
}

fn encode_pdf_unicode_text_bytes(
    input: &str,
    font: &PdfUnicodeFontAsset,
    unicode_cmap: &mut FxHashMap<u16, char>,
) -> Vec<u8> {
    let Ok(face) = Face::parse(&font.bytes, 0) else {
        return Vec::new();
    };
    let mut out = Vec::with_capacity(input.chars().count() * 2);
    for ch in input.chars() {
        if ch == '\t' {
            for _ in 0..PDF_TAB_WIDTH {
                let gid = face
                    .glyph_index(' ')
                    .map(|id| id.0)
                    .or_else(|| face.glyph_index('?').map(|id| id.0))
                    .unwrap_or(font.fallback_gid);
                out.push((gid >> 8) as u8);
                out.push((gid & 0xff) as u8);
                unicode_cmap.entry(gid).or_insert(' ');
            }
            continue;
        }
        if ch.is_ascii_control() && ch != '\t' && ch != ' ' {
            continue;
        }
        let normalized = if ch == '\u{00a0}' { ' ' } else { ch };
        let gid = face
            .glyph_index(normalized)
            .map(|id| id.0)
            .or_else(|| face.glyph_index('?').map(|id| id.0))
            .unwrap_or(font.fallback_gid);
        out.push((gid >> 8) as u8);
        out.push((gid & 0xff) as u8);
        unicode_cmap.entry(gid).or_insert(normalized);
    }
    out
}

fn expand_tabs_for_pdf(input: &str, tab_width: usize) -> String {
    if tab_width == 0 || !input.contains('\t') {
        return input.to_string();
    }
    let mut out = String::with_capacity(input.len());
    let mut col = 0usize;
    for ch in input.chars() {
        if ch == '\t' {
            let step = tab_width - (col % tab_width);
            for _ in 0..step {
                out.push(' ');
                col += 1;
            }
        } else {
            out.push(ch);
            if ch == '\n' || ch == '\r' {
                col = 0;
            } else {
                col += 1;
            }
        }
    }
    out
}

fn utf16be_hex_for_char(ch: char) -> String {
    let mut out = String::new();
    let code = ch as u32;
    if code <= 0xFFFF {
        out.push_str(&format!("{:04X}", code));
        return out;
    }
    let scalar = code - 0x1_0000;
    let high = 0xD800 + ((scalar >> 10) as u16);
    let low = 0xDC00 + ((scalar & 0x3FF) as u16);
    out.push_str(&format!("{:04X}{:04X}", high, low));
    out
}

fn build_to_unicode_cmap(unicode_cmap: &FxHashMap<u16, char>) -> Vec<u8> {
    let mut entries: Vec<(u16, char)> = unicode_cmap.iter().map(|(k, v)| (*k, *v)).collect();
    entries.sort_by_key(|(gid, _)| *gid);
    let mut out = String::new();
    out.push_str("/CIDInit /ProcSet findresource begin\n");
    out.push_str("12 dict begin\n");
    out.push_str("begincmap\n");
    out.push_str("/CIDSystemInfo << /Registry (Adobe) /Ordering (Identity) /Supplement 0 >> def\n");
    out.push_str("/CMapName /SlateUnicodeToUnicode def\n");
    out.push_str("/CMapType 2 def\n");
    out.push_str("1 begincodespacerange\n<0000> <FFFF>\nendcodespacerange\n");
    if entries.is_empty() {
        out.push_str("0 beginbfchar\nendbfchar\n");
    } else {
        for chunk in entries.chunks(100) {
            out.push_str(&format!("{} beginbfchar\n", chunk.len()));
            for (gid, ch) in chunk {
                out.push_str(&format!("<{:04X}> <{}>\n", gid, utf16be_hex_for_char(*ch)));
            }
            out.push_str("endbfchar\n");
        }
    }
    out.push_str("endcmap\n");
    out.push_str("CMapName currentdict /CMap defineresource pop\n");
    out.push_str("end\nend\n");
    out.into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::codecs::png::PngEncoder;
    use image::{ColorType, ImageEncoder, Rgb, RgbImage};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn unique_suffix() -> u128 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after unix epoch")
            .as_nanos()
    }

    fn tiny_png_bytes() -> Vec<u8> {
        let image = RgbImage::from_pixel(1, 1, Rgb([255, 0, 0]));
        let mut out = Vec::new();
        PngEncoder::new(&mut out)
            .write_image(image.as_raw(), 1, 1, ColorType::Rgb8.into())
            .expect("png encode");
        out
    }

    #[test]
    fn markdown_pdf_contains_headings_table_and_text() {
        let source =
            "# Title\n\n| Name | Score |\n| --- | ---: |\n| Ana | 5 |\n\n- item one\n- item two";
        let bytes = build_markdown_pdf(source, &PdfExportPalette::default(), |_| None)
            .expect("pdf generation should succeed");
        let text = String::from_utf8_lossy(&bytes);

        assert!(text.contains("%PDF-1.4"));
        assert!(pdf_contains_text(&text, "Title"));
        assert!(pdf_contains_text(&text, "Name"));
        assert!(pdf_contains_text(&text, "Score"));
        assert!(pdf_contains_text(&text, "item one"));
        assert!(text.contains("xref"));
    }

    #[test]
    fn markdown_pdf_renders_unordered_list_markers_as_ascii_dash() {
        let source = "- one\n- two";
        let bytes = build_markdown_pdf(source, &PdfExportPalette::default(), |_| None)
            .expect("pdf generation should succeed");
        let text = String::from_utf8_lossy(&bytes);

        assert!(pdf_contains_text(&text, "- "));
        assert!(pdf_contains_text(&text, "one"));
        assert!(pdf_contains_text(&text, "two"));
    }

    #[test]
    fn markdown_pdf_embeds_images_when_resolver_returns_bytes() {
        let source = "![tiny](./assets/tiny.png)";
        let bytes = build_markdown_pdf(source, &PdfExportPalette::default(), |src| {
            if src == "./assets/tiny.png" {
                Some(tiny_png_bytes())
            } else {
                None
            }
        })
        .expect("pdf with image should build");

        let text = String::from_utf8_lossy(&bytes);
        assert!(text.contains("/Subtype /Image"));
        assert!(text.contains("/Im1 Do"));
    }

    #[test]
    fn parse_image_only_line_extracts_alt_and_source() {
        let parsed = parse_image_only_line("![Alt Text](./assets/a.png)").expect("image line");
        assert_eq!(parsed.0, "Alt Text");
        assert_eq!(parsed.1, "./assets/a.png");
    }

    #[test]
    fn decode_data_url_image_bytes_rejects_non_base64() {
        let err = decode_data_url_image_bytes("data:image/png,abc").expect_err("expected error");
        assert!(err.contains("non-base64"));
    }

    #[test]
    fn resolve_export_path_expands_tilde_prefix() {
        let raw = format!("~/slate-export-{}.txt", unique_suffix());
        let resolved = resolve_export_path(&raw).expect("tilde path should resolve");
        let home = resolve_home_dir().expect("home should be set");
        assert!(resolved.starts_with(home));
        assert!(resolved.to_string_lossy().contains("slate-export-"));
    }

    #[test]
    fn export_to_file_supports_tilde_paths() {
        let relative = format!("slate-export-{}.txt", unique_suffix());
        let raw = format!("~/{}", relative);
        let home = resolve_home_dir().expect("home should be set");
        let full_path = home.join(relative);
        match export_to_file_blocking(&raw, "hello") {
            Ok(()) => {
                let read_back =
                    fs::read_to_string(&full_path).expect("exported file should be readable");
                assert_eq!(read_back, "hello");
                let _ = fs::remove_file(full_path);
            }
            Err(error) => {
                let lowered = error.to_ascii_lowercase();
                assert!(error.contains(full_path.to_string_lossy().as_ref()));
                assert!(
                    lowered.contains("read-only file system")
                        || lowered.contains("permission denied"),
                    "unexpected export error: {error}"
                );
            }
        }
    }

    #[test]
    fn resolve_export_path_accepts_file_uri_paths() {
        let target = std::env::temp_dir().join(format!("slate export {}.pdf", unique_suffix()));
        let uri = Url::from_file_path(&target)
            .expect("temp path should convert to file URI")
            .to_string();
        let resolved = resolve_export_path(&uri).expect("file URI should resolve");
        assert_eq!(resolved, target);
    }

    #[test]
    fn resolve_export_path_strips_wrapping_quotes() {
        let target = std::env::temp_dir().join(format!("slate-export-{}.pdf", unique_suffix()));
        let raw = format!("\"{}\"", target.display());
        let resolved = resolve_export_path(&raw).expect("quoted path should resolve");
        assert_eq!(resolved, target);
    }

    #[test]
    fn resolve_export_path_rejects_missing_parent_directory() {
        let missing_dir = std::env::temp_dir().join(format!("slate-no-dir-{}", unique_suffix()));
        let target = missing_dir.join("out.pdf");
        let err = resolve_export_path(target.to_string_lossy().as_ref())
            .expect_err("missing parent should fail");
        assert!(err.contains("does not exist"));
    }

    #[test]
    fn markdown_pdf_applies_inline_and_code_theme_styles_and_renders_checklist() {
        let source = "- [x] **Done** *soon*\n\ncount := 1\ncount + 2\n\n```rust\nlet x = 1;\n```";
        let palette = PdfExportPalette {
            fg: PdfRgbColor { r: 1, g: 2, b: 3 },
            fg_dim: PdfRgbColor { r: 4, g: 5, b: 6 },
            accent: PdfRgbColor { r: 7, g: 8, b: 9 },
            variable: PdfRgbColor { r: 255, g: 0, b: 0 },
            code_keyword: PdfRgbColor { r: 0, g: 255, b: 0 },
            code_string: PdfRgbColor { r: 0, g: 0, b: 255 },
            code_number: PdfRgbColor {
                r: 120,
                g: 80,
                b: 40,
            },
            code_comment: PdfRgbColor {
                r: 20,
                g: 30,
                b: 40,
            },
            code_function: PdfRgbColor {
                r: 80,
                g: 90,
                b: 100,
            },
            code_type: PdfRgbColor {
                r: 110,
                g: 120,
                b: 130,
            },
        };
        let bytes = build_markdown_pdf(source, &palette, |_| None).expect("pdf generation");
        let text = String::from_utf8_lossy(&bytes);

        assert!(pdf_contains_text(&text, "Done"));
        assert!(text.contains("/F2"));
        assert!(text.contains("/F4"));
        assert!(text.contains("1.0000 0.0000 0.0000 rg"));
        assert!(text.contains("0.0000 1.0000 0.0000 rg"));
        assert!(!pdf_contains_text(&text, "[x]"));
    }

    #[test]
    fn strikethrough_lines_are_split_across_whitespace_gaps() {
        let mut pages = vec![Page::default()];
        let style = TextStyle {
            mono: false,
            bold: false,
            italic: false,
            strikethrough: true,
            color: pdf_black(),
        };
        let line = vec![
            StyledChar { ch: 'a', style },
            StyledChar { ch: ' ', style },
            StyledChar { ch: 'b', style },
        ];
        render_styled_line(&mut pages, 10.0, 20.0, BODY_FONT_SIZE_PT, &line);
        let page = pages.last().expect("page");
        let strike_count = page
            .ops
            .iter()
            .filter(|op| matches!(op, DrawOp::Line { .. }))
            .count();
        assert_eq!(strike_count, 2);
    }

    #[test]
    fn styled_chars_from_inline_preserves_spacing_around_code_and_links() {
        let palette = PdfExportPalette::default();
        let variable_names: Vec<String> = Vec::new();
        let source = "Add assignment-trailer evaluation support (`val := a - b = 44` style reconciliation on tab). Link handling polish for [text](url) display behavior.";
        let styled = styled_chars_from_inline(
            source,
            &variable_names,
            &palette,
            TextStyle::body(pdf_black()),
        );
        let flattened: String = styled.into_iter().map(|entry| entry.ch).collect();
        assert_eq!(
            flattened,
            "Add assignment-trailer evaluation support (val := a - b = 44 style reconciliation on tab). Link handling polish for text display behavior."
        );
    }

    #[test]
    fn styled_chars_from_inline_highlights_variables_case_insensitively() {
        let palette = PdfExportPalette {
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
                r: 220,
                g: 80,
                b: 40,
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
        };
        let styled = styled_chars_from_inline(
            "MoN := 41",
            &["mon".to_string()],
            &palette,
            TextStyle::body(pdf_black()),
        );
        let var_chars = styled
            .iter()
            .filter(|entry| matches!(entry.ch, 'M' | 'o' | 'N'))
            .collect::<Vec<_>>();
        assert_eq!(var_chars.len(), 3);
        for entry in var_chars {
            assert!(entry.style.bold);
            assert_eq!(entry.style.color, palette.variable);
        }
    }

    #[test]
    fn markdown_pdf_does_not_collapse_code_space_before_following_text() {
        let source =
            "- [ ] Add assignment-trailer evaluation support (`val := a - b = 44` style reconciliation on tab).";
        let bytes = build_markdown_pdf(source, &PdfExportPalette::default(), |_| None)
            .expect("pdf generation should succeed");
        let text = String::from_utf8_lossy(&bytes);
        assert!(
            !text.contains("44style"),
            "inline code boundary collapsed required whitespace"
        );
    }

    #[test]
    fn encode_pdf_text_bytes_preserves_common_unicode_punctuation() {
        let encoded = encode_pdf_text_bytes("• “quote” – …");
        assert_eq!(
            encoded,
            vec![0x95, 0x20, 0x93, 0x71, 0x75, 0x6f, 0x74, 0x65, 0x94, 0x20, 0x96, 0x20, 0x85]
        );
    }

    #[test]
    fn encode_pdf_text_bytes_keeps_checkbox_fallbacks() {
        let encoded = encode_pdf_text_bytes("☐ ☑");
        assert_eq!(encoded, b"[ ] [x]");
    }

    #[test]
    fn styled_chars_for_code_line_expands_tabs() {
        let chars =
            styled_chars_for_code_line("\tif x {\t}", Some("go"), &PdfExportPalette::default());
        let text: String = chars.into_iter().map(|entry| entry.ch).collect();
        assert_eq!(text, "    if x {  }");
    }

    #[test]
    fn pdf_uses_hex_encoded_text_runs() {
        let source = "Title";
        let bytes = build_markdown_pdf(source, &PdfExportPalette::default(), |_| None)
            .expect("pdf generation should succeed");
        let text = String::from_utf8_lossy(&bytes);
        assert!(text.contains("<5469746C65> Tj"));
    }

    #[test]
    fn text_requires_unicode_font_flags_non_winansi() {
        assert!(!text_requires_unicode_font("hello"));
        assert!(text_requires_unicode_font("Привет"));
        assert!(text_requires_unicode_font("你好"));
    }

    #[test]
    fn markdown_pdf_uses_unicode_font_for_non_winansi_when_available() {
        if resolve_unicode_pdf_font_asset().is_none() {
            return;
        }
        let source = "Unicode: Привет 你好";
        let bytes = build_markdown_pdf(source, &PdfExportPalette::default(), |_| None)
            .expect("pdf generation should succeed");
        let text = String::from_utf8_lossy(&bytes);
        assert!(text.contains("/Subtype /Type0"));
        assert!(text.contains("/Subtype /CIDFontType2"));
        assert!(text.contains("/F6"));
    }

    #[test]
    fn preserve_hard_linebreaks_for_assignment_dense_paragraphs() {
        let dense = vec![
            "a := 1".to_string(),
            "b := a + 2".to_string(),
            "c := b + 3".to_string(),
        ];
        assert!(should_preserve_hard_linebreaks(&dense));

        let prose = vec![
            "This is a paragraph.".to_string(),
            "It should still wrap naturally.".to_string(),
        ];
        assert!(!should_preserve_hard_linebreaks(&prose));
    }

    #[test]
    fn markdown_pdf_keeps_assignment_lines_separate() {
        let source = "a := 1\nb := 2\nc := 3";
        let pages =
            render_markdown_to_pages(source, &PdfExportPalette::default(), &FxHashMap::default(), &[]);

        let mut y_by_var = FxHashMap::<String, f32>::new();
        for page in pages {
            for op in page.ops {
                if let DrawOp::Text { y, text, .. } = op {
                    let token = text.trim();
                    if matches!(token, "a" | "b" | "c") {
                        y_by_var.insert(token.to_string(), y);
                    }
                }
            }
        }

        let a_y = y_by_var.get("a").expect("a should render");
        let b_y = y_by_var.get("b").expect("b should render");
        let c_y = y_by_var.get("c").expect("c should render");
        assert!(a_y > b_y, "b should render on a lower line than a");
        assert!(b_y > c_y, "c should render on a lower line than b");
    }

    #[test]
    fn markdown_pdf_code_blocks_have_background_and_border_rect() {
        let source = "```go\nx := 1\n```";
        let bytes = build_markdown_pdf(source, &PdfExportPalette::default(), |_| None)
            .expect("pdf generation should succeed");
        let text = String::from_utf8_lossy(&bytes);
        assert!(text.contains(" re f"), "code block fill rect missing");
        assert!(text.contains(" re S"), "code block border rect missing");
    }

    #[test]
    fn markdown_pdf_table_uses_evaluated_formula_values() {
        let source = "| v |\n| --- |\n| 7 |\n| 11 |\n| :=sum_col() |";
        let lines: Vec<String> = source.lines().map(|line| line.to_string()).collect();
        let values = collect_table_formula_display_values(&lines);
        assert!(
            values.values().any(|value| value.starts_with("18")),
            "expected at least one evaluated table formula value starting with 18, got {values:?}"
        );

        let bytes = build_markdown_pdf(source, &PdfExportPalette::default(), |_| None)
            .expect("pdf generation should succeed");
        let text = String::from_utf8_lossy(&bytes);
        assert!(!pdf_contains_text(&text, "sum_col()"));
    }

    #[test]
    fn markdown_pdf_table_does_not_double_stroke_shared_horizontal_borders() {
        let source = "| a | b |\n| --- | --- |\n| 1 | 2 |\n| 3 | 4 |";
        let pages =
            render_markdown_to_pages(source, &PdfExportPalette::default(), &FxHashMap::default(), &[]);
        let page = pages.first().expect("first page");
        let table_left = PDF_MARGIN_LEFT_PT;
        let table_right = PDF_MARGIN_LEFT_PT + content_width();
        let mut horizontal_table_lines = 0usize;

        for op in &page.ops {
            if let DrawOp::Line {
                width,
                x1,
                y1,
                x2,
                y2,
                ..
            } = op
            {
                let is_horizontal = (y1 - y2).abs() < 0.01;
                let spans_full_table =
                    (x1 - table_left).abs() < 0.01 && (x2 - table_right).abs() < 0.01;
                let is_table_border_width = (*width - TABLE_BORDER_WIDTH_PT).abs() < 0.01;
                if is_horizontal && spans_full_table && is_table_border_width {
                    horizontal_table_lines += 1;
                }
            }
        }

        // header+2 data rows => 3 row bands, which should produce exactly
        // 4 horizontal borders (top + 3 bottoms), not 6 with doubled middle lines.
        assert_eq!(horizontal_table_lines, 4);
    }

    fn pdf_contains_text(pdf_text: &str, text: &str) -> bool {
        let encoded = encode_pdf_text_bytes(text);
        if encoded.is_empty() {
            return false;
        }
        let marker = format!("<{}> Tj", encode_pdf_hex_string(&encoded));
        pdf_text.contains(&marker)
    }
}
