//! Canonical document text, cursor and selection state.
use editor_core::buffer::primitives::BufferCursor;

/// Document fields remain public during the mechanical extraction; the edit
/// pipeline will make text mutation private in a subsequent step.
#[derive(Default)]
pub struct Document {
    pub lines: Vec<String>,
    pub joined_text_cache: Option<String>,
    /// Changes with text/cache invalidation, without rehashing the document.
    pub text_generation: u64,
    pub cursor_line: usize,
    /// Character column, not a byte offset.
    pub cursor_col: usize,
    pub selection_anchor: Option<(usize, usize)>,
}

impl Document {
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
