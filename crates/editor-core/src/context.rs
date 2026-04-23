use crate::types::{
    BlockLineRange, EditorContextSnapshot, LineContext, SelectionContext, TextRange, WordContext,
};
use std::borrow::Cow;

#[derive(Debug, Clone)]
struct ParsedLines {
    starts: Vec<usize>,
    ends: Vec<usize>,
}

fn clamp(value: usize, min: usize, max: usize) -> usize {
    value.max(min).min(max)
}

fn parse_lines(text: &str) -> ParsedLines {
    if text.is_empty() {
        return ParsedLines {
            starts: vec![0],
            ends: vec![0],
        };
    }

    let mut starts = Vec::new();
    let mut ends = Vec::new();
    let mut start = 0;
    for (idx, ch) in text.char_indices() {
        if ch == '\n' {
            starts.push(start);
            ends.push(idx);
            start = idx + 1;
        }
    }
    starts.push(start);
    ends.push(text.len());

    ParsedLines { starts, ends }
}

fn is_list_line(line: &str) -> bool {
    let trimmed = line.trim_start();
    if trimmed.is_empty() {
        return false;
    }

    let Some(space_idx) = trimmed.find(char::is_whitespace) else {
        return false;
    };
    let marker = &trimmed[..space_idx];

    if matches!(marker, "-" | "*" | "+" | "->") {
        return true;
    }

    let is_digits = |segment: &str| {
        !segment.is_empty() && segment.as_bytes().iter().all(|b| b.is_ascii_digit())
    };

    if let Some(stripped) = marker.strip_suffix('.') {
        return is_digits(stripped)
            || (stripped.contains('.') && stripped.split('.').all(is_digits));
    }

    marker.contains('.') && marker.split('.').all(is_digits)
}

fn is_table_line(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.starts_with('|') && trimmed.ends_with('|')
}

fn is_word_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

#[derive(Debug, Clone)]
pub struct ResolvedContext<'a> {
    text: Cow<'a, str>,
    selection: crate::types::SelectionSnapshot,
    changed_range: Option<TextRange>,
    parsed: ParsedLines,
}

impl ResolvedContext<'static> {
    pub fn new(snapshot: EditorContextSnapshot) -> Self {
        let parsed = parse_lines(&snapshot.text);
        Self {
            text: Cow::Owned(snapshot.text),
            selection: snapshot.selection,
            changed_range: snapshot.changed_range,
            parsed,
        }
    }
}

impl<'a> ResolvedContext<'a> {
    /// Construct from borrowed text with owned selection/range metadata.
    /// Used by wasm hot paths to avoid cloning full document text.
    pub fn from_parts(
        text: &'a str,
        selection: crate::types::SelectionSnapshot,
        changed_range: Option<TextRange>,
    ) -> Self {
        let parsed = parse_lines(text);
        Self {
            text: Cow::Borrowed(text),
            selection,
            changed_range,
            parsed,
        }
    }

    pub fn to_snapshot(&self) -> EditorContextSnapshot {
        EditorContextSnapshot {
            text: self.text().to_string(),
            selection: self.selection,
            changed_range: self.changed_range,
        }
    }

    pub fn text(&self) -> &str {
        self.text.as_ref()
    }

    pub fn selection(&self) -> SelectionContext {
        let max = self.text().len();
        let anchor = self.selection.anchor.min(max);
        let head = self.selection.head.min(max);
        let from = anchor.min(head);
        let to = anchor.max(head);
        SelectionContext {
            anchor,
            head,
            from,
            to,
            empty: from == to,
        }
    }

    pub fn cursor_pos(&self) -> usize {
        self.selection().head
    }

    pub fn changed_range(&self) -> Option<TextRange> {
        self.changed_range
    }

    pub fn line_count(&self) -> usize {
        self.parsed.starts.len()
    }

    pub fn line(&self, number: usize) -> LineContext {
        let idx = clamp(
            number.saturating_sub(1),
            0,
            self.parsed.starts.len().saturating_sub(1),
        );
        self.line_from_index(idx)
    }

    pub fn line_at(&self, pos: usize) -> LineContext {
        let p = pos.min(self.text().len());
        let idx = match self.parsed.starts.binary_search(&p) {
            Ok(found) => found,
            Err(insert) => insert.saturating_sub(1),
        };
        self.line_from_index(idx)
    }

    pub fn current_line(&self) -> LineContext {
        self.line_at(self.cursor_pos())
    }

    pub fn current_column(&self) -> usize {
        let line = self.current_line();
        self.cursor_pos() - line.from
    }

    pub fn line_text(&self, number: usize) -> &str {
        let idx = clamp(
            number.saturating_sub(1),
            0,
            self.parsed.starts.len().saturating_sub(1),
        );
        let from = self.parsed.starts[idx];
        let to = self.parsed.ends[idx];
        &self.text()[from..to]
    }

    pub fn text_for_line_range(&self, range: BlockLineRange) -> &str {
        let start_idx = clamp(
            range.start_line.saturating_sub(1),
            0,
            self.parsed.starts.len().saturating_sub(1),
        );
        let end_idx = clamp(
            range.end_line.saturating_sub(1),
            0,
            self.parsed.starts.len().saturating_sub(1),
        );
        let from = self.parsed.starts[start_idx];
        let to = self.parsed.ends[end_idx];
        &self.text()[from..to]
    }

    pub fn paragraph_range_at_line(&self, line_number: usize) -> BlockLineRange {
        let line_count = self.line_count();
        let cursor = clamp(line_number, 1, line_count);
        let mut start = cursor;
        let mut end = cursor;

        while start > 1 && !self.line_text(start - 1).trim().is_empty() {
            start -= 1;
        }
        while end < line_count && !self.line_text(end + 1).trim().is_empty() {
            end += 1;
        }

        BlockLineRange {
            start_line: start,
            end_line: end,
        }
    }

