//! Shared formula result and error presentation.
use std::borrow::Cow;
pub fn table_error_text(
    error_kind: Option<&app_core::calc::TableCellErrorKind>,
    value: &str,
) -> Option<String> {
    match error_kind {
        Some(app_core::calc::TableCellErrorKind::OutOfBounds) => {
            Some(String::from("table error: out_of_bounds"))
        }
        Some(app_core::calc::TableCellErrorKind::NonNumeric) => {
            Some(String::from("table error: non_numeric"))
        }
        Some(app_core::calc::TableCellErrorKind::SelfReference) => {
            Some(String::from("table error: self_reference"))
        }
        Some(app_core::calc::TableCellErrorKind::Cycle) => Some(String::from("table error: cycle")),
        Some(app_core::calc::TableCellErrorKind::Unknown) => {
            if let Some(code) = value.strip_prefix("!ERROR#") {
                Some(format!("table error: {code}"))
            } else {
                Some(String::from("table error: unknown"))
            }
        }
        None => None,
    }
}

pub fn masked_formula_value<'a>(value: &'a str, is_error: bool) -> Cow<'a, str> {
    // Keep table columns aligned while avoiding long `!ERROR#...` payloads
    // that get awkwardly cut inside narrow cells.
    if is_error {
        Cow::Borrowed("!ERROR")
    } else {
        Cow::Borrowed(value)
    }
}

/// Value shown for a formula cell: its formatted result, or `…` while it
/// is still being computed.
pub fn formula_cell_value(eval: Option<&app_core::calc::TableCellEvaluation>) -> String {
    eval.map(|entry| super::table::format_formula_display_value(&entry.value))
        .unwrap_or_else(|| String::from("…"))
}

/// Text a formula cell shows while not focused: its value followed by its
/// `*` marker.
pub fn resting_formula_cell_text(
    eval: Option<&app_core::calc::TableCellEvaluation>,
    marker: &str,
) -> String {
    let value = formula_cell_value(eval);
    let has_error = eval.and_then(|entry| entry.error_kind.as_ref()).is_some();
    let masked = masked_formula_value(&value, has_error);
    let mut out = String::with_capacity(masked.len() + marker.len());
    out.push_str(&masked);
    out.push_str(marker);
    out
}

/// Prepared formula row in Unicode scalar columns. Formula segment byte ranges
/// must address `text`; all display ranges and cursor positions are scalar columns.
#[derive(Clone)]
pub struct PreparedFormulaRow {
    pub text: String,
    pub map: super::mapping::SourceDisplayMap,
    pub dim_ranges: Vec<(usize, usize)>,
    pub resting_marker_cells: Vec<(usize, usize, usize)>,
    pub delta_prefixes: Vec<isize>,
    pub ghost_trailer: String,
    pub mapped_cursor: Option<usize>,
}

type SegmentCoordinates = [usize; 7];
fn coordinates(segment: &super::table::TableFormulaSegment) -> SegmentCoordinates {
    [
        segment.from_byte,
        segment.to_byte,
        segment.from_char,
        segment.to_char,
        segment.cell_from_char,
        segment.cell_to_char,
        segment.cell_index,
    ]
}
struct CachedFormulaRow {
    text: String,
    segments: Vec<SegmentCoordinates>,
    results: Vec<app_core::calc::TableCellEvaluation>,
    cursor: Option<usize>,
    row: PreparedFormulaRow,
}
thread_local! {
    static ROW_CACHE: std::cell::RefCell<std::collections::VecDeque<CachedFormulaRow>> =
        const { std::cell::RefCell::new(std::collections::VecDeque::new()) };
}

