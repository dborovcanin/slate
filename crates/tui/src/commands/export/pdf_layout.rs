use super::pdf_style::{
    normalize_for_paper, paper_body_color, paper_border_color, paper_code_bg_color,
    paper_code_border_color, paper_heading_color, paper_unchecked_checkbox_color, FontFace,
    PdfExportPalette, PdfRgbColor, StyledChar, TextStyle, HELVETICA_BOLD_CHAR_WIDTHS,
    HELVETICA_CHAR_WIDTHS,
};
use app_core::calc::{CalcEngine, NoteEvaluationOptions};
use editor_core::calc_plan;
use editor_core::markdown_tokens::{self, CodeTokenType, InlineTokenType};
use editor_core::table::{is_table_line, split_table_cells_for_logical_row};
use regex::Regex;
use rustc_hash::FxHashMap;
use std::collections::BTreeSet;
use std::sync::OnceLock;

pub(super) const PDF_PAGE_WIDTH_PT: f32 = 595.0;
pub(super) const PDF_PAGE_HEIGHT_PT: f32 = 842.0;
pub(super) const PDF_MARGIN_LEFT_PT: f32 = 40.0;
pub(super) const PDF_MARGIN_RIGHT_PT: f32 = 40.0;
pub(super) const PDF_MARGIN_TOP_PT: f32 = 42.0;
pub(super) const PDF_MARGIN_BOTTOM_PT: f32 = 42.0;

pub(super) const BODY_FONT_SIZE_PT: f32 = 11.0;
pub(super) const BODY_LINE_HEIGHT_PT: f32 = 14.0;
pub(super) const HEADING_LINE_GAP_PT: f32 = 6.0;
pub(super) const PARAGRAPH_GAP_PT: f32 = 5.0;

pub(super) const TABLE_FONT_SIZE_PT: f32 = 10.0;
pub(super) const TABLE_LINE_HEIGHT_PT: f32 = 12.0;
pub(super) const TABLE_CELL_PAD_X_PT: f32 = 4.0;
pub(super) const TABLE_CELL_PAD_Y_PT: f32 = 4.0;
pub(super) const TABLE_BORDER_WIDTH_PT: f32 = 0.7;
pub(super) const CODE_BLOCK_BORDER_WIDTH_PT: f32 = 0.7;
pub(super) const CODE_BLOCK_PAD_X_PT: f32 = 6.0;
pub(super) const CODE_BLOCK_PAD_Y_PT: f32 = 6.0;

pub(super) const IMAGE_MAX_HEIGHT_PT: f32 = 280.0;
pub(super) const CHECKBOX_SIZE_PT: f32 = 9.0;

const PDF_TAB_WIDTH: usize = 4;

#[derive(Debug, Clone)]
pub(super) struct PdfImageObject {
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) compressed_rgb: Vec<u8>,
}

#[derive(Debug, Clone)]
pub(super) enum DrawOp {
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
pub(super) struct Page {
    pub(super) ops: Vec<DrawOp>,
    pub(super) used_images: BTreeSet<String>,
}

pub(super) fn render_markdown_to_pages(
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

        if let Some((level, heading_text)) = parse_heading(line) {
            let size = heading_font_size(level);
            let line_height = size + HEADING_LINE_GAP_PT;
            let heading_color = paper_heading_color(palette);
            let chars = styled_chars_from_inline(
                &heading_text,
                &variable_names,
                palette,
                TextStyle::heading(heading_color),
            );
            render_styled_block(
                &mut pages,
                &mut y_top,
                max_top,
                &chars,
                size,
                line_height,
                PDF_MARGIN_LEFT_PT,
                content_width(),
            );
            y_top += PARAGRAPH_GAP_PT;
            i += 1;
            continue;
        }

        if is_fence_start(trimmed) {
            let (code_lines, code_lang, next_i) = collect_code_fence(&lines, i);
            render_code_block(
                &mut pages,
                &mut y_top,
                max_top,
                &code_lines,
                code_lang.as_deref(),
                palette,
            );
            i = next_i;
            continue;
        }

        if is_markdown_table_start(&lines, i) {
            let (table_lines, next_i) = collect_table_block(&lines, i);
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
            i = next_i;
            continue;
        }

        if is_unordered_list_item(line) || is_ordered_list_item(line) {
            let (items, ordered, next_i) = collect_list_block(&lines, i);
            render_list_block(
                &mut pages,
                &mut y_top,
                max_top,
                &items,
                ordered,
                palette,
                &variable_names,
            );
            i = next_i;
            continue;
        }

        if trimmed.starts_with('>') {
            let (quote_lines, next_i) = collect_blockquote(&lines, i);
            render_blockquote_block(
                &mut pages,
                &mut y_top,
                max_top,
                &quote_lines,
                palette,
                &variable_names,
            );
            i = next_i;
            continue;
        }

        if let Some((alt, src)) = parse_image_only_line(trimmed) {
            if let Some(image_name) = image_name_by_src.get(&src) {
                if let Some((_, image_obj)) =
                    image_assets.iter().find(|(name, _)| name == image_name)
                {
                    render_image(&mut pages, &mut y_top, max_top, image_name, image_obj);
                } else {
                    render_paragraph_block(
                        &mut pages,
                        &mut y_top,
                        max_top,
                        &alt_if_empty(&alt),
                        palette,
                        &variable_names,
                    );
                }
            } else {
                render_paragraph_block(
                    &mut pages,
                    &mut y_top,
                    max_top,
                    &alt_if_empty(&alt),
                    palette,
                    &variable_names,
                );
            }
            i += 1;
            continue;
        }

        let (para_lines, next_i) = collect_paragraph(&lines, i);
        if para_lines.is_empty() {
            i += 1;
            continue;
        }
        if should_preserve_hard_linebreaks(&para_lines) {
            render_hard_line_paragraph_block(
                &mut pages,
                &mut y_top,
                max_top,
                &para_lines,
                palette,
                &variable_names,
            );
        } else {
            render_paragraph_block(
                &mut pages,
                &mut y_top,
                max_top,
                &para_lines.join(" "),
                palette,
                &variable_names,
            );
        }
        i = next_i;
    }

