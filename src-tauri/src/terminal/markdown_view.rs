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
    cursor_col >= from && cursor_col <= to
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
) -> Vec<(usize, usize)> {
    let component_ranges = markdown_tokens::inline_marker_component_ranges_from_tokens(tokens);
    let mut hidden_ranges = Vec::new();
    for (index, token) in tokens.iter().enumerate() {
        let from = token.from.min(len);
        let to = token.to.min(len);
        if to <= from {
            continue;
        }
        if markdown_tokens::is_inline_marker_token_kind(token.kind)
            && !should_reveal_inline_marker(tokens, &component_ranges, index, active_cursor_col)
        {
            hidden_ranges.push((from, to));
        }
    }
    hidden_ranges
}

pub fn hidden_ranges_for_markdown_line(
    text: &str,
    active_cursor_col: Option<usize>,
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
    ));
    normalize_hidden_ranges(hidden_ranges, len)
}

pub fn collapse_markdown_line_for_cursor(text: &str, cursor_col: usize) -> (String, usize) {
    let chars: Vec<char> = text.chars().collect();
    let len = chars.len();
    if len == 0 {
        return (String::new(), 0);
    }
    let clamped_cursor = cursor_col.min(len);
    let hidden_ranges = hidden_ranges_for_markdown_line(text, Some(clamped_cursor));
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

#[cfg(test)]
mod tests {
    use super::collapse_markdown_line_for_cursor;

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
}
