use app_core::AppCore;
use app_core::note_sources::NoteSourceService;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use base64::Engine as _;
use editor_core::calc_plan;
use editor_core::markdown_tokens::{self, CodeTokenType, InlineTokenType};
use editor_core::table::{is_table_line, split_table_cells_for_logical_row};
use flate2::{write::ZlibEncoder, Compression};
use image::GenericImageView as _;
use regex::Regex;
use serde::Deserialize;
use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use tauri::State;

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

const IMAGE_MAX_HEIGHT_PT: f32 = 280.0;
const CHECKBOX_SIZE_PT: f32 = 9.0;

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

fn pdf_black() -> PdfRgbColor {
    PdfRgbColor { r: 0, g: 0, b: 0 }
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
}

#[derive(Debug, Clone, Default)]
struct Page {
    ops: Vec<DrawOp>,
    used_images: BTreeSet<String>,
}

fn resolve_export_path(path: &str) -> Result<PathBuf, String> {
    let raw = path.trim();
    if raw.is_empty() {
        return Err("export path is empty".to_string());
    }

    let resolved = if raw == "~" || raw.starts_with("~/") {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .ok_or_else(|| "HOME is not set; cannot expand '~' in export path".to_string())?;
        if raw == "~" {
            home
        } else {
            let rest = raw.trim_start_matches("~/");
            home.join(rest)
        }
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

#[tauri::command]
pub fn export_to_file(path: String, content: String) -> Result<(), String> {
    let resolved = resolve_export_path(&path)?;
    fs::write(&resolved, &content)
        .map_err(|e| format!("Failed to write file '{}': {e}", resolved.display()))
}

#[tauri::command]
pub fn export_to_pdf(
    core: State<'_, AppCore>,
    note_id: String,
    path: String,
    content: String,
    palette: PdfExportPalette,
) -> Result<(), String> {
    export_markdown_to_pdf_file(
        &core.note_sources(),
        &note_id,
        &path,
        &content,
        &palette,
    )
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
        return decode_data_url_image_bytes(&value).map(Some);
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
    let mut image_name_by_src: HashMap<String, String> = HashMap::new();
    let mut image_assets: Vec<(String, PdfImageObject)> = Vec::new();

    for src in image_sources {
        let Some(image_bytes) = resolve_image(src.as_str()) else {
            continue;
        };
        let Ok(image_obj) = decode_pdf_image(&image_bytes) else {
            continue;
        };
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
    image_name_by_src: &HashMap<String, String>,
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
                    TextStyle::mono(pdf_black()),
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
            let heading_style = TextStyle::heading(pdf_black());
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
                palette,
                &variable_names,
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

        let (paragraph, next) = collect_paragraph(&lines, i);
        render_paragraph_block(
            &mut pages,
            &mut y_top,
            max_top,
            paragraph.as_str(),
            palette,
            &variable_names,
        );
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
            if &bytes[idx..end] == needle && has_variable_word_boundaries(bytes, idx, end) {
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
    let chars: Vec<char> = text.chars().collect();
    let mut styles = vec![TextStyle::mono(pdf_black()); chars.len()];
    let tokens = markdown_tokens::tokenize_code_line(text, lang);

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
        size * font.width_factor() * 0.8
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
                let strike_y = y + (size * 0.35);
                push_line_op(
                    page,
                    0.7,
                    *cursor_x,
                    strike_y,
                    *cursor_x + run_width,
                    strike_y,
                    run_style.color,
                );
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
    let chars =
        styled_chars_from_inline(text, variable_names, palette, TextStyle::body(pdf_black()));
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

fn render_code_block(
    pages: &mut Vec<Page>,
    y_top: &mut f32,
    max_top: f32,
    code_lines: &[String],
    code_lang: Option<&str>,
    palette: &PdfExportPalette,
) {
    let block_padding = 6.0;
    let code_width = content_width() - (block_padding * 2.0);

    for line in code_lines {
        let chars = styled_chars_for_code_line(line, code_lang, palette);
        render_styled_block(
            pages,
            y_top,
            max_top,
            &chars,
            BODY_FONT_SIZE_PT,
            BODY_LINE_HEIGHT_PT,
            PDF_MARGIN_LEFT_PT + block_padding,
            code_width,
        );
    }

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
    let color = if checked { palette.accent } else { pdf_black() };

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

        let mut base = TextStyle::body(pdf_black());
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
                        "• ".to_string()
                    };
                    push_text_op(
                        current_page_mut(pages),
                        TextStyle::body(pdf_black()),
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
        let mut base = TextStyle::body(pdf_black());
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
        pdf_black(),
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
    palette: &PdfExportPalette,
    variable_names: &[String],
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

    let mut rows: Vec<Vec<String>> = Vec::new();
    for (idx, line) in table_lines.iter().enumerate() {
        if idx == 1 && is_table_delimiter(line) {
            continue;
        }
        let cells = split_table_cells_for_logical_row(line)
            .into_iter()
            .map(|cell| cell.trim().to_string())
            .collect::<Vec<_>>();
        rows.push(cells);
    }

    let col_count = rows.iter().map(|row| row.len()).max().unwrap_or(0);
    if col_count == 0 {
        return;
    }

    let table_width = content_width();
    let col_width = table_width / (col_count as f32);
    let table_left = PDF_MARGIN_LEFT_PT;

    for (row_idx, row) in rows.iter().enumerate() {
        let mut cell_wrapped: Vec<Vec<Vec<StyledChar>>> = Vec::with_capacity(col_count);
        let mut max_line_count = 1usize;
        for col in 0..col_count {
            let text = row.get(col).cloned().unwrap_or_default();
            let mut base = TextStyle::body(pdf_black());
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
        ensure_space(pages, y_top, row_height + TABLE_BORDER_WIDTH_PT, max_top);

        let top_y_pdf = PDF_PAGE_HEIGHT_PT - *y_top;
        let bottom_y_pdf = PDF_PAGE_HEIGHT_PT - (*y_top + row_height);

        let border = pdf_black();
        push_line_op(
            current_page_mut(pages),
            TABLE_BORDER_WIDTH_PT,
            table_left,
            top_y_pdf,
            table_left + table_width,
            top_y_pdf,
            border,
        );
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

fn collect_paragraph(lines: &[&str], start: usize) -> (String, usize) {
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
    (out.join(" "), i)
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
    let page_count = pages.len().max(1);

    let image_count = image_assets.len();
    let image_first_id = 8usize;
    let page_first_id = image_first_id + image_count;

    let mut image_obj_id_by_name: HashMap<String, usize> = HashMap::new();
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

    let actual_pages = if pages.is_empty() {
        vec![Page::default()]
    } else {
        pages
    };

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

        objects.push(
            format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {PDF_PAGE_WIDTH_PT:.0} {PDF_PAGE_HEIGHT_PT:.0}] /Resources << /Font << /F1 3 0 R /F2 4 0 R /F3 5 0 R /F4 6 0 R /F5 7 0 R >>{xobject_section} >> /Contents {content_obj_id} 0 R >>\n"
            )
            .into_bytes(),
        );

        let stream = page_stream(page);
        let mut content_obj = format!("<< /Length {} >>\nstream\n", stream.len()).into_bytes();
        content_obj.extend_from_slice(stream.as_bytes());
        content_obj.extend_from_slice(b"\nendstream\n");
        objects.push(content_obj);
    }

    serialize_objects(objects)
}

fn page_stream(page: &Page) -> String {
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
                let escaped = escape_pdf_text(text);
                out.push_str("BT\n");
                out.push_str(&format!("/{} {:.2} Tf\n", font.resource_name(), size));
                out.push_str(&format!("{r:.4} {g:.4} {b:.4} rg\n"));
                out.push_str(&format!("1 0 0 1 {:.2} {:.2} Tm ({escaped}) Tj\n", x, y));
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

fn escape_pdf_text(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '(' => out.push_str("\\("),
            ')' => out.push_str("\\)"),
            _ if ch.is_ascii() && !ch.is_ascii_control() => out.push(ch),
            _ => out.push('?'),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::codecs::png::PngEncoder;
    use image::{ColorType, ImageEncoder, Rgb, RgbImage};
    use std::path::PathBuf;
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
        assert!(text.contains("(Title) Tj"));
        assert!(text.contains("(Name) Tj"));
        assert!(text.contains("(Score) Tj"));
        assert!(text.contains("(item one) Tj"));
        assert!(text.contains("xref"));
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
        let home = std::env::var_os("HOME").expect("HOME should be set");
        assert!(resolved.starts_with(home));
        assert!(resolved.to_string_lossy().contains("slate-export-"));
    }

    #[test]
    fn export_to_file_supports_tilde_paths() {
        let relative = format!("slate-export-{}.txt", unique_suffix());
        let raw = format!("~/{}", relative);
        let home = std::env::var_os("HOME").expect("HOME should be set");
        let full_path = PathBuf::from(home).join(relative);
        match export_to_file(raw, "hello".to_string()) {
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

        assert!(text.contains("(Done) Tj"));
        assert!(text.contains("/F2"));
        assert!(text.contains("/F4"));
        assert!(text.contains("1.0000 0.0000 0.0000 rg"));
        assert!(text.contains("0.0000 1.0000 0.0000 rg"));
        assert!(!text.contains("([x]) Tj"));
    }
}
