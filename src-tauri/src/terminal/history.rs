#[derive(Debug, Clone, Copy, Default)]
pub struct HistoryCursor {
    pub line: usize,
    pub col: usize,
}

#[derive(Debug, Clone)]
struct HistorySnapshot {
    lines: Vec<String>,
    cursor: HistoryCursor,
}

#[derive(Debug, Clone)]
struct HistoryEntry {
    start_line: usize,
    removed_lines: Vec<String>,
    inserted_lines: Vec<String>,
    cursor_before: HistoryCursor,
    cursor_after: HistoryCursor,
}

#[derive(Debug, Clone)]
pub struct LineHistory {
    entries: Vec<HistoryEntry>,
    pos: usize,
    max_entries: usize,
    snapshot: HistorySnapshot,
    coalesce_anchor: Option<HistorySnapshot>,
}

pub(crate) const COALESCE_ANCHOR_MAX_LINES: usize = 5_000;

impl LineHistory {
    pub fn new(
        max_entries: usize,
        lines: &[String],
        cursor_line: usize,
        cursor_col: usize,
    ) -> Self {
        let disabled = max_entries == 0;
        Self {
            entries: Vec::new(),
            pos: 0,
            max_entries,
            snapshot: HistorySnapshot {
                lines: if disabled { Vec::new() } else { lines.to_vec() },
                cursor: HistoryCursor {
                    line: cursor_line,
                    col: cursor_col,
                },
            },
            coalesce_anchor: None,
        }
    }

    fn disabled(&self) -> bool {
        self.max_entries == 0
    }

    #[cfg(test)]
    pub fn reset(&mut self, lines: &[String], cursor_line: usize, cursor_col: usize) {
        self.entries.clear();
        self.pos = 0;
        self.snapshot = HistorySnapshot {
            lines: if self.disabled() {
                Vec::new()
            } else {
                lines.to_vec()
            },
            cursor: HistoryCursor {
                line: cursor_line,
                col: cursor_col,
            },
        };
        self.coalesce_anchor = None;
    }

    pub fn checkpoint(&mut self, lines: &[String], cursor_line: usize, cursor_col: usize) {
        if self.disabled() {
            self.snapshot.lines.clear();
            self.snapshot.cursor = HistoryCursor {
                line: cursor_line,
                col: cursor_col,
            };
            self.coalesce_anchor = None;
            return;
        }
        self.snapshot = HistorySnapshot {
            lines: lines.to_vec(),
            cursor: HistoryCursor {
                line: cursor_line,
                col: cursor_col,
            },
        };
        self.coalesce_anchor = None;
    }

    pub fn record_edit(
        &mut self,
        lines: &[String],
        cursor_line: usize,
        cursor_col: usize,
        coalesce: bool,
    ) -> bool {
        if self.disabled() {
            self.snapshot.cursor = HistoryCursor {
                line: cursor_line,
                col: cursor_col,
            };
            self.coalesce_anchor = None;
            return false;
        }
        if coalesce && self.pos == self.entries.len() {
            if let Some(anchor) = self.coalesce_anchor.as_ref() {
                let merged = build_history_entry(anchor, lines, cursor_line, cursor_col);
                let snapshot_delta =
                    build_history_entry(&self.snapshot, lines, cursor_line, cursor_col);
                match merged {
                    Some(entry) => {
                        if let Some(last) = self.entries.last_mut() {
                            *last = entry;
                            self.apply_snapshot_delta(snapshot_delta, cursor_line, cursor_col);
                            return true;
                        }
                    }
                    None => {
                        if !self.entries.is_empty() {
                            self.entries.pop();
                            self.pos = self.entries.len();
                        }
                        self.coalesce_anchor = None;
                        self.apply_snapshot_delta(snapshot_delta, cursor_line, cursor_col);
                        return false;
                    }
                }
            }
        }

        let Some(entry) = build_history_entry(&self.snapshot, lines, cursor_line, cursor_col)
        else {
            self.snapshot = HistorySnapshot {
                lines: lines.to_vec(),
                cursor: HistoryCursor {
                    line: cursor_line,
                    col: cursor_col,
                },
            };
            self.coalesce_anchor = None;
            return false;
        };

        if self.pos < self.entries.len() {
            self.entries.truncate(self.pos);
        }

        let anchor = if self.snapshot.lines.len() <= COALESCE_ANCHOR_MAX_LINES {
            Some(self.snapshot.clone())
        } else {
            None
        };
        let snapshot_delta = entry.clone();
        self.entries.push(entry);
        self.pos = self.entries.len();

        if self.entries.len() > self.max_entries {
            self.entries.remove(0);
            self.pos = self.pos.saturating_sub(1);
        }

        self.apply_snapshot_delta(Some(snapshot_delta), cursor_line, cursor_col);
        self.coalesce_anchor = anchor;
        true
    }

