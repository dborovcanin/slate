//! EditOperation mutation and exact coordinate mapping, independent of frontend policy.
use crate::{Document, NoteSession};
use editor_core::buffer::{
    apply_text_change_in_place, document_text_len, line_and_byte_for_offset,
    map_offset_through_changes, prepare_text_change, EditDelta,
};
use editor_core::types::EditOperation;

pub struct OperationOutcome {
    pub delta: EditDelta,
    /// Multiple-change operations retain both each splice and the combined span.
    pub calc_splices: Vec<EditDelta>,
    pub fold_rescan: bool,
}
impl NoteSession {
    pub(crate) fn apply_operation_text(
        &mut self,
        doc: &mut Document,
        op: &EditOperation,
    ) -> Option<OperationOutcome> {
        if op.changes.is_empty() {
            if let Some(sel) = &op.selection {
                let target = sel.anchor.min(document_text_len(&doc.lines));
                let (line_idx, line_byte) = line_and_byte_for_offset(&doc.lines, target);
                if let Some(line) = doc.lines.get(line_idx) {
                    doc.cursor_line = line_idx;
                    doc.cursor_col = char_col_at_byte(line, line_byte);
                }
            }
            return None;
        }

        if op.changes.len() == 1 {
            let change = &op.changes[0];
            let doc_len = document_text_len(&doc.lines);
            let from = change.from.min(doc_len);
            let to = change.to.min(doc_len);
            let prepared = prepare_text_change(&doc.lines, change, doc_len);
            let edit = prepared.edit;
            let from_line = edit.from.0;

            let mut mapped_anchor = byte_offset_for_cursor(doc);
            if from <= mapped_anchor {
                if to <= mapped_anchor {
                    let removed = to.saturating_sub(from);
                    let added = change.insert.len();
                    mapped_anchor = mapped_anchor.saturating_add(added).saturating_sub(removed);
                } else {
                    let inside = mapped_anchor.saturating_sub(from);
                    mapped_anchor = from.saturating_add(inside.min(change.insert.len()));
                }
            }

            self.record_exact(edit);
            let delta = apply_text_change_in_place(&mut doc.lines, prepared);
            let old_line_span = delta.old_span;
            let new_line_span = delta.new_span;

            let new_doc_len = doc_len
                .saturating_add(change.insert.len())
                .saturating_sub(to.saturating_sub(from));
            let final_anchor = op
                .selection
                .as_ref()
                .map_or(mapped_anchor, |selection| selection.anchor)
                .min(new_doc_len);
            let (line_idx, line_byte) = line_and_byte_for_offset(&doc.lines, final_anchor);
            if let Some(line) = doc.lines.get(line_idx) {
                doc.cursor_line = line_idx;
                doc.cursor_col = char_col_at_byte(line, line_byte);
            }
            return Some(OperationOutcome {
                delta,
                calc_splices: Vec::new(),
                fold_rescan: !(old_line_span == 1
                    && new_line_span == 1
                    && from_line == doc.cursor_line),
            });
        }

        let old_doc_len = document_text_len(&doc.lines);
        let changed_from_offset = op
            .changes
            .iter()
            .map(|change| change.from.min(old_doc_len))
            .min()
            .unwrap_or(0);
        let changed_to_offset_old = op
            .changes
            .iter()
            .map(|change| change.to.min(old_doc_len))
            .max()
            .unwrap_or(changed_from_offset);
        let changed_from_line = line_and_byte_for_offset(&doc.lines, changed_from_offset).0;
        let old_changed_to_line_exclusive =
            line_and_byte_for_offset(&doc.lines, changed_to_offset_old).0 + 1;

        let original_anchor = byte_offset_for_cursor(doc);
        let mut changes = op.changes.clone();
        changes.sort_by(|a, b| b.from.cmp(&a.from));
        let mapped_anchor = map_offset_through_changes(original_anchor, &changes);

        let mut calc_splices = Vec::with_capacity(changes.len() + 1);
        let mut current_doc_len = old_doc_len;
        for change in &changes {
            let from = change.from.min(current_doc_len);
            let to = change.to.min(current_doc_len);
            let removed = to.saturating_sub(from);
            let prepared = prepare_text_change(&doc.lines, change, current_doc_len);
            let edit = prepared.edit;
            self.record_exact(edit);
            let delta = apply_text_change_in_place(&mut doc.lines, prepared);
            calc_splices.push(delta);
            current_doc_len = current_doc_len
                .saturating_add(change.insert.len())
                .saturating_sub(removed);
        }

        let mapped_from =
            map_offset_through_changes(changed_from_offset, &changes).min(current_doc_len);
        let mapped_to =
            map_offset_through_changes(changed_to_offset_old, &changes).min(current_doc_len);
        let mapped_changed_to = mapped_from.max(mapped_to);
        let new_changed_to_line_exclusive =
            line_and_byte_for_offset(&doc.lines, mapped_changed_to).0 + 1;
        let old_line_span = old_changed_to_line_exclusive
            .saturating_sub(changed_from_line)
            .max(1);
        let new_line_span = new_changed_to_line_exclusive
            .saturating_sub(changed_from_line)
            .max(1);
        let delta = EditDelta {
            start_line: changed_from_line,
            old_span: old_line_span,
            new_span: new_line_span,
        };
        calc_splices.push(delta);

        let final_anchor = if let Some(sel) = &op.selection {
            sel.anchor
        } else {
            mapped_anchor
        }
        .min(current_doc_len);
        let (line_idx, line_byte) = line_and_byte_for_offset(&doc.lines, final_anchor);
        if let Some(line) = doc.lines.get(line_idx) {
            doc.cursor_line = line_idx;
            doc.cursor_col = char_col_at_byte(line, line_byte);
        }
        Some(OperationOutcome {
            delta,
            calc_splices,
            fold_rescan: true,
        })
    }
}
fn byte_index(text: &str, column: usize) -> usize {
    text.char_indices()
        .nth(column)
        .map_or(text.len(), |(i, _)| i)
}
fn byte_offset_for_cursor(doc: &Document) -> usize {
    let mut offset = 0;
    for (i, line) in doc.lines.iter().enumerate() {
        if i == doc.cursor_line {
            offset += byte_index(line, doc.cursor_col);
            break;
        }
        offset += line.len() + 1;
    }
    offset
}
fn char_col_at_byte(text: &str, byte: usize) -> usize {
    let mut end = byte.min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].chars().count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use editor_core::{
        history::LineHistory,
        types::{OperationSelection, TextChange},
    };
    use std::sync::Arc;
    fn fixture(text: &str, column: usize) -> (Document, NoteSession) {
        let doc = Document {
            lines: text.split('\n').map(str::to_owned).collect(),
            cursor_col: column,
            ..Default::default()
        };
        let history = LineHistory::new(500, &doc.lines, 0, column, Arc::new(Vec::new()));
        let mut session = NoteSession::new(history, Default::default(), Default::default());
        session.reminder_ghosts.insert(
            0,
            crate::LineReminderGhost {
                remind_at_ms: 1,
                display_at: "later".into(),
                line_text: doc.lines[0].clone(),
                reminded_at_ms: None,
            },
        );
        (doc, session)
    }
    #[test]
    fn single_replacement_transforms_character_cursor_and_records_exact_reminder_edit() {
        let (mut doc, mut session) = fixture("é🙂 tail", 3);
        let op = EditOperation {
            changes: vec![TextChange {
                from: 2,
                to: 6,
                insert: "λ".into(),
            }],
            selection: None,
        };
        let outcome = session.apply_operation_text(&mut doc, &op).unwrap();
        assert_eq!(doc.lines, vec!["éλ tail"]);
        assert_eq!(doc.cursor_col, 3);
        assert_eq!(
            outcome.delta,
            EditDelta {
                start_line: 0,
                old_span: 1,
                new_span: 1
            }
        );
        assert!(!outcome.fold_rescan);
        assert!(outcome.calc_splices.is_empty());
        assert_eq!(session.pending_line_edits.len(), 1);
        // Mutation alone does not run lifecycle bookkeeping.
        assert_eq!(doc.text_generation, 0);
        assert!(!session.dirty);
    }
    #[test]
    fn multiple_unicode_changes_keep_descending_exact_coordinates_and_combined_splice() {
        let (mut doc, mut session) = fixture("é🙂\nβ end", 2);
        let op = EditOperation {
            changes: vec![
                TextChange {
                    from: 0,
                    to: 2,
                    insert: "λ\n".into(),
                },
                TextChange {
                    from: 7,
                    to: 9,
                    insert: "γ".into(),
                },
            ],
            selection: None,
        };
        let outcome = session.apply_operation_text(&mut doc, &op).unwrap();
        assert_eq!(doc.lines, vec!["λ", "🙂", "γ end"]);
        assert_eq!((doc.cursor_line, doc.cursor_col), (1, 1));
        assert_eq!(outcome.calc_splices.len(), 3);
        assert_eq!(outcome.calc_splices[0].start_line, 1);
        assert_eq!(
            outcome.calc_splices[1],
            EditDelta {
                start_line: 0,
                old_span: 1,
                new_span: 2
            }
        );
        assert_eq!(
            outcome.delta,
            EditDelta {
                start_line: 0,
                old_span: 2,
                new_span: 3
            }
        );
        assert_eq!(session.pending_line_edits.len(), 2);
        assert!(outcome.fold_rescan);
    }
    #[test]
    fn selection_only_rounds_byte_cursor_without_recording_an_edit() {
        let (mut doc, mut session) = fixture("é🙂", 0);
        let op = EditOperation {
            changes: vec![],
            selection: Some(OperationSelection {
                anchor: 3,
                head: None,
            }),
        };
        assert!(session.apply_operation_text(&mut doc, &op).is_none());
        assert_eq!(doc.cursor_col, 1);
        assert!(session.pending_line_edits.is_empty());
    }
}