/// Cache bounded rows only. Keys include every input used by formula presentation;
/// labels are deliberately excluded because presentation never reads them.
pub fn prepare_formula_row(
    text: &str,
    formula_segments: &[super::table::TableFormulaSegment],
    cell_results: &[app_core::calc::TableCellEvaluation],
    source_cursor: Option<usize>,
) -> PreparedFormulaRow {
    let cacheable = text.len() <= 4096
        && formula_segments.len() <= 32
        && cell_results.len() <= 32
        && cell_results
            .iter()
            .map(|entry| entry.value.len())
            .sum::<usize>()
            <= 4096;
    if cacheable {
        if let Some(row) = ROW_CACHE.with(|cache| {
            cache
                .borrow()
                .iter()
                .find(|entry| {
                    entry.text == text
                        && entry.cursor == source_cursor
                        && entry.results == cell_results
                        && entry.segments.len() == formula_segments.len()
                        && entry
                            .segments
                            .iter()
                            .zip(formula_segments)
                            .all(|(left, right)| *left == coordinates(right))
                })
                .map(|entry| entry.row.clone())
        }) {
            return row;
        }
    }
    let row = prepare_formula_row_uncached(text, formula_segments, cell_results, source_cursor);
    if cacheable {
        ROW_CACHE.with(|cache| {
            let mut cache = cache.borrow_mut();
            if cache.len() == 32 {
                cache.pop_front();
            }
            cache.push_back(CachedFormulaRow {
                text: text.to_owned(),
                segments: formula_segments.iter().map(coordinates).collect(),
                results: cell_results.to_vec(),
                cursor: source_cursor,
                row: row.clone(),
            });
        });
    }
    row
}

fn prepare_formula_row_uncached(
    text: &str,
    formula_segments: &[super::table::TableFormulaSegment],
    cell_results: &[app_core::calc::TableCellEvaluation],
    source_cursor: Option<usize>,
) -> PreparedFormulaRow {
    let mut ghost_dim_ranges = Vec::new();
    let mut resting_formula_markers = Vec::new();
    let mut formula_segment_char_delta_prefix = Vec::new();
    let mut cell_result_by_index: Vec<Option<&app_core::calc::TableCellEvaluation>> = Vec::new();
    for entry in cell_results {
        if entry.cell_index >= cell_result_by_index.len() {
            cell_result_by_index.resize(entry.cell_index + 1, None);
        }
        cell_result_by_index[entry.cell_index] = Some(entry);
    }
    let value_for_cell = |cell_index: usize| {
        cell_result_by_index
            .get(cell_index)
            .and_then(|entry| *entry)
    };

    let mut replacements: Vec<(std::ops::Range<usize>, String)> = Vec::new();
    let mut char_delta: isize = 0;
    let mut trailer_parts: Vec<String> = Vec::new();

    formula_segment_char_delta_prefix.push(0);
    // Char position of the cursor in the rendered line; we
    // collect this only when the cursor sits inside a
    // focused (un-masked) formula cell.
    let mut focused_cursor_col: Option<usize> = None;

    for (fi, seg) in formula_segments.iter().enumerate() {
        let marker = super::table::formula_marker_token(fi);
        let eval = value_for_cell(seg.cell_index);
        let value = formula_cell_value(eval);
        let has_error = eval.and_then(|entry| entry.error_kind.as_ref()).is_some();
        let source_text = text[seg.from_byte..seg.to_byte].trim().to_string();

        let is_focused = source_cursor.is_some()
            && source_cursor.unwrap_or(0) >= seg.cell_from_char
            && source_cursor.unwrap_or(0) < seg.cell_to_char;

        // Ghost trailer: focused cell shows the value
        // (so the user can see the result while editing),
        // resting cells show the formula source.
        let trailer_text = if let Some(err) = table_error_text(
            eval.and_then(|entry| entry.error_kind.as_ref()),
            eval.map(|entry| entry.value.as_str()).unwrap_or(""),
        ) {
            err
        } else if is_focused && !has_error {
            value.clone()
        } else {
            source_text
        };
        if !trailer_text.is_empty() {
            trailer_parts.push(format!("{marker} ➜ {trailer_text}"));
        }

        if is_focused {
            let mapped = (source_cursor.unwrap_or(0) as isize + char_delta).max(0) as usize;
            focused_cursor_col = Some(mapped);
            formula_segment_char_delta_prefix.push(char_delta);
        } else {
            let old_chars = seg.to_char.saturating_sub(seg.from_char);
            let replacement = resting_formula_cell_text(eval, &marker);
            let rendered_chars = replacement.chars().count();
            let value_chars = rendered_chars - marker.len();
            let marker_char = ((seg.from_char as isize) + char_delta) as usize + value_chars;
            ghost_dim_ranges.push((marker_char, marker_char + marker.len()));
            resting_formula_markers.push((seg.cell_index, value_chars, marker.len()));
            char_delta += rendered_chars as isize - old_chars as isize;
            formula_segment_char_delta_prefix.push(char_delta);
            replacements.push((seg.from_char..seg.to_char, replacement));
        }
    }
    let replacements: Vec<_> = replacements
        .iter()
        .map(|(source, text)| super::transform::Replacement {
            source: source.clone(),
            text,
        })
        .collect();
    let mapped = super::transform::substitute(text, &replacements);

    let mapped_cursor = source_cursor.map(|cursor| {
        focused_cursor_col.unwrap_or_else(|| {
            let count = formula_segments
                .iter()
                .take_while(|seg| seg.cell_to_char <= cursor)
                .count();
            let delta = formula_segment_char_delta_prefix
                .get(count)
                .copied()
                .unwrap_or(0);
            (cursor as isize + delta).max(0) as usize
        })
    });
    PreparedFormulaRow {
        text: mapped.text,
        map: mapped.map,
        dim_ranges: ghost_dim_ranges,
        resting_marker_cells: resting_formula_markers,
        delta_prefixes: formula_segment_char_delta_prefix,
        ghost_trailer: trailer_parts.join("  "),
        mapped_cursor,
    }
}