    pub fn record_edit_span(
        &mut self,
        lines: &[String],
        cursor_line: usize,
        cursor_col: usize,
        start_line: usize,
        old_line_span: usize,
        new_line_span: usize,
    ) -> bool {
        if self.disabled() {
            self.snapshot.cursor = HistoryCursor {
                line: cursor_line,
                col: cursor_col,
            };
            self.coalesce_anchor = None;
            return false;
        }
        let before_len = self.snapshot.lines.len();
        let after_len = lines.len();
        let start = start_line.min(before_len).min(after_len);
        let old_end = start.saturating_add(old_line_span).min(before_len);
        let new_end = start.saturating_add(new_line_span).min(after_len);
        let removed_lines = self.snapshot.lines[start..old_end].to_vec();
        let inserted_lines = lines[start..new_end].to_vec();

        let cursor_after = HistoryCursor {
            line: cursor_line,
            col: cursor_col,
        };
        if removed_lines.is_empty()
            && inserted_lines.is_empty()
            && self.snapshot.cursor.line == cursor_after.line
            && self.snapshot.cursor.col == cursor_after.col
        {
            self.snapshot.cursor = cursor_after;
            self.coalesce_anchor = None;
            return false;
        }

        if self.pos < self.entries.len() {
            self.entries.truncate(self.pos);
        }

        let entry = HistoryEntry {
            start_line: start,
            removed_lines,
            inserted_lines,
            cursor_before: self.snapshot.cursor,
            cursor_after,
        };
        let snapshot_delta = entry.clone();
        self.entries.push(entry);
        self.pos = self.entries.len();

        if self.entries.len() > self.max_entries {
            self.entries.remove(0);
            self.pos = self.pos.saturating_sub(1);
        }

        self.apply_snapshot_delta(Some(snapshot_delta), cursor_line, cursor_col);
        self.coalesce_anchor = None;
        true
    }

    fn apply_snapshot_delta(
        &mut self,
        delta: Option<HistoryEntry>,
        cursor_line: usize,
        cursor_col: usize,
    ) {
        if let Some(entry) = delta {
            apply_line_replace(
                &mut self.snapshot.lines,
                entry.start_line,
                entry.removed_lines.len(),
                &entry.inserted_lines,
            );
        }
        self.snapshot.cursor = HistoryCursor {
            line: cursor_line,
            col: cursor_col,
        };
    }

    pub fn undo(&mut self, lines: &mut Vec<String>) -> Option<HistoryCursor> {
        if self.disabled() {
            return None;
        }
        if self.pos == 0 {
            return None;
        }

        self.pos -= 1;
        let entry = &self.entries[self.pos];
        apply_line_replace(
            lines,
            entry.start_line,
            entry.inserted_lines.len(),
            &entry.removed_lines,
        );
        let cursor = entry.cursor_before;
        self.snapshot = HistorySnapshot {
            lines: lines.clone(),
            cursor,
        };
        self.coalesce_anchor = None;
        Some(cursor)
    }

