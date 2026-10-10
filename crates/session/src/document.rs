//! Canonical document text, cursor and selection state.
use editor_core::buffer::primitives::BufferCursor;

/// Canonical text is writable only inside the session crate.
#[derive(Default)]
pub struct Document {
    pub(crate) lines: Vec<String>,
    pub joined_text_cache: Option<String>,
    /// Changes with text/cache invalidation, without rehashing the document.
    pub text_generation: u64,
    pub cursor_line: usize,
    /// Character column, not a byte offset.
    pub cursor_col: usize,
    pub selection_anchor: Option<(usize, usize)>,
}

impl Document {
    /// Initialize owned lines without an edit-generation bump.
    pub fn from_lines(lines: Vec<String>) -> Self {
        Self {
            lines,
            ..Default::default()
        }
    }
    pub fn from_text(text: &str) -> Self {
        Self::from_lines(text.split('\n').map(str::to_owned).collect())
    }
    pub fn lines(&self) -> &[String] {
        &self.lines
    }
    /// Replace document text on open/reload; invalidate derived text once.
    pub fn set_text(&mut self, text: &str) {
        self.lines = text.split('\n').map(str::to_owned).collect();
        self.joined_text_cache = None;
        self.text_generation = self.text_generation.wrapping_add(1);
    }

    /// Release unused text capacity after switching away from a large note.
    pub fn compact(&mut self) {
        self.lines.shrink_to_fit();
    }

    pub fn cursor(&self) -> BufferCursor {
        BufferCursor {
            line: self.cursor_line,
            column: self.cursor_col,
        }
    }
    pub fn set_cursor(&mut self, cursor: BufferCursor) {
        self.cursor_line = cursor.line;
        self.cursor_col = cursor.column;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn replacing_text_preserves_unicode_and_trailing_line_and_invalidates_once() {
        let mut doc = Document::from_text("old");
        doc.joined_text_cache = Some("old".into());
        doc.set_text("éλ\n");
        assert_eq!(doc.lines(), &["éλ", ""]);
        assert_eq!(doc.text_generation, 1);
        assert!(doc.joined_text_cache.is_none());
    }
    #[test]
    fn default_preserves_empty_buffer_and_cursor() {
        let doc = Document::default();
        assert!(doc.lines.is_empty());
        assert!(doc.joined_text_cache.is_none());
        assert_eq!(doc.text_generation, 0);
        assert_eq!(doc.cursor(), BufferCursor { line: 0, column: 0 });
        assert_eq!(doc.selection_anchor, None);
    }
    #[test]
    fn cursor_roundtrip_uses_character_columns() {
        let mut doc = Document {
            lines: vec!["éλ".into()],
            ..Default::default()
        };
        doc.set_cursor(BufferCursor { line: 0, column: 2 });
        assert_eq!(doc.cursor(), BufferCursor { line: 0, column: 2 });
    }
}