    pub fn list_range_at_line(&self, line_number: usize) -> Option<BlockLineRange> {
        let line_count = self.line_count();
        let cursor = clamp(line_number, 1, line_count);
        if !is_list_line(self.line_text(cursor)) {
            return None;
        }

        let mut start = cursor;
        let mut end = cursor;
        while start > 1 && is_list_line(self.line_text(start - 1)) {
            start -= 1;
        }
        while end < line_count && is_list_line(self.line_text(end + 1)) {
            end += 1;
        }

        Some(BlockLineRange {
            start_line: start,
            end_line: end,
        })
    }

    pub fn table_range_at_line(
        &self,
        line_number: usize,
        min_rows: usize,
    ) -> Option<BlockLineRange> {
        let line_count = self.line_count();
        let cursor = clamp(line_number, 1, line_count);
        if !is_table_line(self.line_text(cursor)) {
            return None;
        }

        let mut start = cursor;
        let mut end = cursor;
        while start > 1 && is_table_line(self.line_text(start - 1)) {
            start -= 1;
        }
        while end < line_count && is_table_line(self.line_text(end + 1)) {
            end += 1;
        }

        if end - start + 1 < min_rows {
            return None;
        }

        Some(BlockLineRange {
            start_line: start,
            end_line: end,
        })
    }

    pub fn word_at(&self, pos: Option<usize>) -> Option<WordContext> {
        let bytes = self.text().as_bytes();
        if bytes.is_empty() {
            return None;
        }

        let p = clamp(pos.unwrap_or(self.cursor_pos()), 0, bytes.len());
        let left = if p > 0 { bytes[p - 1] } else { 0 };
        let right = if p < bytes.len() { bytes[p] } else { 0 };
        if !is_word_byte(left) && !is_word_byte(right) {
            return None;
        }

        let mut from = p;
        let mut to = p;
        while from > 0 && is_word_byte(bytes[from - 1]) {
            from -= 1;
        }
        while to < bytes.len() && is_word_byte(bytes[to]) {
            to += 1;
        }
        if from >= to {
            return None;
        }

        Some(WordContext {
            from,
            to,
            text: String::from_utf8_lossy(&bytes[from..to]).into_owned(),
        })
    }

    pub fn position_for_line_column(&self, line_number: usize, column: usize) -> usize {
        let line = self.line(line_number);
        line.from + column.min(line.text.len())
    }

    fn line_from_index(&self, idx: usize) -> LineContext {
        let safe_idx = idx.min(self.parsed.starts.len().saturating_sub(1));
        let from = self.parsed.starts[safe_idx];
        let to = self.parsed.ends[safe_idx];
        let text = self.text()[from..to].to_string();
        LineContext {
            number: safe_idx + 1,
            from,
            to,
            text,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::SelectionSnapshot;

    fn ctx(text: &str, head: usize, anchor: usize) -> ResolvedContext<'static> {
        ResolvedContext::new(EditorContextSnapshot {
            text: text.to_string(),
            selection: SelectionSnapshot { anchor, head },
            changed_range: None,
        })
    }

    #[test]
    fn line_lookup_follows_cursor_positions() {
        let resolved = ctx("a\nbc", 1, 1);
        assert_eq!(resolved.current_line().number, 1);
        assert_eq!(resolved.current_column(), 1);
        assert_eq!(resolved.line_at(2).number, 2);
        assert_eq!(resolved.line(2).text, "bc");
    }

    #[test]
    fn resolves_paragraph_list_and_table_ranges() {
        let text = "- a\n- b\n\nc\nd\n\n| a | 1 |\n| b | 2 |";
        let resolved = ctx(text, 1, 1);
        assert_eq!(
            resolved.list_range_at_line(1),
            Some(BlockLineRange {
                start_line: 1,
                end_line: 2
            })
        );
        assert_eq!(
            resolved.paragraph_range_at_line(4),
            BlockLineRange {
                start_line: 4,
                end_line: 5
            }
        );
        assert_eq!(
            resolved.table_range_at_line(7, 2),
            Some(BlockLineRange {
                start_line: 7,
                end_line: 8
            })
        );
        assert_eq!(resolved.table_range_at_line(4, 2), None);
    }

    #[test]
    fn recognizes_arrow_and_hierarchical_ordered_lists() {
        let text = "1.1 parent\n  -> child\nplain";
        let resolved = ctx(text, 2, 2);
        assert_eq!(
            resolved.list_range_at_line(1),
            Some(BlockLineRange {
                start_line: 1,
                end_line: 2
            })
        );
    }

    #[test]
    fn resolves_words_around_cursor() {
        let text = "hello world";
        assert_eq!(
            ctx(text, 1, 1).word_at(None).map(|w| w.text),
            Some("hello".to_string())
        );
        assert_eq!(
            ctx(text, 5, 5).word_at(None).map(|w| w.text),
            Some("hello".to_string())
        );
        assert_eq!(
            ctx(text, 6, 6).word_at(None).map(|w| w.text),
            Some("world".to_string())
        );
        assert_eq!(
            ctx(text, 11, 11).word_at(None).map(|w| w.text),
            Some("world".to_string())
        );
    }

    #[test]
    fn selection_is_clamped_to_text_bounds() {
        let resolved = ctx("abc", 50, 10);
        assert_eq!(
            resolved.selection(),
            SelectionContext {
                anchor: 3,
                head: 3,
                from: 3,
                to: 3,
                empty: true
            }
        );
    }
}
