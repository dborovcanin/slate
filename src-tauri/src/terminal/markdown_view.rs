use crate::editor_core::markdown_tokens::{self, InlineToken, InlineTokenType, MarkdownLineInfo};

pub fn hidden_line_prefix_marker_ranges(
    info: &MarkdownLineInfo,
    len: usize,
    reveal_prefix: bool,
) -> Vec<(usize, usize)> {
    let mut ranges = Vec::new();
    if reveal_prefix {
        return ranges;
    }
    if let Some(marker_end) = info.heading_marker_end {
        let to = marker_end.min(len);
        if to > 0 {
            ranges.push((0, to));
        }
    }
    if let Some(marker_end) = info.quote_marker_end {
        let to = marker_end.min(len);
        if to > 0 {
            ranges.push((0, to));
        }
    }
    ranges
}

pub fn normalize_hidden_ranges(mut ranges: Vec<(usize, usize)>, len: usize) -> Vec<(usize, usize)> {
    if ranges.is_empty() || len == 0 {
        return Vec::new();
    }
    for range in &mut ranges {
        range.0 = range.0.min(len);
        range.1 = range.1.min(len);
    }
    ranges.retain(|(from, to)| to > from);
    if ranges.is_empty() {
        return Vec::new();
    }
    ranges.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
    let mut merged: Vec<(usize, usize)> = Vec::with_capacity(ranges.len());
    for (from, to) in ranges {
        if let Some(last) = merged.last_mut() {
            if from <= last.1 {
                if to > last.1 {
                    last.1 = to;
                }
                continue;
            }
        }
        merged.push((from, to));
    }
    merged
}

pub fn should_reveal_inline_marker(
    tokens: &[markdown_tokens::InlineToken],
    component_ranges: &[markdown_tokens::InlineMarkerComponentRange],
    marker_index: usize,
    active_cursor_col: Option<usize>,
) -> bool {
    let Some(cursor_col) = active_cursor_col else {
        return false;
    };
    let Some(marker) = tokens.get(marker_index) else {
        return false;
    };
    if !markdown_tokens::is_inline_marker_token_kind(marker.kind) {
        return false;
    }
    let Some((from, to)) = component_ranges
        .iter()
        .find(|range| marker.from >= range.from && marker.to <= range.to)
        .map(|range| (range.from, range.to))
    else {
        return false;
    };
    if cursor_col >= from && cursor_col < to {
        return true;
    }
    let has_formatting_body = tokens.iter().any(|token| {
        token.from >= from
            && token.to <= to
            && matches!(
                token.kind,
                InlineTokenType::Strong
                    | InlineTokenType::Emphasis
                    | InlineTokenType::Strikethrough
            )
    });
    cursor_col == to
        && (has_formatting_body
            || matches!(
                marker.kind,
                InlineTokenType::LinkMarker
                    | InlineTokenType::WikiLinkMarker
                    | InlineTokenType::ImageMarker
            ))
}

fn inline_tokens_for_display(text: &str) -> Vec<InlineToken> {
    let mut inline_tokens = markdown_tokens::tokenize_inline_markdown(text);
    // Markdown emphasis (`*x*`, `***x***`) inside table rows is ambiguous with
    // formula markers (`value*`, `value***`) and can bleed styling across cells.
    // Keep code, links, and strikethrough for table rows.
    if text.trim_start().starts_with('|') {
        inline_tokens.retain(|token| {
            !matches!(
                token.kind,
                InlineTokenType::Strong | InlineTokenType::Emphasis
            )
        });
    }
    inline_tokens
}