    pub fn redo(&mut self, lines: &mut Vec<String>) -> Option<HistoryCursor> {
        if self.disabled() {
            return None;
        }
        if self.pos >= self.entries.len() {
            return None;
        }

        let entry = &self.entries[self.pos];
        apply_line_replace(
            lines,
            entry.start_line,
            entry.removed_lines.len(),
            &entry.inserted_lines,
        );
        self.pos += 1;
        let cursor = entry.cursor_after;
        self.snapshot = HistorySnapshot {
            lines: lines.clone(),
            cursor,
        };
        self.coalesce_anchor = None;
        Some(cursor)
    }

    pub fn undo_depth(&self) -> usize {
        self.pos
    }

    pub fn redo_depth(&self) -> usize {
        self.entries.len().saturating_sub(self.pos)
    }

    pub fn compact(&mut self) {
        self.entries.shrink_to_fit();
        self.snapshot.lines.shrink_to_fit();
        if let Some(anchor) = self.coalesce_anchor.as_mut() {
            anchor.lines.shrink_to_fit();
        }
    }
}

fn build_history_entry(
    before: &HistorySnapshot,
    after_lines: &[String],
    after_cursor_line: usize,
    after_cursor_col: usize,
) -> Option<HistoryEntry> {
    let before_lines = &before.lines;
    let prefix = shared_prefix_lines_len(before_lines, after_lines);
    let suffix = shared_suffix_lines_len(before_lines, after_lines, prefix);

    let before_changed_to = before_lines.len().saturating_sub(suffix);
    let after_changed_to = after_lines.len().saturating_sub(suffix);

    let removed_lines = before_lines[prefix..before_changed_to].to_vec();
    let inserted_lines = after_lines[prefix..after_changed_to].to_vec();

    let cursor_after = HistoryCursor {
        line: after_cursor_line,
        col: after_cursor_col,
    };

    if removed_lines.is_empty()
        && inserted_lines.is_empty()
        && before.cursor.line == cursor_after.line
        && before.cursor.col == cursor_after.col
    {
        return None;
    }

    Some(HistoryEntry {
        start_line: prefix,
        removed_lines,
        inserted_lines,
        cursor_before: before.cursor,
        cursor_after,
    })
}

fn shared_prefix_lines_len(a: &[String], b: &[String]) -> usize {
    let max = a.len().min(b.len());
    let mut i = 0usize;
    while i < max && a[i] == b[i] {
        i += 1;
    }
    i
}

fn shared_suffix_lines_len(a: &[String], b: &[String], prefix_len: usize) -> usize {
    let max = a.len().min(b.len()).saturating_sub(prefix_len);
    let mut i = 0usize;
    while i < max && a[a.len() - 1 - i] == b[b.len() - 1 - i] {
        i += 1;
    }
    i
}

fn apply_line_replace(
    lines: &mut Vec<String>,
    start: usize,
    remove_len: usize,
    inserted: &[String],
) {
    let range_start = start.min(lines.len());
    let range_end = start.saturating_add(remove_len).min(lines.len());
    lines.splice(range_start..range_end, inserted.iter().cloned());
    if lines.is_empty() {
        lines.push(String::new());
    }
}

#[cfg(test)]
mod tests {
    use super::{HistoryCursor, LineHistory};

    #[test]
    fn history_roundtrip_replaces_line_range() {
        let start = vec!["one".to_string(), "two".to_string(), "three".to_string()];
        let mut history = LineHistory::new(8, &start, 1, 0);

        let mut edited = vec!["one".to_string(), "THREE".to_string()];
        assert!(history.record_edit(&edited, 1, 3, false));
        assert_eq!(history.undo_depth(), 1);

        let undo_cursor = history.undo(&mut edited).expect("undo");
        assert_eq!(undo_cursor.line, 1);
        assert_eq!(undo_cursor.col, 0);
        assert_eq!(edited, start);

        let redo_cursor = history.redo(&mut edited).expect("redo");
        assert_eq!(redo_cursor.line, 1);
        assert_eq!(redo_cursor.col, 3);
        assert_eq!(edited, vec!["one".to_string(), "THREE".to_string()]);
    }