#[cfg(test)]
mod tests {
    use super::super::{mapping::Provenance, table::find_table_formula_segments};
    use super::*;
    use app_core::calc::{TableCellErrorKind, TableCellEvaluation};

    #[test]
    fn unicode_multiple_cells_focus_and_source_provenance() {
        let text = "| λ | :=1+2 | :=4+5 |";
        let segments = find_table_formula_segments(text);
        assert_eq!(segments.len(), 2);
        let results = [
            TableCellEvaluation {
                cell_index: 1,
                value: "3".into(),
                error_kind: None,
            },
            TableCellEvaluation {
                cell_index: 2,
                value: "9".into(),
                error_kind: None,
            },
        ];
        let resting = prepare_formula_row(text, &segments, &results, None);
        assert!(resting.text.contains("3*"));
        assert!(resting.text.contains("9**"));
        assert_eq!(resting.dim_ranges.len(), 2);
        assert_eq!(resting.delta_prefixes.len(), 3);
        assert!(resting.ghost_trailer.contains(":=1+2"));
        let cursor = segments[1].from_char + 2;
        let focused = prepare_formula_row(text, &segments, &results, Some(cursor));
        assert!(focused.text.contains(":=4+5"));
        assert!(focused.ghost_trailer.contains("** ➜ 9"));
        assert_eq!(
            focused.mapped_cursor,
            Some((cursor as isize + focused.delta_prefixes[1]) as usize)
        );
        assert_eq!(focused.resting_marker_cells.len(), 1);
        let source: Vec<_> = text.chars().collect();
        let display: Vec<_> = focused.text.chars().collect();
        for segment in &focused.map.segments {
            if let Provenance::Copied(range) = &segment.provenance {
                assert_eq!(source[range.clone()], display[segment.display.clone()]);
            }
        }
        let after = prepare_formula_row(text, &segments, &results, Some(text.chars().count()));
        assert_eq!(after.mapped_cursor, Some(after.text.chars().count()));
    }

    #[test]
    fn error_trailer_and_pending_value_preserve_policy() {
        let text = "| :=1/0 | :=2 |";
        let segments = find_table_formula_segments(text);
        let results = [TableCellEvaluation {
            cell_index: 0,
            value: "!ERROR#cycle".into(),
            error_kind: Some(TableCellErrorKind::Cycle),
        }];
        let row = prepare_formula_row(text, &segments, &results, None);
        assert!(row.text.contains("!ERROR*"));
        assert!(row.text.contains("…**"));
        assert!(row.ghost_trailer.contains("* ➜ table error: cycle"));
        let focused = prepare_formula_row(text, &segments, &results, Some(segments[0].from_char));
        assert!(focused.text.contains(":=1/0"));
        assert!(focused.ghost_trailer.contains("table error: cycle"));
        assert_eq!(focused.map.source_len, text.chars().count());
    }
}