fn hidden_inline_marker_ranges(
    tokens: &[InlineToken],
    len: usize,
    active_cursor_col: Option<usize>,
    force_formatting_right_boundary_exit: bool,
) -> Vec<(usize, usize)> {
    let component_ranges = markdown_tokens::inline_marker_component_ranges_from_tokens(tokens);
    let mut hidden_ranges = Vec::new();
    for (index, token) in tokens.iter().enumerate() {
        let from = token.from.min(len);
        let to = token.to.min(len);
        if to <= from {
            continue;
        }
        if !markdown_tokens::is_inline_marker_token_kind(token.kind) {
            continue;
        }
        let component_range = component_ranges
            .iter()
            .find(|range| token.from >= range.from && token.to <= range.to);
        if force_formatting_right_boundary_exit
            && active_cursor_col.is_some_and(|cursor_col| {
                component_range.is_some_and(|range| {
                    cursor_col == range.to
                        && component_has_formatting_body(tokens, range.from, range.to)
                })
            })
        {
            hidden_ranges.push((from, to));
        } else if !should_reveal_inline_marker(tokens, &component_ranges, index, active_cursor_col)
        {
            hidden_ranges.push((from, to));
        }
    }
    hidden_ranges.extend(
        image_hidden_token_ranges(tokens, active_cursor_col)
            .into_iter()
            .map(|(from, to)| (from.min(len), to.min(len)))
            .filter(|(from, to)| to > from),
    );
    hidden_ranges.extend(
        wiki_link_hidden_token_ranges(tokens, active_cursor_col)
            .into_iter()
            .map(|(from, to)| (from.min(len), to.min(len)))
            .filter(|(from, to)| to > from),
    );
    hidden_ranges
}

pub fn wiki_link_hidden_token_ranges(
    tokens: &[InlineToken],
    active_cursor_col: Option<usize>,
) -> Vec<(usize, usize)> {
    let mut hidden = Vec::new();
    let mut open_idx: Option<usize> = None;
    let mut has_title = false;

    for (index, token) in tokens.iter().enumerate() {
        match token.kind {
            InlineTokenType::WikiLinkMarker => {
                if open_idx.is_none() {
                    open_idx = Some(index);
                    has_title = false;
                    continue;
                }

                let Some(start_idx) = open_idx.take() else {
                    continue;
                };
                let start = tokens[start_idx].from;
                let end = token.to;
                let cursor_inside = active_cursor_col
                    .map(|col| col >= start && col <= end)
                    .unwrap_or(false);
                if has_title && !cursor_inside {
                    for wiki_token in &tokens[start_idx..=index] {
                        if !matches!(wiki_token.kind, InlineTokenType::WikiLinkTitle) {
                            hidden.push((wiki_token.from, wiki_token.to));
                        }
                    }
                }
            }
            InlineTokenType::WikiLinkTitle => {
                if open_idx.is_some() {
                    has_title = true;
                }
            }
            _ => {}
        }
    }

    hidden
}

pub fn image_hidden_token_ranges(
    tokens: &[InlineToken],
    active_cursor_col: Option<usize>,
) -> Vec<(usize, usize)> {
    let mut hidden = Vec::new();
    let mut open_idx: Option<usize> = None;
    let mut marker_count = 0usize;

    for (index, token) in tokens.iter().enumerate() {
        match token.kind {
            InlineTokenType::ImageMarker => {
                if open_idx.is_none() {
                    open_idx = Some(index);
                    marker_count = 1;
                    continue;
                }
                marker_count += 1;
                if marker_count >= 3 {
                    let Some(start_idx) = open_idx.take() else {
                        continue;
                    };
                    let start = tokens[start_idx].from;
                    let end = token.to;
                    let cursor_inside = active_cursor_col
                        .map(|col| col >= start && col <= end)
                        .unwrap_or(false);
                    if !cursor_inside {
                        for image_token in &tokens[start_idx..=index] {
                            if !matches!(image_token.kind, InlineTokenType::ImageAlt) {
                                hidden.push((image_token.from, image_token.to));
                            }
                        }
                    }
                    marker_count = 0;
                }
            }
            _ => {}
        }
    }

    hidden
}

#[allow(dead_code)]
pub fn hidden_ranges_for_markdown_line(
    text: &str,
    active_cursor_col: Option<usize>,
) -> Vec<(usize, usize)> {
    hidden_ranges_for_markdown_line_with_formatting_boundary_exit(text, active_cursor_col, false)
}

fn hidden_ranges_for_markdown_line_with_formatting_boundary_exit(
    text: &str,
    active_cursor_col: Option<usize>,
    force_formatting_right_boundary_exit: bool,
) -> Vec<(usize, usize)> {
    let len = text.chars().count();
    if len == 0 {
        return Vec::new();
    }
    let info = markdown_tokens::classify_markdown_line(text);
    let mut hidden_ranges =
        hidden_line_prefix_marker_ranges(&info, len, active_cursor_col.is_some());
    let inline_tokens = inline_tokens_for_display(text);
    hidden_ranges.extend(hidden_inline_marker_ranges(
        &inline_tokens,
        len,
        active_cursor_col,
        force_formatting_right_boundary_exit,
    ));
    normalize_hidden_ranges(hidden_ranges, len)
}