    pages
}

pub(super) fn content_width() -> f32 {
    PDF_PAGE_WIDTH_PT - PDF_MARGIN_LEFT_PT - PDF_MARGIN_RIGHT_PT
}

fn ensure_space(pages: &mut Vec<Page>, y_top: &mut f32, needed_height: f32, max_top: f32) {
    if *y_top + needed_height > max_top {
        pages.push(Page::default());
        *y_top = PDF_MARGIN_TOP_PT;
    }
}

fn current_page_mut(pages: &mut Vec<Page>) -> &mut Page {
    if pages.is_empty() {
        pages.push(Page::default());
    }
    pages.last_mut().expect("pages is non-empty")
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

pub(super) fn styled_chars_from_inline(
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

pub(super) fn styled_chars_for_code_line(
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
    // Courier is fixed-pitch; existing factor is already correct for all chars.
    if matches!(font, FontFace::Mono) {
        return if ch == ' ' {
            size * font.width_factor() * font.space_factor()
        } else {
            size * font.width_factor()
        };
    }
    let cp = ch as u32;
    // ASCII printable: use standard Helvetica/Helvetica-Bold AFM metrics.
    if cp >= 32 && cp <= 126 {
        let idx = (cp - 32) as usize;
        let w = if matches!(font, FontFace::Bold | FontFace::BoldItalic) {
            HELVETICA_BOLD_CHAR_WIDTHS[idx]
        } else {
            HELVETICA_CHAR_WIDTHS[idx]
        };
        return size * w as f32 / 1000.0;
    }
    // Non-ASCII: look up actual advance width from the unicode fallback font.
    if let Some(asset) = super::resolve_unicode_pdf_font_asset() {
        if let Some(adv) = asset.glyph_advance_for_char(ch) {
            return size * adv;
        }
    }
    // Last resort: existing heuristic for chars not covered above.
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
    while line.last().is_some_and(|item| item.ch.is_whitespace()) {
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

pub(super) fn render_styled_line(
    pages: &mut Vec<Page>,
    x: f32,
    y: f32,
    size: f32,
    line: &[StyledChar],
) {
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

pub(super) fn collect_table_formula_display_values(
    lines: &[String],
) -> FxHashMap<(usize, usize), String> {
    let options = NoteEvaluationOptions {
        variables_enabled: true,
        table_enabled: true,
        eval_range: None,
        ..Default::default()
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

/// Delimiter check for the line right after a table's header row.
fn is_table_delimiter(line: &str) -> bool {
    is_table_line(line)
        && editor_core::table::is_delimiter_row_at(&editor_core::table::split_table_cells(line), true)
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

pub(super) fn should_preserve_hard_linebreaks(lines: &[String]) -> bool {
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

pub(super) fn parse_image_only_line(line: &str) -> Option<(String, String)> {
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

pub(super) fn collect_image_sources(content: &str) -> Vec<String> {
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
