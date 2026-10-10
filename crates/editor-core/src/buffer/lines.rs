//! Whole-line edits. Plans borrow insertion text or consume an owned vector;
//! application shifts the buffer once, without repeated insertion/cloning.
use super::primitives::BufferCursor;
use super::EditDelta;
use std::borrow::Cow;

pub struct LineInsertPlan<'a> {
    pub delta: EditDelta,
    pub cursor: BufferCursor,
    /// Relative fates of old lines within the replaced block (empty here).
    pub fates: Vec<Option<usize>>,
    inserted: Cow<'a, [String]>,
    old_len: usize,
}

pub struct LineRemovePlan {
    pub delta: EditDelta,
    pub cursor: BufferCursor,
    /// Relative fates of removed lines; even an empty surviving buffer slot
    /// does not inherit a removed line's reminder.
    pub fates: Vec<Option<usize>>,
    end: usize,
    old_len: usize,
}

/// Insert before `at`, clamping beyond the buffer to its end. An empty
/// insertion is a no-op. The resulting cursor selects the first new line.
pub fn prepare_insert_lines<'a>(
    lines: &[String],
    at: usize,
    new_lines: impl Into<Cow<'a, [String]>>,
) -> Option<LineInsertPlan<'a>> {
    let inserted = new_lines.into();
    if inserted.is_empty() {
        return None;
    }
    let start = at.min(lines.len());
    Some(LineInsertPlan {
        delta: EditDelta {
            start_line: start,
            old_span: 0,
            new_span: inserted.len(),
        },
        cursor: BufferCursor {
            line: start,
            column: 0,
        },
        fates: Vec::new(),
        inserted,
        old_len: lines.len(),
    })
}

/// Apply against the unchanged buffer used during preparation. Owned inserted
/// lines move directly into the splice; borrowed lines clone once each.
pub fn apply_insert_lines(lines: &mut Vec<String>, plan: LineInsertPlan<'_>) -> BufferCursor {
    debug_assert_eq!(lines.len(), plan.old_len);
    let at = plan.delta.start_line;
    lines.splice(at..at, plan.inserted.into_owned());
    plan.cursor
}

/// Remove the half-open range `start..end`, clamping its endpoints. Empty or
/// reversed ranges are no-ops. Removing all lines leaves one empty line.
pub fn prepare_remove_lines(lines: &[String], start: usize, end: usize) -> Option<LineRemovePlan> {
    let start = start.min(lines.len());
    let end = end.min(lines.len());
    if end <= start {
        return None;
    }
    let remaining = lines.len() - (end - start);
    Some(LineRemovePlan {
        delta: EditDelta {
            start_line: start,
            old_span: end - start,
            new_span: usize::from(remaining == 0),
        },
        cursor: BufferCursor {
            line: start.min(remaining.saturating_sub(1)),
            column: 0,
        },
        fates: vec![None; end - start],
        end,
        old_len: lines.len(),
    })
}

pub fn apply_remove_lines(lines: &mut Vec<String>, plan: LineRemovePlan) -> BufferCursor {
    debug_assert_eq!(lines.len(), plan.old_len);
    let replacement = (plan.delta.new_span == 1).then(String::new);
    lines.splice(plan.delta.start_line..plan.end, replacement);
    plan.cursor
}

#[cfg(test)]
mod tests {
    use super::*;
    fn buffer(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn borrowed_unicode_insertion_clamps_and_preserves_source() {
        let mut lines = buffer(&["a"]);
        let insertion = buffer(&["λ", "é"]);
        let plan = prepare_insert_lines(&lines, 99, insertion.as_slice()).unwrap();
        assert_eq!(
            plan.delta,
            EditDelta {
                start_line: 1,
                old_span: 0,
                new_span: 2
            }
        );
        assert!(plan.fates.is_empty());
        assert_eq!(
            apply_insert_lines(&mut lines, plan),
            BufferCursor { line: 1, column: 0 }
        );
        assert_eq!(lines, buffer(&["a", "λ", "é"]));
        assert_eq!(insertion, buffer(&["λ", "é"]));
    }

    #[test]
    fn owned_insertion_moves_allocations_and_handles_empty_buffer() {
        let insertion = buffer(&["first", "last"]);
        let ptr = insertion[0].as_ptr();
        let mut lines = Vec::new();
        let plan = prepare_insert_lines(&lines, 0, insertion).unwrap();
        apply_insert_lines(&mut lines, plan);
        assert_eq!(lines[0].as_ptr(), ptr);
        assert_eq!(lines, buffer(&["first", "last"]));
    }

    #[test]
    fn removal_range_edges_and_cursor() {
        for (start, end, expected, cursor) in [
            (0, 1, vec!["b", "c"], 0),
            (1, 2, vec!["a", "c"], 1),
            (2, 99, vec!["a", "b"], 1),
        ] {
            let mut lines = buffer(&["a", "b", "c"]);
            let plan = prepare_remove_lines(&lines, start, end).unwrap();
            assert_eq!(plan.fates, vec![None]);
            assert_eq!(
                apply_remove_lines(&mut lines, plan),
                BufferCursor {
                    line: cursor,
                    column: 0
                }
            );
            assert_eq!(lines, buffer(&expected));
        }
    }

    #[test]
    fn removal_all_keeps_empty_line_without_inheriting_reminders() {
        let mut lines = buffer(&["λ", "b"]);
        let plan = prepare_remove_lines(&lines, 0, 99).unwrap();
        assert_eq!(
            plan.delta,
            EditDelta {
                start_line: 0,
                old_span: 2,
                new_span: 1
            }
        );
        assert_eq!(plan.fates, vec![None, None]);
        assert_eq!(
            apply_remove_lines(&mut lines, plan),
            BufferCursor { line: 0, column: 0 }
        );
        assert_eq!(lines, buffer(&[""]));
    }

    #[test]
    fn noops() {
        let lines = buffer(&["a"]);
        assert!(prepare_insert_lines(&lines, 0, Vec::new()).is_none());
        for (start, end) in [(0, 0), (1, 0), (2, 99)] {
            assert!(prepare_remove_lines(&lines, start, end).is_none());
        }
        assert!(prepare_remove_lines(&[], 0, 1).is_none());
    }
}
