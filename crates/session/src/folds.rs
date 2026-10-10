//! Source-line fold structure; collapse choices and view geometry stay in frontends.
use crate::Document;
use editor_core::{
    buffer::EditDelta,
    folding::{
        build_fold_ranges,
        upkeep::{
            line_has_fold_structure, plan_fold_upkeep_with_delta, FoldUpkeep, FoldUpkeepFlags,
        },
        FoldRange,
    },
};
#[derive(Default)]
pub struct FoldStructure {
    pub ranges: Vec<FoldRange>,
    pub range_by_start: Vec<Option<FoldRange>>,
    pub line_has_structure: Vec<bool>,
    pub line_text_snapshot: Vec<String>,
    pub rescan_pending: bool,
    pub analysis_ready: bool,
}
/// View-independent fold policy inputs; counts are source-line counts.
#[derive(Clone, Copy)]
pub struct FoldInputs {
    /// Folding is reduced only when the resulting source-line count exceeds this limit.
    pub full_feature_line_limit: usize,
    pub has_collapsed: bool,
    pub view_line_count: usize,
}
#[derive(Default, Debug, Clone, Copy)]
pub struct FoldEffect {
    pub rebuild_view: bool,
    pub clear_collapsed: bool,
}
impl FoldStructure {
    pub fn bootstrap(&mut self, len: usize) {
        self.ranges.clear();
        self.range_by_start = vec![None; len];
        self.line_has_structure = vec![false; len];
        self.line_text_snapshot.clear();
        self.rescan_pending = false;
        self.analysis_ready = false;
    }
    pub fn install_ranges(&mut self, ranges: Vec<FoldRange>, len: usize) {
        self.ranges = ranges;
        self.range_by_start = vec![None; len];
        for range in &self.ranges {
            if range.start_line < len {
                self.range_by_start[range.start_line] = Some(*range);
            }
        }
    }
    pub fn recompute(&mut self, doc: &Document) -> FoldEffect {
        self.line_has_structure = doc
            .lines()
            .iter()
            .map(|line| line_has_fold_structure(line))
            .collect();
        self.line_text_snapshot = doc.lines().to_vec();
        self.recompute_ranges(doc)
    }
    pub fn recompute_ranges(&mut self, doc: &Document) -> FoldEffect {
        self.rescan_pending = false;
        self.install_ranges(build_fold_ranges(doc.lines()), doc.lines().len());
        self.analysis_ready = true;
        FoldEffect {
            rebuild_view: true,
            clear_collapsed: false,
        }
    }
    pub fn upkeep(
        &mut self,
        doc: &Document,
        delta: Option<EditDelta>,
        reduced_features: bool,
        has_collapsed: bool,
        view_line_count: usize,
    ) -> FoldEffect {
        let flags = FoldUpkeepFlags {
            reduced_features,
            rescan_pending: self.rescan_pending,
            has_collapsed,
            view_line_count,
        };
        match plan_fold_upkeep_with_delta(
            doc.lines(),
            doc.cursor_line,
            delta,
            &mut self.line_has_structure,
            &mut self.line_text_snapshot,
            &self.ranges,
            flags,
        ) {
            FoldUpkeep::Disabled => {
                self.ranges.clear();
                if self.range_by_start.len() != doc.lines().len() {
                    self.range_by_start = vec![None; doc.lines().len()];
                }
                if self.line_has_structure.len() != doc.lines().len() {
                    self.line_has_structure = vec![false; doc.lines().len()];
                }
                self.line_text_snapshot.clear();
                self.rescan_pending = false;
                self.analysis_ready = false;
                FoldEffect {
                    rebuild_view: true,
                    clear_collapsed: true,
                }
            }
            FoldUpkeep::PendingVisible { reset_map } | FoldUpkeep::Defer { reset_map } => {
                if reset_map {
                    self.install_ranges(Vec::new(), doc.lines().len());
                }
                self.rescan_pending = true;
                self.analysis_ready = false;
                FoldEffect {
                    rebuild_view: reset_map,
                    clear_collapsed: false,
                }
            }
            FoldUpkeep::Recompute => self.recompute(doc),
            FoldUpkeep::Empty => {
                self.install_ranges(Vec::new(), doc.lines().len());
                self.analysis_ready = true;
                FoldEffect {
                    rebuild_view: true,
                    clear_collapsed: false,
                }
            }
            FoldUpkeep::Mapped(ranges) => {
                self.rescan_pending = false;
                self.install_ranges(ranges, doc.lines().len());
                self.analysis_ready = true;
                FoldEffect {
                    rebuild_view: true,
                    clear_collapsed: false,
                }
            }
            FoldUpkeep::Unchanged => FoldEffect::default(),
        }
    }
}