    #[test]
    fn history_record_edit_span_roundtrip_replaces_known_span() {
        let start = vec![
            "one".to_string(),
            "two".to_string(),
            "three".to_string(),
            "four".to_string(),
        ];
        let mut history = LineHistory::new(8, &start, 1, 0);

        let mut edited = vec![
            "one".to_string(),
            "TWO".to_string(),
            "THREE".to_string(),
            "four".to_string(),
        ];
        assert!(history.record_edit_span(&edited, 2, 5, 1, 2, 2));
        assert_eq!(history.undo_depth(), 1);

        let undo_cursor = history.undo(&mut edited).expect("undo");
        assert_eq!(undo_cursor.line, 1);
        assert_eq!(undo_cursor.col, 0);
        assert_eq!(edited, start);

        let redo_cursor = history.redo(&mut edited).expect("redo");
        assert_eq!(redo_cursor.line, 2);
        assert_eq!(redo_cursor.col, 5);
        assert_eq!(
            edited,
            vec![
                "one".to_string(),
                "TWO".to_string(),
                "THREE".to_string(),
                "four".to_string(),
            ]
        );
    }

    #[test]
    fn history_coalesces_multiple_edits_into_one_undo_step() {
        let start = vec!["a".to_string()];
        let mut history = LineHistory::new(8, &start, 0, 1);

        let mut lines = vec!["ab".to_string()];
        assert!(history.record_edit(&lines, 0, 2, false));
        lines = vec!["abc".to_string()];
        assert!(history.record_edit(&lines, 0, 3, true));
        lines = vec!["abcd".to_string()];
        assert!(history.record_edit(&lines, 0, 4, true));

        assert_eq!(history.undo_depth(), 1);
        assert_eq!(history.redo_depth(), 0);

        let undo_cursor = history.undo(&mut lines).expect("undo");
        assert_eq!(undo_cursor.line, 0);
        assert_eq!(undo_cursor.col, 1);
        assert_eq!(lines, start);

        let redo_cursor = history.redo(&mut lines).expect("redo");
        assert_eq!(redo_cursor.line, 0);
        assert_eq!(redo_cursor.col, 4);
        assert_eq!(lines, vec!["abcd".to_string()]);
    }

    #[test]
    fn history_large_docs_avoid_coalesce_anchor_clone() {
        let mut start = vec!["plain".to_string(); super::COALESCE_ANCHOR_MAX_LINES + 1];
        let mut history = LineHistory::new(8, &start, 0, 0);

        start[100] = "first".to_string();
        assert!(history.record_edit(&start, 100, 5, false));
        start[100] = "second".to_string();
        assert!(history.record_edit(&start, 100, 6, true));

        assert_eq!(history.undo_depth(), 2);
        let cursor = history.undo(&mut start).expect("undo");
        assert_eq!(cursor.line, 100);
        assert_eq!(start[100], "first");
    }

    #[test]
    fn history_record_edit_span_roundtrips_just_above_coalesce_cap() {
        // Mirrors the doc size at which `prefer_span_history_fast_path` now routes
        // edits through `record_edit_span`. The generic path does not coalesce here
        // (see `history_large_docs_avoid_coalesce_anchor_clone`), so the span path is
        // behavior-equivalent: one entry per edit, exact undo/redo.
        let mut lines = vec!["plain".to_string(); super::COALESCE_ANCHOR_MAX_LINES + 1];
        let mut history = LineHistory::new(8, &lines, 0, 0);

        lines[4000] = "edited".to_string();
        assert!(history.record_edit_span(&lines, 4000, 6, 4000, 1, 1));
        lines[4000] = "edited again".to_string();
        assert!(history.record_edit_span(&lines, 4000, 12, 4000, 1, 1));

        assert_eq!(history.undo_depth(), 2);

        let cursor = history.undo(&mut lines).expect("undo 1");
        assert_eq!(cursor.line, 4000);
        assert_eq!(lines[4000], "edited");
        history.undo(&mut lines).expect("undo 2");
        assert_eq!(lines[4000], "plain");
        history.redo(&mut lines).expect("redo");
        assert_eq!(lines[4000], "edited");
    }

