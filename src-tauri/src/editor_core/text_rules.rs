use super::types::{EditOperation, EditorContextSnapshot};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextRuleOptions {
    pub markdown_autoformat: bool,
}

impl Default for TextRuleOptions {
    fn default() -> Self {
        Self {
            markdown_autoformat: true,
        }
    }
}

pub fn run_doc_change_rules(
    _snapshot: &EditorContextSnapshot,
    _options: TextRuleOptions,
) -> Option<EditOperation> {
    // Placeholder for Rust-native text rule execution.
    None
}

pub fn run_enter_rules(
    _snapshot: &EditorContextSnapshot,
    _options: TextRuleOptions,
) -> Option<EditOperation> {
    // Placeholder for Rust-native enter key text rules.
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor_core::types::SelectionSnapshot;

    fn snapshot(text: &str, head: usize, anchor: usize) -> EditorContextSnapshot {
        EditorContextSnapshot {
            text: text.to_string(),
            selection: SelectionSnapshot { anchor, head },
            changed_range: None,
        }
    }

    #[test]
    fn placeholder_rules_return_none() {
        let doc = snapshot("- [ ] task /x", 12, 12);
        assert_eq!(run_doc_change_rules(&doc, TextRuleOptions::default()), None);
        assert_eq!(run_enter_rules(&doc, TextRuleOptions::default()), None);
    }
}
