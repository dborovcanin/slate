//! Script result planning uses the same byte ranges and operations as commands.
use crate::types::{EditOperation, OperationSelection, SelectionSnapshot, TextChange, TextRange};

/// Vim visual endpoints include the character under the farther cursor.
/// Linewise snapshots already span complete lines and can select an empty line.
pub fn visual_selection_range(
    text: &str,
    selection: SelectionSnapshot,
    linewise: bool,
) -> Result<TextRange, String> {
    let from = selection.anchor.min(selection.head);
    let mut to = selection.anchor.max(selection.head);
    if text.get(from..to).is_none() {
        return Err("invalid script selection".into());
    }
    if !linewise {
        to += text[to..].chars().next().map_or(0, char::len_utf8);
        if from == to {
            return Err("script requires a selection".into());
        }
    }
    Ok(TextRange { from, to })
}

pub fn plan_result(range: TextRange, text: String) -> EditOperation {
    let cursor = range.from + text.len();
    EditOperation {
        changes: vec![TextChange {
            from: range.from,
            to: range.to,
            insert: text,
        }],
        selection: Some(OperationSelection {
            anchor: cursor,
            head: None,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn visual_selection_includes_cursor_character_and_empty_lines() {
        assert_eq!(
            visual_selection_range("é日z", SelectionSnapshot { anchor: 2, head: 0 }, false)
                .unwrap(),
            TextRange { from: 0, to: 5 }
        );
        assert_eq!(
            visual_selection_range("é", SelectionSnapshot { anchor: 0, head: 0 }, false).unwrap(),
            TextRange { from: 0, to: 2 }
        );
        assert_eq!(
            visual_selection_range("", SelectionSnapshot { anchor: 0, head: 0 }, true).unwrap(),
            TextRange { from: 0, to: 0 }
        );
    }
    #[test]
    fn reversed_unicode_selection_and_result() {
        let range =
            visual_selection_range("aé日z", SelectionSnapshot { anchor: 3, head: 1 }, false)
                .unwrap();
        assert_eq!(range, TextRange { from: 1, to: 6 });
        let op = plan_result(range, "ü".into());
        assert_eq!(op.selection.unwrap().anchor, 3);
        assert_eq!(op.changes[0].to, 6);
        assert!(
            visual_selection_range("é", SelectionSnapshot { anchor: 0, head: 1 }, false).is_err()
        );
        assert!(
            visual_selection_range("", SelectionSnapshot { anchor: 0, head: 0 }, false).is_err()
        );
    }
}