    #[test]
    fn history_truncates_redo_tail_after_new_edit() {
        let start = vec!["a".to_string()];
        let mut history = LineHistory::new(8, &start, 0, 0);
        let mut lines = vec!["ab".to_string()];
        assert!(history.record_edit(&lines, 0, 2, false));
        lines = vec!["abc".to_string()];
        assert!(history.record_edit(&lines, 0, 3, false));

        history.undo(&mut lines).expect("undo 1");
        assert_eq!(lines, vec!["ab".to_string()]);

        lines = vec!["ab!".to_string()];
        assert!(history.record_edit(&lines, 0, 3, false));

        assert!(history.redo(&mut lines).is_none());
        assert_eq!(history.redo_depth(), 0);
    }

    #[test]
    fn history_reset_clears_entries() {
        let start = vec!["x".to_string()];
        let mut history = LineHistory::new(8, &start, 0, 0);
        let mut lines = vec!["xy".to_string()];
        history.record_edit(&lines, 0, 2, false);

        history.reset(&lines, 0, 2);
        assert_eq!(history.undo_depth(), 0);
        assert!(history.undo(&mut lines).is_none());
    }

    #[test]
    fn history_record_edit_ignores_noop() {
        let start = vec!["noop".to_string()];
        let mut history = LineHistory::new(8, &start, 0, 0);
        assert!(!history.record_edit(&start, 0, 0, false));
        assert_eq!(history.undo_depth(), 0);
    }

    #[test]
    fn history_undo_reapplies_cursor_positions() {
        let start = vec!["aa".to_string()];
        let mut history = LineHistory::new(8, &start, 0, 0);
        let mut lines = vec!["aab".to_string()];
        history.record_edit(&lines, 0, 3, false);
        let cursor = history.undo(&mut lines).expect("undo");
        assert_eq!(cursor.line, 0);
        assert_eq!(cursor.col, 0);
        let redo_cursor = history.redo(&mut lines).expect("redo");
        assert_eq!(redo_cursor.line, 0);
        assert_eq!(redo_cursor.col, 3);
    }

    #[test]
    fn history_limit_evicts_oldest_entries() {
        let start = vec!["0".to_string()];
        let mut history = LineHistory::new(2, &start, 0, 0);
        let mut lines = vec!["1".to_string()];
        history.record_edit(&lines, 0, 1, false);
        lines = vec!["2".to_string()];
        history.record_edit(&lines, 0, 1, false);
        lines = vec!["3".to_string()];
        history.record_edit(&lines, 0, 1, false);

        assert_eq!(history.undo_depth(), 2);
        history.undo(&mut lines).expect("undo #1");
        assert_eq!(lines, vec!["2".to_string()]);
        history.undo(&mut lines).expect("undo #2");
        assert_eq!(lines, vec!["1".to_string()]);
        assert!(history.undo(&mut lines).is_none());
    }

    #[test]
    fn history_keeps_non_empty_document_after_undo() {
        let start = vec![String::new()];
        let mut history = LineHistory::new(4, &start, 0, 0);
        let mut lines = vec!["x".to_string()];
        history.record_edit(&lines, 0, 1, false);
        history.undo(&mut lines).expect("undo");
        assert_eq!(lines, vec![String::new()]);
    }

    #[test]
    fn cursor_type_is_copy() {
        let cursor = HistoryCursor { line: 1, col: 2 };
        let copied = cursor;
        assert_eq!(copied.line, 1);
        assert_eq!(copied.col, 2);
    }
}
