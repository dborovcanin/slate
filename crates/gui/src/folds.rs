//! Folding in the desktop view. Which blocks can fold comes from the shared
//! `note_session::folds` structure; which are folded and which lines are
//! hidden is view state and lives here, as in the terminal app.
use note_session::folds::FoldStructure;
use note_session::Document;
use rustc_hash::FxHashSet;

/// Notes longer than this do not fold, as in the terminal app's
/// reduced-feature mode.
const MAX_FOLD_LINES: usize = 50_000;

#[derive(Default)]
pub struct Folds {
    structure: FoldStructure,
    /// First lines of the folded blocks.
    collapsed: FxHashSet<usize>,
    /// For each line, the folded block that hides it.
    hidden_by: Vec<Option<usize>>,
    /// The structure no longer matches the text.
    stale: bool,
}

/// What a fold command did.
#[derive(Debug, PartialEq, Eq)]
pub enum Folded {
    Changed { folded: bool, lines: usize },
    Already(bool),
    NoBlock,
    TooLarge,
}

impl Folds {
    pub fn any(&self) -> bool {
        !self.collapsed.is_empty()
    }

    /// The text changed: remember to look at the blocks again. Folded
    /// blocks below the edit move with their lines.
    pub fn edited(&mut self, first_changed: usize, line_delta: isize) {
        self.stale = true;
        if self.collapsed.is_empty() || line_delta == 0 {
            return;
        }
        self.collapsed = self
            .collapsed
            .drain()
            .filter_map(|start| {
                if start <= first_changed {
                    Some(start)
                } else {
                    start.checked_add_signed(line_delta)
                }
            })
            .collect();
        self.hidden_by.clear();
    }

    /// Bring the blocks up to date with the text; drop folds whose block
    /// is gone and rebuild the hidden lines.
    pub fn refresh(&mut self, doc: &Document) {
        if !self.stale && self.hidden_by.len() == doc.lines().len() {
            return;
        }
        self.stale = false;
        if doc.lines().len() > MAX_FOLD_LINES {
            self.structure.bootstrap(doc.lines().len());
            self.collapsed.clear();
            self.hidden_by = vec![None; doc.lines().len()];
            return;
        }
        self.structure.recompute(doc);
        let starts = &self.structure.range_by_start;
        self.collapsed
            .retain(|start| starts.get(*start).is_some_and(|r| r.is_some()));
        self.rebuild_hidden(doc.lines().len());
    }

    fn rebuild_hidden(&mut self, len: usize) {
        self.hidden_by = vec![None; len];
        for &start in &self.collapsed {
            if let Some(Some(range)) = self.structure.range_by_start.get(start) {
                for line in range.start_line + 1..=range.end_line.min(len.saturating_sub(1)) {
                    self.hidden_by[line] = Some(start);
                }
            }
        }
    }

    /// The folded block hiding `line`.
    pub fn hidden_by(&self, line: usize) -> Option<usize> {
        self.hidden_by.get(line).copied().flatten()
    }

    /// Lines folded away under `line`, if it starts a folded block.
    pub fn folded_lines(&self, line: usize) -> Option<usize> {
        if !self.collapsed.contains(&line) {
            return None;
        }
        let range = (*self.structure.range_by_start.get(line)?)?;
        Some(range.end_line - range.start_line)
    }

    /// The block `line` belongs to: the one it hides in or starts, else the
    /// innermost enclosing one.
    fn block_for(&self, line: usize) -> Option<usize> {
        if let Some(owner) = self.hidden_by(line) {
            return Some(owner);
        }
        if self
            .structure
            .range_by_start
            .get(line)
            .is_some_and(|r| r.is_some())
        {
            return Some(line);
        }
        self.structure
            .ranges
            .iter()
            .filter(|r| r.start_line < line && line <= r.end_line)
            .min_by_key(|r| r.end_line - r.start_line)
            .map(|r| r.start_line)
    }

    /// Fold (`Some(true)`), unfold (`Some(false)`) or toggle (`None`) the
    /// block at `line`.
    pub fn set(&mut self, doc: &Document, line: usize, want: Option<bool>) -> Folded {
        if doc.lines().len() > MAX_FOLD_LINES {
            return Folded::TooLarge;
        }
        self.stale = true;
        self.refresh(doc);
        let Some(start) = self.block_for(line) else {
            return Folded::NoBlock;
        };
        let Some(Some(range)) = self.structure.range_by_start.get(start).copied() else {
            return Folded::NoBlock;
        };
        let was = self.collapsed.contains(&start);
        let target = want.unwrap_or(!was);
        if target == was {
            return Folded::Already(was);
        }
        if target {
            self.collapsed.insert(start);
        } else {
            self.collapsed.remove(&start);
        }
        self.rebuild_hidden(doc.lines().len());
        Folded::Changed {
            folded: target,
            lines: range.end_line - range.start_line,
        }
    }

    /// Where the cursor goes when it is on a hidden line: the line after
    /// the block when moving down, else the block's first line.
    pub fn visible_line(&self, line: usize, moving_down: bool, last: usize) -> usize {
        let Some(start) = self.hidden_by(line) else {
            return line;
        };
        if moving_down {
            if let Some(Some(range)) = self.structure.range_by_start.get(start) {
                if range.end_line < last {
                    return range.end_line + 1;
                }
            }
        }
        start
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc() -> Document {
        Document::from_text("# One\na\nb\n# Two\nc\n")
    }

    #[test]
    fn folding_hides_the_block_and_unfolding_shows_it() {
        let d = doc();
        let mut f = Folds::default();
        assert_eq!(
            f.set(&d, 0, None),
            Folded::Changed {
                folded: true,
                lines: 2
            }
        );
        assert_eq!(f.hidden_by(1), Some(0));
        assert_eq!(f.hidden_by(2), Some(0));
        assert_eq!(f.hidden_by(3), None);
        assert_eq!(f.folded_lines(0), Some(2));
        assert_eq!(f.set(&d, 0, Some(true)), Folded::Already(true));
        assert_eq!(
            f.set(&d, 0, Some(false)),
            Folded::Changed {
                folded: false,
                lines: 2
            }
        );
        assert!(!f.any());
        assert_eq!(f.hidden_by(1), None);
        assert_eq!(
            Folds::default().set(&Document::from_text("x"), 0, None),
            Folded::NoBlock
        );
    }

    #[test]
    fn the_cursor_leaves_hidden_lines() {
        let d = doc();
        let mut f = Folds::default();
        f.set(&d, 0, Some(true));
        assert_eq!(f.visible_line(1, true, 5), 3);
        assert_eq!(f.visible_line(2, false, 5), 0);
        assert_eq!(f.visible_line(4, true, 5), 4);
    }

    #[test]
    fn folds_move_with_edits_above_them() {
        let mut d = doc();
        let mut f = Folds::default();
        f.set(&d, 3, Some(true));
        // A line is added above the "Two" block.
        let mut lines = d.lines().to_vec();
        lines.insert(0, "intro".into());
        d = Document::from_text(&lines.join("\n"));
        f.edited(0, 1);
        f.refresh(&d);
        assert_eq!(f.folded_lines(4), Some(1));
        assert_eq!(f.hidden_by(5), Some(4));
        // Text that no longer starts a block drops the fold.
        let mut d2 = Document::from_text("x\ny\nz");
        f.edited(0, -3);
        f.refresh(&d2);
        assert!(!f.any());
        d2 = d;
        let _ = d2;
    }
}
