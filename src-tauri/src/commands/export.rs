use app_core::AppCore;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use base64::Engine as _;
use editor_core::table::{is_table_line, split_table_cells_for_logical_row};
use flate2::{write::ZlibEncoder, Compression};
use image::GenericImageView as _;
use regex::Regex;
use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::io::Write;
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

#[derive(Clone, Copy, Debug)]
enum FontFace {
    Body,
    Bold,
    Mono,
}

impl FontFace {
    fn resource_name(self) -> &'static str {
        match self {
            Self::Body => "F1",
            Self::Bold => "F2",
            Self::Mono => "F3",
        }
    }

    fn width_factor(self) -> f32 {
        match self {
            Self::Body => 0.53,
            Self::Bold => 0.56,
            Self::Mono => 0.60,
        }
    }
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
        text: String,
    },
    Line {
        width: f32,
        x1: f32,
        y1: f32,
        x2: f32,
        y2: f32,
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

#[tauri::command]
pub fn export_to_file(path: String, content: String) -> Result<(), String> {
    fs::write(&path, &content).map_err(|e| format!("Failed to write file: {e}"))
}

#[tauri::command]
pub fn export_to_pdf(
    core: State<'_, AppCore>,
    note_id: String,
    path: String,
    content: String,
) -> Result<(), String> {
    let pdf_bytes = build_markdown_pdf(&content, |src| {
        resolve_markdown_image_bytes(&core, &note_id, src)
            .ok()
            .flatten()
    })?;
    fs::write(&path, pdf_bytes).map_err(|e| format!("Failed to write PDF: {e}"))
}

fn resolve_markdown_image_bytes(
    core: &State<'_, AppCore>,
    note_id: &str,
    src: &str,
) -> Result<Option<Vec<u8>>, String> {
    let resolved = core
        .note_sources()
        .resolve_image_markdown_source_by_id(note_id, src)?;
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

    let pages = render_markdown_to_pages(content, &image_name_by_src, &image_assets);
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
    image_name_by_src: &HashMap<String, String>,
    image_assets: &[(String, PdfImageObject)],
) -> Vec<Page> {
    let mut pages = vec![Page::default()];
    let mut y_top = PDF_MARGIN_TOP_PT;
    let max_top = PDF_PAGE_HEIGHT_PT - PDF_MARGIN_BOTTOM_PT;

    let normalized = content.replace("\r\n", "\n").replace('\r', "\n");
    let lines: Vec<&str> = normalized.split('\n').collect();
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
                render_wrapped_text(
                    &mut pages,
                    &mut y_top,
                    max_top,
                    &[format!("[image unavailable: {}]", alt_if_empty(&alt))],
                    FontFace::Mono,
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
            let (size, font) = heading_style(level);
            let wrapped = wrap_text(
                &strip_inline_markdown(heading_text.as_str()),
                content_width(),
                size,
                font,
            );
            render_wrapped_text(
                &mut pages,
                &mut y_top,
                max_top,
                &wrapped,
                font,
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
            let (block_lines, next) = collect_code_fence(&lines, i);
            render_code_block(&mut pages, &mut y_top, max_top, &block_lines);
            i = next;
            continue;
        }

        if is_markdown_table_start(&lines, i) {
            let (table_lines, next) = collect_table_block(&lines, i);
            render_table_block(&mut pages, &mut y_top, max_top, &table_lines);
            i = next;
            continue;
        }

        if is_unordered_list_item(line) || is_ordered_list_item(line) {
            let (items, ordered, next) = collect_list_block(&lines, i);
            render_list_block(&mut pages, &mut y_top, max_top, &items, ordered);
            i = next;
            continue;
        }

        if trimmed.starts_with('>') {
            let (quote_lines, next) = collect_blockquote(&lines, i);
            render_blockquote_block(&mut pages, &mut y_top, max_top, &quote_lines);
            i = next;
            continue;
        }

        let (paragraph, next) = collect_paragraph(&lines, i);
        render_paragraph_block(&mut pages, &mut y_top, max_top, paragraph.as_str());
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

fn render_wrapped_text(
    pages: &mut Vec<Page>,
    y_top: &mut f32,
    max_top: f32,
    lines: &[String],
    font: FontFace,
    size: f32,
    line_height: f32,
    x: f32,
    max_width: f32,
) {
    for source in lines {
        let wrapped = wrap_text(source, max_width, size, font);
        for line in wrapped {
            ensure_space(pages, y_top, line_height, max_top);
            let y_pdf = PDF_PAGE_HEIGHT_PT - *y_top - size;
            current_page_mut(pages).ops.push(DrawOp::Text {
                font,
                size,
                x,
                y: y_pdf,
                text: line,
            });
            *y_top += line_height;
        }
    }
}

fn render_paragraph_block(pages: &mut Vec<Page>, y_top: &mut f32, max_top: f32, text: &str) {
    let normalized = strip_inline_markdown(text);
    let wrapped = wrap_text(
        normalized.as_str(),
        content_width(),
        BODY_FONT_SIZE_PT,
        FontFace::Body,
    );
    render_wrapped_text(
        pages,
        y_top,
        max_top,
        &wrapped,
        FontFace::Body,
        BODY_FONT_SIZE_PT,
        BODY_LINE_HEIGHT_PT,
        PDF_MARGIN_LEFT_PT,
        content_width(),
    );
    *y_top += PARAGRAPH_GAP_PT;
}

fn render_code_block(pages: &mut Vec<Page>, y_top: &mut f32, max_top: f32, code_lines: &[String]) {
    let block_padding = 6.0;
    let code_width = content_width();

    for line in code_lines {
        let wrapped = wrap_text(
            line,
            code_width - (block_padding * 2.0),
            BODY_FONT_SIZE_PT,
            FontFace::Mono,
        );
        for wrapped_line in wrapped {
            ensure_space(pages, y_top, BODY_LINE_HEIGHT_PT, max_top);
            let y_pdf = PDF_PAGE_HEIGHT_PT - *y_top - BODY_FONT_SIZE_PT;
            current_page_mut(pages).ops.push(DrawOp::Text {
                font: FontFace::Mono,
                size: BODY_FONT_SIZE_PT,
                x: PDF_MARGIN_LEFT_PT + block_padding,
                y: y_pdf,
                text: wrapped_line,
            });
            *y_top += BODY_LINE_HEIGHT_PT;
        }
    }

    *y_top += PARAGRAPH_GAP_PT;
}

fn render_list_block(
    pages: &mut Vec<Page>,
    y_top: &mut f32,
    max_top: f32,
    items: &[String],
    ordered: bool,
) {
    for (idx, item) in items.iter().enumerate() {
        let bullet = if ordered {
            format!("{}. ", idx + 1)
        } else {
            "• ".to_string()
        };
        let clean = strip_inline_markdown(item);
        let body_width = content_width() - 18.0;
        let wrapped = wrap_text(
            clean.as_str(),
            body_width,
            BODY_FONT_SIZE_PT,
            FontFace::Body,
        );
        if wrapped.is_empty() {
            continue;
        }

        ensure_space(pages, y_top, BODY_LINE_HEIGHT_PT, max_top);
        let first_y = PDF_PAGE_HEIGHT_PT - *y_top - BODY_FONT_SIZE_PT;
        current_page_mut(pages).ops.push(DrawOp::Text {
            font: FontFace::Body,
            size: BODY_FONT_SIZE_PT,
            x: PDF_MARGIN_LEFT_PT,
            y: first_y,
            text: bullet,
        });

        for (line_idx, wrapped_line) in wrapped.iter().enumerate() {
            if line_idx > 0 {
                ensure_space(pages, y_top, BODY_LINE_HEIGHT_PT, max_top);
            }
            let y_pdf = PDF_PAGE_HEIGHT_PT - *y_top - BODY_FONT_SIZE_PT;
            current_page_mut(pages).ops.push(DrawOp::Text {
                font: FontFace::Body,
                size: BODY_FONT_SIZE_PT,
                x: PDF_MARGIN_LEFT_PT + 18.0,
                y: y_pdf,
                text: wrapped_line.clone(),
            });
            *y_top += BODY_LINE_HEIGHT_PT;
        }
    }
    *y_top += PARAGRAPH_GAP_PT;
}

fn render_blockquote_block(pages: &mut Vec<Page>, y_top: &mut f32, max_top: f32, lines: &[String]) {
    let quote_x = PDF_MARGIN_LEFT_PT + 10.0;
    let text_x = PDF_MARGIN_LEFT_PT + 16.0;
    let wrapped = lines
        .iter()
        .flat_map(|line| {
            wrap_text(
                strip_inline_markdown(line).as_str(),
                content_width() - 16.0,
                BODY_FONT_SIZE_PT,
                FontFace::Body,
            )
        })
        .collect::<Vec<_>>();

    if wrapped.is_empty() {
        return;
    }

    let block_height = (wrapped.len() as f32) * BODY_LINE_HEIGHT_PT;
    ensure_space(pages, y_top, block_height + PARAGRAPH_GAP_PT, max_top);

    let y1 = PDF_PAGE_HEIGHT_PT - *y_top;
    let y2 = PDF_PAGE_HEIGHT_PT - (*y_top + block_height);
    current_page_mut(pages).ops.push(DrawOp::Line {
        width: 1.2,
        x1: quote_x,
        y1,
        x2: quote_x,
        y2,
    });

    for line in wrapped {
        let y_pdf = PDF_PAGE_HEIGHT_PT - *y_top - BODY_FONT_SIZE_PT;
        current_page_mut(pages).ops.push(DrawOp::Text {
            font: FontFace::Body,
            size: BODY_FONT_SIZE_PT,
            x: text_x,
            y: y_pdf,
            text: line,
        });
        *y_top += BODY_LINE_HEIGHT_PT;
    }

    *y_top += PARAGRAPH_GAP_PT;
}

fn render_table_block(
    pages: &mut Vec<Page>,
    y_top: &mut f32,
    max_top: f32,
    table_lines: &[String],
) {
    if table_lines.len() < 2 {
        render_paragraph_block(pages, y_top, max_top, &table_lines.join(" "));
        return;
    }

    let mut rows: Vec<Vec<String>> = Vec::new();
    for (idx, line) in table_lines.iter().enumerate() {
        if idx == 1 && is_table_delimiter(line) {
            continue;
        }
        let cells = split_table_cells_for_logical_row(line)
            .into_iter()
            .map(|cell| strip_inline_markdown(cell.trim()))
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
        let mut cell_wrapped: Vec<Vec<String>> = Vec::with_capacity(col_count);
        let mut max_line_count = 1usize;
        for col in 0..col_count {
            let text = row.get(col).cloned().unwrap_or_default();
            let wrapped = wrap_text(
                text.as_str(),
                col_width - (TABLE_CELL_PAD_X_PT * 2.0),
                TABLE_FONT_SIZE_PT,
                if row_idx == 0 {
                    FontFace::Bold
                } else {
                    FontFace::Body
                },
            );
            max_line_count = max_line_count.max(wrapped.len().max(1));
            cell_wrapped.push(if wrapped.is_empty() {
                vec![String::new()]
            } else {
                wrapped
            });
        }

        let row_height =
            (max_line_count as f32) * TABLE_LINE_HEIGHT_PT + (TABLE_CELL_PAD_Y_PT * 2.0);
        ensure_space(pages, y_top, row_height + TABLE_BORDER_WIDTH_PT, max_top);

        let top_y_pdf = PDF_PAGE_HEIGHT_PT - *y_top;
        let bottom_y_pdf = PDF_PAGE_HEIGHT_PT - (*y_top + row_height);

        current_page_mut(pages).ops.push(DrawOp::Line {
            width: TABLE_BORDER_WIDTH_PT,
            x1: table_left,
            y1: top_y_pdf,
            x2: table_left + table_width,
            y2: top_y_pdf,
        });
        current_page_mut(pages).ops.push(DrawOp::Line {
            width: TABLE_BORDER_WIDTH_PT,
            x1: table_left,
            y1: bottom_y_pdf,
            x2: table_left + table_width,
            y2: bottom_y_pdf,
        });

        for col in 0..=col_count {
            let x = table_left + (col as f32) * col_width;
            current_page_mut(pages).ops.push(DrawOp::Line {
                width: TABLE_BORDER_WIDTH_PT,
                x1: x,
                y1: top_y_pdf,
                x2: x,
                y2: bottom_y_pdf,
            });
        }

        for col in 0..col_count {
            let text_lines = &cell_wrapped[col];
            for (line_idx, text_line) in text_lines.iter().enumerate() {
                let y_text = PDF_PAGE_HEIGHT_PT
                    - *y_top
                    - TABLE_CELL_PAD_Y_PT
                    - (line_idx as f32) * TABLE_LINE_HEIGHT_PT
                    - TABLE_FONT_SIZE_PT;
                current_page_mut(pages).ops.push(DrawOp::Text {
                    font: if row_idx == 0 {
                        FontFace::Bold
                    } else {
                        FontFace::Body
                    },
                    size: TABLE_FONT_SIZE_PT,
                    x: table_left + (col as f32) * col_width + TABLE_CELL_PAD_X_PT,
                    y: y_text,
                    text: text_line.clone(),
                });
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

fn heading_style(level: usize) -> (f32, FontFace) {
    match level {
        1 => (24.0, FontFace::Bold),
        2 => (20.0, FontFace::Bold),
        3 => (17.0, FontFace::Bold),
        4 => (15.0, FontFace::Bold),
        5 => (13.0, FontFace::Bold),
        _ => (12.0, FontFace::Bold),
    }
}

fn wrap_text(text: &str, max_width: f32, font_size: f32, font: FontFace) -> Vec<String> {
    let clean = text.trim();
    if clean.is_empty() {
        return vec![String::new()];
    }

    let char_width = font_size * font.width_factor();
    if char_width <= 0.0 || max_width <= 0.0 {
        return vec![clean.to_string()];
    }
    let max_chars = (max_width / char_width).floor().max(1.0) as usize;

    let mut out: Vec<String> = Vec::new();
    let mut line = String::new();

    for word in clean.split_whitespace() {
        if line.is_empty() {
            if word.chars().count() > max_chars {
                out.extend(split_long_token(word, max_chars));
            } else {
                line.push_str(word);
            }
            continue;
        }

        let projected = line.chars().count() + 1 + word.chars().count();
        if projected <= max_chars {
            line.push(' ');
            line.push_str(word);
            continue;
        }

        out.push(line);
        line = String::new();
        if word.chars().count() > max_chars {
            out.extend(split_long_token(word, max_chars));
        } else {
            line.push_str(word);
        }
    }

    if !line.is_empty() {
        out.push(line);
    }

    if out.is_empty() {
        out.push(String::new());
    }

    out
}

fn split_long_token(token: &str, chunk_len: usize) -> Vec<String> {
    if chunk_len == 0 {
        return vec![token.to_string()];
    }
    let chars = token.chars().collect::<Vec<_>>();
    let mut out = Vec::new();
    let mut start = 0usize;
    while start < chars.len() {
        let end = (start + chunk_len).min(chars.len());
        out.push(chars[start..end].iter().collect());
        start = end;
    }
    out
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

fn collect_code_fence(lines: &[&str], start: usize) -> (Vec<String>, usize) {
    let mut out = Vec::new();
    let mut i = start + 1;
    while i < lines.len() {
        let line = lines[i];
        if line.trim_start().starts_with("```") {
            return (out, i + 1);
        }
        out.push(line.to_string());
        i += 1;
    }
    (out, i)
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

fn strip_inline_markdown(text: &str) -> String {
    static IMAGE_RE: OnceLock<Regex> = OnceLock::new();
    static LINK_RE: OnceLock<Regex> = OnceLock::new();

    let image_re = IMAGE_RE
        .get_or_init(|| Regex::new(r"!\[([^\]]*)\]\(([^)]+)\)").expect("inline image regex"));
    let link_re =
        LINK_RE.get_or_init(|| Regex::new(r"\[([^\]]+)\]\(([^)]+)\)").expect("inline link regex"));

    let no_images = image_re.replace_all(text, "[image: $1]").to_string();
    let linked = link_re.replace_all(&no_images, "$1 ($2)").to_string();
    linked
        .replace("**", "")
        .replace("__", "")
        .replace("~~", "")
        .replace('`', "")
        .replace('*', "")
        .replace('_', "")
        .trim()
        .to_string()
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
    let image_first_id = 6usize;
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
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {PDF_PAGE_WIDTH_PT:.0} {PDF_PAGE_HEIGHT_PT:.0}] /Resources << /Font << /F1 3 0 R /F2 4 0 R /F3 5 0 R >>{xobject_section} >> /Contents {content_obj_id} 0 R >>\n"
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
                text,
            } => {
                let escaped = escape_pdf_text(text);
                out.push_str("BT\n");
                out.push_str(&format!("/{} {:.2} Tf\n", font.resource_name(), size));
                out.push_str(&format!("1 0 0 1 {:.2} {:.2} Tm ({escaped}) Tj\n", x, y));
                out.push_str("ET\n");
            }
            DrawOp::Line {
                width,
                x1,
                y1,
                x2,
                y2,
            } => {
                out.push_str(&format!(
                    "{width:.2} w\n{x1:.2} {y1:.2} m\n{x2:.2} {y2:.2} l\nS\n"
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
        let bytes = build_markdown_pdf(source, |_| None).expect("pdf generation should succeed");
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
        let bytes = build_markdown_pdf(source, |src| {
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
    fn strip_inline_markdown_keeps_link_text_and_removes_markers() {
        let clean = strip_inline_markdown("**Bold** and [Link](https://example.com) and `code`");
        assert_eq!(clean, "Bold and Link (https://example.com) and code");
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
}