#[allow(dead_code)]
pub fn collapse_markdown_line_for_cursor(text: &str, cursor_col: usize) -> (String, usize) {
    collapse_markdown_line_for_cursor_with_formatting_boundary_exit(text, cursor_col, false)
}

pub fn collapse_markdown_line_for_cursor_with_formatting_boundary_exit(
    text: &str,
    cursor_col: usize,
    force_formatting_right_boundary_exit: bool,
) -> (String, usize) {
    let chars: Vec<char> = text.chars().collect();
    let len = chars.len();
    if len == 0 {
        return (String::new(), 0);
    }
    if !text
        .chars()
        .any(|ch| matches!(ch, '#' | '>' | '`' | '[' | '*' | '_' | '~'))
    {
        let clamped_cursor = cursor_col.min(len);
        return (text.to_string(), clamped_cursor);
    }
    let clamped_cursor = cursor_col.min(len);
    let hidden_ranges = hidden_ranges_for_markdown_line_with_formatting_boundary_exit(
        text,
        Some(clamped_cursor),
        force_formatting_right_boundary_exit,
    );
    if hidden_ranges.is_empty() {
        return (text.to_string(), clamped_cursor);
    }

    let mut mapped_cursor = clamped_cursor;
    for (from, to) in &hidden_ranges {
        if *from >= clamped_cursor {
            break;
        }
        let removed = if *to <= clamped_cursor {
            to - from
        } else {
            clamped_cursor.saturating_sub(*from)
        };
        mapped_cursor = mapped_cursor.saturating_sub(removed);
    }

    let mut out = String::with_capacity(chars.len());
    let mut hidden_iter = hidden_ranges.iter().peekable();
    for (idx, ch) in chars.iter().enumerate() {
        while let Some((_, end)) = hidden_iter.peek() {
            if idx >= *end {
                hidden_iter.next();
            } else {
                break;
            }
        }
        if let Some((start, end)) = hidden_iter.peek() {
            if idx >= *start && idx < *end {
                continue;
            }
        }
        out.push(*ch);
    }
    let out_len = out.chars().count();
    (out, mapped_cursor.min(out_len))
}

pub fn formatting_component_right_boundary_at(text: &str, cursor_col: usize) -> bool {
    let tokens = inline_tokens_for_display(text);
    if tokens.is_empty() {
        return false;
    }
    markdown_tokens::inline_marker_component_ranges_from_tokens(&tokens)
        .into_iter()
        .any(|range| {
            cursor_col == range.to && component_has_formatting_body(&tokens, range.from, range.to)
        })
}

fn component_has_formatting_body(tokens: &[InlineToken], from: usize, to: usize) -> bool {
    tokens.iter().any(|token| {
        token.from >= from
            && token.to <= to
            && matches!(
                token.kind,
                InlineTokenType::Strong
                    | InlineTokenType::Emphasis
                    | InlineTokenType::Strikethrough
            )
    })
}

#[cfg(test)]
mod tests {
    use crate::editor_core::markdown_tokens::{self, InlineTokenType};

    use super::{
        collapse_markdown_line_for_cursor,
        collapse_markdown_line_for_cursor_with_formatting_boundary_exit,
        formatting_component_right_boundary_at, hidden_ranges_for_markdown_line,
        wiki_link_hidden_token_ranges,
    };

    #[test]
    fn collapse_markdown_line_for_cursor_removes_hidden_marker_gaps_and_maps_cursor() {
        let (collapsed, mapped_col) = collapse_markdown_line_for_cursor("`code` tail **bold**", 10);
        assert_eq!(collapsed, "code tail bold");
        assert_eq!(mapped_col, 8);
    }

    #[test]
    fn collapse_markdown_line_for_cursor_does_not_collapse_mid_word_underscores() {
        let (collapsed, mapped_col) = collapse_markdown_line_for_cursor("this_Is_my_Word", 7);
        assert_eq!(collapsed, "this_Is_my_Word");
        assert_eq!(mapped_col, 7);
    }

    #[test]
    fn collapse_markdown_line_for_cursor_treats_inline_code_right_boundary_as_outside() {
        let (collapsed, mapped_col) = collapse_markdown_line_for_cursor("(`xx`)", 5);
        assert_eq!(collapsed, "(xx)");
        assert_eq!(mapped_col, 3);
    }

