//! In-place buffer edits and byte-offset mapping, independent of the host.
use crate::types::TextChange;

pub mod lines;
pub mod paste;
pub mod primitives;
pub mod replace;
pub mod words;

pub use replace::{apply_line_replace, prepare_line_replace, LineReplacePlan};

/// Line-span summary for invalidating derived state; not an exact edit map.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EditDelta {
    pub start_line: usize,
    pub old_span: usize,
    pub new_span: usize,
}

/// Coordinates in the buffer immediately before this change is applied.
/// Columns and boundary-line lengths are bytes, not character counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExactTextEdit {
    pub from: (usize, usize),
    pub to: (usize, usize),
    pub from_line_len: usize,
    pub to_line_len: usize,
    pub inserted_breaks: usize,
}

/// Replacement lines prepared without mutating the source buffer.
/// The host can record the exact edit before consuming it to apply the change.
pub struct PreparedTextChange {
    pub edit: ExactTextEdit,
    pub delta: EditDelta,
    replacement: Vec<String>,
}

/// Map a byte offset through changes supplied in descending start-offset order.
pub fn map_offset_through_changes(mut offset: usize, changes_desc: &[TextChange]) -> usize {
    for change in changes_desc {
        let from = change.from;
        let to = change.to.max(from);
        let added = change.insert.len();
        let removed = to.saturating_sub(from);
        if from <= offset {
            if to <= offset {
                offset = offset.saturating_add(added).saturating_sub(removed);
            } else {
                let inside = offset.saturating_sub(from);
                offset = from.saturating_add(inside.min(added));
            }
        }
    }
    offset
}

/// Byte length of the document, including one newline between adjacent lines.
pub fn document_text_len(lines: &[String]) -> usize {
    if lines.len() == 1 && lines.first().is_some_and(String::is_empty) {
        0
    } else {
        let line_bytes: usize = lines.iter().map(|line| line.len()).sum();
        line_bytes.saturating_add(lines.len().saturating_sub(1))
    }
}

/// Resolve a byte offset, clamping beyond the document to its final line end.
pub fn line_and_byte_for_offset(lines: &[String], target: usize) -> (usize, usize) {
    if lines.is_empty() {
        return (0, 0);
    }
    let mut offset = 0usize;
    for (idx, line) in lines.iter().enumerate() {
        let line_end = offset + line.len();
        if target <= line_end {
            return (idx, target.saturating_sub(offset));
        }
        offset = line_end + 1;
    }
    let last = lines.len().saturating_sub(1);
    (last, lines[last].len())
}

/// Prepare replacement lines and metadata without changing the buffer.
/// `doc_len` is the current document byte length; change offsets must be UTF-8
/// boundaries. This preserves the existing replacement and end-clamping rules.
pub fn prepare_text_change(
    lines: &[String],
    change: &TextChange,
    doc_len: usize,
) -> PreparedTextChange {
    let from = change.from.min(doc_len);
    let to = change.to.min(doc_len);
    let (from_line, from_byte) = line_and_byte_for_offset(lines, from);
    let (to_line, to_byte) = line_and_byte_for_offset(lines, to);

    let from_text = lines.get(from_line).cloned().unwrap_or_default();
    let to_text = lines.get(to_line).cloned().unwrap_or_default();
    let prefix = &from_text[..from_byte.min(from_text.len())];
    let suffix = &to_text[to_byte.min(to_text.len())..];
    let insert_parts = change.insert.split('\n').collect::<Vec<_>>();
    let mut replacement = Vec::with_capacity(insert_parts.len().max(1));

    if insert_parts.len() <= 1 {
        replacement.push(format!(
            "{prefix}{}{suffix}",
            insert_parts.first().copied().unwrap_or("")
        ));
    } else {
        replacement.push(format!("{prefix}{}", insert_parts[0]));
        for part in &insert_parts[1..insert_parts.len() - 1] {
            replacement.push((*part).to_string());
        }
        replacement.push(format!(
            "{}{suffix}",
            insert_parts.last().copied().unwrap_or("")
        ));
    }

    let old_line_span = to_line.saturating_sub(from_line).saturating_add(1);
    let new_line_span = replacement.len().max(1);
    PreparedTextChange {
        edit: ExactTextEdit {
            from: (from_line, from_byte),
            to: (to_line, to_byte),
            from_line_len: from_text.len(),
            to_line_len: to_text.len(),
            inserted_breaks: insert_parts.len() - 1,
        },
        delta: EditDelta {
            start_line: from_line,
            old_span: old_line_span,
            new_span: new_line_span,
        },
        replacement,
    }
}

