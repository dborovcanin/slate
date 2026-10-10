//! Incremental fold cache upkeep. Host applies the returned view/cache effects.
use super::{edits_require_rebuild, map_ranges_through_line_edits, FoldLineEdit, FoldRange};
#[derive(Debug, Clone, Copy)]
pub struct FoldUpkeepFlags {
    pub reduced_features: bool,
    pub rescan_pending: bool,
    pub has_collapsed: bool,
    pub view_line_count: usize,
}
#[derive(Debug, PartialEq, Eq)]
pub enum FoldUpkeep {
    Disabled,
    PendingVisible { reset_map: bool },
    Recompute,
    Empty,
    Mapped(Vec<FoldRange>),
    Defer { reset_map: bool },
    Unchanged,
}
pub fn line_has_fold_structure(text: &str) -> bool {
    let trimmed = text.trim_start();
    trimmed.starts_with('#')
        || trimmed.starts_with("```")
        || trimmed.starts_with("~~~")
        || (trimmed.starts_with('|') && trimmed.ends_with('|'))
        || crate::markdown_tokens::list_marker_end(text).is_some()
}
fn decide_remap(
    ranges: &[FoldRange],
    edits: &[FoldLineEdit],
    len: usize,
    collapsed: bool,
    touches_structure: bool,
    reset_map: bool,
) -> FoldUpkeep {
    if !edits_require_rebuild(edits) {
        FoldUpkeep::Mapped(map_ranges_through_line_edits(ranges, edits, len.max(1)))
    } else if touches_structure {
        if collapsed {
            FoldUpkeep::Recompute
        } else {
            FoldUpkeep::Defer { reset_map }
        }
    } else {
        FoldUpkeep::Unchanged
    }
}
/// Updates only the affected cache entries; a full rescan is a returned host effect.
pub fn plan_fold_upkeep(
    lines: &[String],
    cursor_line: usize,
    structure: &mut Vec<bool>,
    snapshot: &mut Vec<String>,
    ranges: &[FoldRange],
    flags: FoldUpkeepFlags,
) -> FoldUpkeep {
    if flags.reduced_features {
        return FoldUpkeep::Disabled;
    }
    if flags.rescan_pending {
        return if flags.has_collapsed {
            FoldUpkeep::Recompute
        } else {
            FoldUpkeep::PendingVisible {
                reset_map: flags.view_line_count != lines.len(),
            }
        };
    }
    if lines.is_empty() {
        structure.clear();
        snapshot.clear();
        return FoldUpkeep::Empty;
    }
    let cl = cursor_line.min(lines.len().saturating_sub(1));
    let line_count_changed = lines.len() != structure.len();

    if line_count_changed {
        let next_len = lines.len();
        let prev_len = structure.len();
        // Safety guard: incremental insert/remove remap assumes snapshot and
        // structure vectors are aligned. If a prior mode change/reset left
        // them out of sync, use full recompute instead of risking panic on
        // Vec::insert/remove indexes during Enter/Delete edits.
        if snapshot.len() != prev_len {
            return FoldUpkeep::Recompute;
        }
        let mut remap_edits: Vec<FoldLineEdit> = Vec::new();
        if next_len == prev_len + 1 {
            // One line inserted near cursor.
            let insert_at = cl.min(prev_len);
            let old_line_text = snapshot.get(insert_at).cloned().unwrap_or_default();
            let new_line_text = lines.get(insert_at).cloned().unwrap_or_default();
            remap_edits.push(FoldLineEdit {
                old_start_line: insert_at,
                old_line_span: 1,
                new_line_span: 2,
                old_line_text,
                new_line_text: new_line_text.clone(),
            });

            if cl > 0 {
                if let Some(flag) = structure.get_mut(cl - 1) {
                    *flag = lines
                        .get(cl - 1)
                        .map(|l| line_has_fold_structure(l))
                        .unwrap_or(false);
                }
                if let Some(text) = snapshot.get_mut(cl - 1) {
                    *text = lines.get(cl - 1).cloned().unwrap_or_default();
                }
            }
            let new_flag = lines
                .get(insert_at)
                .map(|l| line_has_fold_structure(l))
                .unwrap_or(false);
            structure.insert(insert_at, new_flag);
            snapshot.insert(insert_at, new_line_text);
        } else if next_len + 1 == prev_len {
            // One line deleted near cursor.
            let remove_at = cl.min(prev_len.saturating_sub(1));
            let old_start_line = remove_at.min(prev_len.saturating_sub(2));
            let old_line_text = snapshot.get(old_start_line).cloned().unwrap_or_default();
            let new_line_text = lines.get(old_start_line).cloned().unwrap_or_default();
            remap_edits.push(FoldLineEdit {
                old_start_line,
                old_line_span: 2,
                new_line_span: 1,
                old_line_text,
                new_line_text: new_line_text.clone(),
            });

            if remove_at < prev_len {
                structure.remove(remove_at);
            }
            if remove_at < snapshot.len() {
                snapshot.remove(remove_at);
            }
            let update_at = remove_at.min(next_len.saturating_sub(1));
            if let Some(flag) = structure.get_mut(update_at) {
                *flag = lines
                    .get(update_at)
                    .map(|l| line_has_fold_structure(l))
                    .unwrap_or(false);
            }
            if let Some(text) = snapshot.get_mut(update_at) {
                *text = lines.get(update_at).cloned().unwrap_or_default();
            }
        } else {
            // Bulk change (paste, format, etc.): rebuild entirely.
            return FoldUpkeep::Recompute;
        }

        return decide_remap(
            ranges,
            &remap_edits,
            lines.len(),
            flags.has_collapsed,
            true,
            true,
        );
    }

    // Same-line edit: check whether the current line touches fold structure.
    let old_text = snapshot.get(cl).cloned().unwrap_or_default();
    let current_text = lines.get(cl).map(|s| s.as_str()).unwrap_or("");
    let new_text = current_text.to_string();
    let next_flag = line_has_fold_structure(current_text);
    let prev_flag = structure.get(cl).copied().unwrap_or(false);

    if next_flag != prev_flag {
        if let Some(flag) = structure.get_mut(cl) {
            *flag = next_flag;
        }
    }
    if let Some(text) = snapshot.get_mut(cl) {
        *text = new_text.clone();
    }

    let remap_edits = [FoldLineEdit {
        old_start_line: cl,
        old_line_span: 1,
        new_line_span: 1,
        old_line_text: old_text,
        new_line_text: new_text,
    }];
    decide_remap(
        ranges,
        &remap_edits,
        lines.len(),
        flags.has_collapsed,
        next_flag || prev_flag,
        false,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    fn flags() -> FoldUpkeepFlags {
        FoldUpkeepFlags {
            reduced_features: false,
            rescan_pending: false,
            has_collapsed: false,
            view_line_count: 2,
        }
    }
    fn plan(old: &[&str], new: &[&str], flags: FoldUpkeepFlags) -> FoldUpkeep {
        let mut structure = old.iter().map(|s| line_has_fold_structure(s)).collect();
        let mut snapshot = old.iter().map(|s| s.to_string()).collect();
        let lines = new.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        plan_fold_upkeep(&lines, 0, &mut structure, &mut snapshot, &[], flags)
    }
    #[test]
    fn pending_rescan_stays_idle_unless_content_is_hidden() {
        let mut f = flags();
        f.rescan_pending = true;
        assert_eq!(
            plan(&["one", "two"], &["one", "two", "three"], f),
            FoldUpkeep::PendingVisible { reset_map: true }
        );
        f.has_collapsed = true;
        assert_eq!(
            plan(&["one", "two"], &["one", "two"], f),
            FoldUpkeep::Recompute
        );
        f.reduced_features = true;
        assert_eq!(
            plan(&["one", "two"], &["one", "two"], f),
            FoldUpkeep::Disabled
        );
    }
    #[test]
    fn structural_same_line_change_defers_only_without_collapsed_ranges() {
        assert_eq!(
            plan(&["# One", "tail"], &["## One", "tail"], flags()),
            FoldUpkeep::Defer { reset_map: false }
        );
        let mut f = flags();
        f.has_collapsed = true;
        assert_eq!(
            plan(&["# One", "tail"], &["## One", "tail"], f),
            FoldUpkeep::Recompute
        );
        assert_eq!(
            plan(&["# One", "tail"], &["# Two", "tail"], f),
            FoldUpkeep::Mapped(vec![])
        );
    }
    #[test]
    fn insert_delete_bulk_and_misaligned_caches() {
        assert_eq!(
            plan(&["one", "two"], &["one", "", "two"], flags()),
            FoldUpkeep::Defer { reset_map: true }
        );
        assert_eq!(
            plan(&["one", "two"], &["one"], flags()),
            FoldUpkeep::Defer { reset_map: true }
        );
        assert_eq!(
            plan(&["one"], &["one", "two", "three"], flags()),
            FoldUpkeep::Recompute
        );
        let mut structure = vec![false];
        let mut snapshot = vec![];
        assert_eq!(
            plan_fold_upkeep(
                &["one".into(), "two".into()],
                0,
                &mut structure,
                &mut snapshot,
                &[],
                flags()
            ),
            FoldUpkeep::Recompute
        );
    }
}