    #[test]
    fn collapse_markdown_line_for_cursor_reveals_formatting_at_right_boundary() {
        let (collapsed, mapped_col) = collapse_markdown_line_for_cursor("**bold**", 8);
        assert_eq!(collapsed, "**bold**");
        assert_eq!(mapped_col, 8);
    }

    #[test]
    fn collapse_markdown_line_for_cursor_can_force_exit_at_formatting_right_boundary() {
        let (collapsed, mapped_col) =
            collapse_markdown_line_for_cursor_with_formatting_boundary_exit(
                "**bold** tail",
                8,
                true,
            );
        assert_eq!(collapsed, "bold tail");
        assert_eq!(mapped_col, 4);
    }

    #[test]
    fn formatting_component_right_boundary_detects_inline_formatting_exit_point() {
        assert!(formatting_component_right_boundary_at("**bold** tail", 8));
        assert!(!formatting_component_right_boundary_at("**bold** tail", 9));
        assert!(!formatting_component_right_boundary_at("`code` tail", 6));
    }

    #[test]
    fn wiki_link_alt_hides_source_segments_when_cursor_outside_link() {
        let line = "[[01HX4VHR#Intro|My Alt]]";
        let hidden = hidden_ranges_for_markdown_line(line, None);
        assert_eq!(hidden, vec![(0, 17), (23, 25)]);
    }

    #[test]
    fn wiki_link_alt_reveals_source_segments_when_cursor_inside_link() {
        let line = "[[01HX4VHR#Intro|My Alt]]";
        let hidden = hidden_ranges_for_markdown_line(line, Some(10));
        assert!(hidden.is_empty());
    }

    #[test]
    fn wiki_link_alt_reveals_source_segments_at_right_boundary() {
        let line = "[[01HX4VHR#Intro|My Alt]]";
        let hidden = hidden_ranges_for_markdown_line(line, Some(line.chars().count()));
        assert!(hidden.is_empty());
    }

    #[test]
    fn markdown_link_reveals_source_at_right_boundary() {
        let line = "[guide](./docs/guide.md)";
        let hidden = hidden_ranges_for_markdown_line(line, Some(line.chars().count()));
        assert!(hidden.is_empty());
    }

    #[test]
    fn markdown_link_reveals_source_at_left_boundary() {
        let line = "[guide](./docs/guide.md)";
        let hidden = hidden_ranges_for_markdown_line(line, Some(0));
        assert!(hidden.is_empty());
    }

    #[test]
    fn wiki_link_hidden_token_ranges_only_hide_non_title_segments_for_alt_text() {
        let line = "[[01HX4VHR#Intro|My Alt]] tail";
        let tokens = markdown_tokens::tokenize_inline_markdown(line);
        let title = tokens
            .iter()
            .find(|token| matches!(token.kind, InlineTokenType::WikiLinkTitle))
            .expect("wiki-link title token");
        let hidden = wiki_link_hidden_token_ranges(&tokens, None);

        assert!(!hidden.is_empty());
        assert!(hidden
            .iter()
            .all(|(from, to)| *to <= title.from || *from >= title.to));
    }

    #[test]
    fn image_hidden_token_ranges_hide_markers_and_src_when_cursor_outside_image() {
        let line = "![diagram](./assets/plan.png) tail";
        let hidden = hidden_ranges_for_markdown_line(line, None);
        assert_eq!(hidden, vec![(0, 2), (9, 29)]);
    }

    #[test]
    fn image_hidden_token_ranges_reveal_source_when_cursor_inside_image() {
        let line = "![diagram](./assets/plan.png) tail";
        let hidden = hidden_ranges_for_markdown_line(line, Some(12));
        assert!(hidden.is_empty());
    }

    #[test]
    fn image_hidden_token_ranges_reveal_source_at_right_boundary() {
        let line = "![diagram](./assets/plan.png) tail";
        let boundary = "![diagram](./assets/plan.png)".chars().count();
        let hidden = hidden_ranges_for_markdown_line(line, Some(boundary));
        assert!(hidden.is_empty());
    }

    #[test]
    fn image_hidden_token_ranges_reveal_source_at_left_boundary() {
        let line = "![diagram](./assets/plan.png) tail";
        let hidden = hidden_ranges_for_markdown_line(line, Some(0));
        assert!(hidden.is_empty());
    }
}
