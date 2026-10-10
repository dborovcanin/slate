//! Post-edit calc planning: which upkeep an edit needs, decided from cached
//! signals and line metadata. Scheduling and evaluation stay in the host.
use super::{
    contains_builtin_formula_with_mask, contains_variable_assignment_with_mask, has_calc_signal,
    is_table_line, line_for_calc_evaluation_with_mask, CalcFeatureMask, CalcSignalFlags,
    LineMetadata,
};

/// Host-independent choice for calc upkeep after an edit. Scheduling and evaluation
/// are host effects; this planner reads cached signals and never scans the note.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CalcAfterEdit {
    Skip,
    Defer,
    ViewportRefresh,
    RemapOrRecompute,
    ScheduleIdle,
    RecomputeRange,
}
#[derive(Debug, Clone, Copy)]
pub struct AfterEditFlags {
    pub line_count: usize,
    pub previous_line_count: usize,
    pub signals: CalcSignalFlags,
    pub stale: bool,
    pub viewport_only: bool,
    pub async_min_lines: usize,
    pub defer_min_lines: usize,
}
pub fn can_skip_after_edit(signals: CalcSignalFlags, stale: bool) -> bool {
    !signals.has_builtin_formula
        && !signals.has_variable_assignment
        && !signals.has_expression
        && !stale
}
pub fn should_defer_after_edit(
    line_count: usize,
    signals: CalcSignalFlags,
    defer_min_lines: usize,
) -> bool {
    line_count >= defer_min_lines
        && !signals.has_builtin_formula
        && !signals.has_variable_assignment
}
pub fn plan_after_edit(flags: AfterEditFlags) -> CalcAfterEdit {
    if can_skip_after_edit(flags.signals, flags.stale) {
        CalcAfterEdit::Skip
    } else if should_defer_after_edit(flags.line_count, flags.signals, flags.defer_min_lines) {
        CalcAfterEdit::Defer
    } else if flags.line_count < flags.async_min_lines {
        CalcAfterEdit::RecomputeRange
    } else if flags.line_count != flags.previous_line_count {
        if flags.viewport_only {
            CalcAfterEdit::ViewportRefresh
        } else {
            CalcAfterEdit::RemapOrRecompute
        }
    } else {
        CalcAfterEdit::ScheduleIdle
    }
}

pub fn lines_affect_calc(lines: &[String], mask: CalcFeatureMask) -> bool {
    (mask.table_enabled && lines.iter().any(|line| is_table_line(line)))
        || contains_variable_assignment_with_mask(lines, mask)
        || contains_builtin_formula_with_mask(lines, mask)
        || lines.iter().any(|line| {
            let eval_target = line_for_calc_evaluation_with_mask(line, mask);
            let trimmed = eval_target.trim();
            !trimmed.is_empty() && has_calc_signal(trimmed)
        })
}

/// Unchanged prefix/suffix whose cached results can survive a structural edit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CalcResultRemap {
    pub prefix: usize,
    pub suffix: usize,
}
pub fn plan_result_remap(
    previous: &[LineMetadata],
    next: &[LineMetadata],
    lines: &[String],
    mask: CalcFeatureMask,
) -> Option<CalcResultRemap> {
    if previous.is_empty() || next.len() != lines.len() {
        return None;
    }
    let prefix = previous
        .iter()
        .zip(next)
        .take_while(|(a, b)| a.hash == b.hash)
        .count();
    let suffix = previous[prefix..]
        .iter()
        .rev()
        .zip(next[prefix..].iter().rev())
        .take_while(|(a, b)| a.hash == b.hash)
        .count();
    let removed = &previous[prefix..previous.len() - suffix];
    if removed
        .iter()
        .any(|m| m.has_assignment || m.has_builtin_formula)
        || lines_affect_calc(&lines[prefix..lines.len() - suffix], mask)
    {
        return None;
    }
    Some(CalcResultRemap { prefix, suffix })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::calc_plan::line_metadata_for_lines_with_mask;
    fn flags() -> AfterEditFlags {
        AfterEditFlags {
            line_count: 2000,
            previous_line_count: 2000,
            signals: CalcSignalFlags {
                has_variable_assignment: true,
                ..Default::default()
            },
            stale: false,
            viewport_only: false,
            async_min_lines: 2000,
            defer_min_lines: 20000,
        }
    }
    #[test]
    fn chooses_every_calc_path_and_preserves_priority() {
        let mut f = flags();
        assert_eq!(plan_after_edit(f), CalcAfterEdit::ScheduleIdle);
        f.line_count += 1;
        assert_eq!(plan_after_edit(f), CalcAfterEdit::RemapOrRecompute);
        f.viewport_only = true;
        assert_eq!(plan_after_edit(f), CalcAfterEdit::ViewportRefresh);
        f.line_count = 1999;
        assert_eq!(plan_after_edit(f), CalcAfterEdit::RecomputeRange);
        f.line_count = 20000;
        f.signals = CalcSignalFlags {
            has_expression: true,
            ..Default::default()
        };
        assert_eq!(plan_after_edit(f), CalcAfterEdit::Defer);
        f.signals = CalcSignalFlags::default();
        assert_eq!(plan_after_edit(f), CalcAfterEdit::Skip);
        f.stale = true;
        assert_eq!(plan_after_edit(f), CalcAfterEdit::Defer);
        f.signals.has_builtin_formula = true;
        assert_eq!(plan_after_edit(f), CalcAfterEdit::ViewportRefresh);
    }
    #[test]
    fn stale_small_notes_cannot_skip() {
        let mut f = flags();
        f.line_count = 1;
        f.signals = CalcSignalFlags::default();
        f.stale = true;
        assert_eq!(plan_after_edit(f), CalcAfterEdit::RecomputeRange);
    }

    #[test]
    fn prose_insertion_keeps_formula_suffix_but_formula_changes_recompute() {
        let mask = CalcFeatureMask::default();
        let before = vec!["title".into(), "2 + 2".into()];
        let next = vec!["title".into(), "prose".into(), "2 + 2".into()];
        let old = line_metadata_for_lines_with_mask(&before, mask);
        let new = line_metadata_for_lines_with_mask(&next, mask);
        assert_eq!(
            plan_result_remap(&old, &new, &next, mask),
            Some(CalcResultRemap {
                prefix: 1,
                suffix: 1
            })
        );
        let changed = vec!["title".into(), "x := 4".into(), "2 + 2".into()];
        let new = line_metadata_for_lines_with_mask(&changed, mask);
        assert_eq!(plan_result_remap(&old, &new, &changed, mask), None);
    }
}