/// Apply a prepared edit to the unchanged buffer it was prepared against.
/// Record host metadata first; do not mutate the buffer between these calls.
pub fn apply_text_change_in_place(
    lines: &mut Vec<String>,
    prepared: PreparedTextChange,
) -> EditDelta {
    let from_line = prepared.edit.from.0;
    let to_line = prepared.edit.to.0;
    debug_assert!(
        lines.is_empty()
            || lines.get(from_line).map(String::len) == Some(prepared.edit.from_line_len),
        "text change applied to a buffer that changed after preparation"
    );
    if from_line <= to_line && from_line < lines.len() {
        let end = to_line.min(lines.len().saturating_sub(1));
        lines.splice(from_line..=end, prepared.replacement);
    } else {
        *lines = prepared.replacement;
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    prepared.delta
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(text: &str) -> Vec<String> {
        text.split('\n').map(str::to_owned).collect()
    }

    #[test]
    fn edits_match_flat_text_for_unicode_boundaries_and_multiline_replacements() {
        for original in ["", "é task\nβ task\nend", "a\n\nb\n"] {
            let boundaries: Vec<_> = original
                .char_indices()
                .map(|(i, _)| i)
                .chain(std::iter::once(original.len()))
                .collect();
            for &from in &boundaries {
                for &to in boundaries.iter().filter(|&&to| to >= from) {
                    for insert in ["", "X", "λ\n", "\na\nb"] {
                        let mut buffer = lines(original);
                        let change = TextChange {
                            from,
                            to,
                            insert: insert.into(),
                        };
                        let prepared = prepare_text_change(&buffer, &change, original.len());
                        assert_eq!(buffer.join("\n"), original, "preparation must not mutate");
                        let edit = prepared.edit;
                        assert_eq!(edit.from_line_len, buffer[edit.from.0].len());
                        assert_eq!(edit.to_line_len, buffer[edit.to.0].len());
                        assert_eq!(edit.inserted_breaks, insert.matches('\n').count());
                        let expected_delta = prepared.delta;
                        assert_eq!(
                            apply_text_change_in_place(&mut buffer, prepared),
                            expected_delta
                        );
                        assert_eq!(
                            buffer.join("\n"),
                            format!("{}{insert}{}", &original[..from], &original[to..])
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn exact_edits_distinguish_column_zero_from_mid_line_splits() {
        let buffer = lines("é task");
        for (offset, column) in [(0, 0), (2, 2)] {
            let prepared = prepare_text_change(
                &buffer,
                &TextChange {
                    from: offset,
                    to: offset,
                    insert: "\n".into(),
                },
                document_text_len(&buffer),
            );
            assert_eq!(
                prepared.edit,
                ExactTextEdit {
                    from: (0, column),
                    to: (0, column),
                    from_line_len: 7,
                    to_line_len: 7,
                    inserted_breaks: 1,
                }
            );
            assert_eq!(
                prepared.delta,
                EditDelta {
                    start_line: 0,
                    old_span: 1,
                    new_span: 2
                }
            );
        }
    }

    #[test]
    fn compound_edits_keep_coordinates_relative_to_each_pre_change_buffer() {
        let mut buffer = lines("é task\nβ task\nend");
        let mut edits = Vec::new();
        for offset in [8, 2] {
            let prepared = prepare_text_change(
                &buffer,
                &TextChange {
                    from: offset,
                    to: offset,
                    insert: "\n".into(),
                },
                document_text_len(&buffer),
            );
            edits.push(prepared.edit);
            apply_text_change_in_place(&mut buffer, prepared);
        }
        assert_eq!(edits[0].from, (1, 0));
        assert_eq!(edits[1].from, (0, 2));
        assert_eq!(buffer, lines("é\n task\n\nβ task\nend"));
    }

    #[test]
    fn empty_buffer_and_out_of_range_offsets_preserve_existing_clamping() {
        let mut buffer = Vec::new();
        assert_eq!(document_text_len(&buffer), 0);
        assert_eq!(line_and_byte_for_offset(&buffer, 99), (0, 0));
        let prepared = prepare_text_change(
            &buffer,
            &TextChange {
                from: 99,
                to: 100,
                insert: "x\n".into(),
            },
            0,
        );
        apply_text_change_in_place(&mut buffer, prepared);
        assert_eq!(buffer, lines("x\n"));
        assert_eq!(document_text_len(&buffer), 2);
        assert_eq!(line_and_byte_for_offset(&buffer, 1), (0, 1));
        assert_eq!(line_and_byte_for_offset(&buffer, 2), (1, 0));
        assert_eq!(line_and_byte_for_offset(&buffer, 99), (1, 0));
    }

    #[test]
    fn offset_mapping_retains_inside_replacement_and_descending_change_behavior() {
        let changes = [
            TextChange {
                from: 8,
                to: 12,
                insert: "Z".into(),
            },
            TextChange {
                from: 2,
                to: 2,
                insert: "\n".into(),
            },
        ];
        for (before, after) in [(0, 0), (2, 3), (8, 9), (10, 10), (12, 10), (15, 13)] {
            assert_eq!(map_offset_through_changes(before, &changes), after);
        }
    }
}
