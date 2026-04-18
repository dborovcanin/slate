use std::collections::HashSet;
use std::time::Instant;
use super::folding::FoldRange;

/// All folding index state, kept separate from the editor cursor/content state.
pub struct FoldingState {
    pub ranges: Vec<FoldRange>,
    pub range_by_start: Vec<Option<FoldRange>>,
    pub collapsed_starts: HashSet<usize>,
    pub visible_to_real: Vec<usize>,
    pub real_to_visible: Vec<usize>,
    pub hidden_owner: Vec<Option<usize>>,
    pub placeholder_hidden_lines: Vec<Option<usize>>,
    pub line_has_structure: Vec<bool>,
    pub rescan_pending: bool,
    pub pending_prefix_until: Option<Instant>,
}

impl FoldingState {
    pub fn empty(line_has_structure: Vec<bool>) -> Self {
        Self {
            ranges: Vec::new(),
            range_by_start: Vec::new(),
            collapsed_starts: HashSet::new(),
            visible_to_real: Vec::new(),
            real_to_visible: Vec::new(),
            hidden_owner: Vec::new(),
            placeholder_hidden_lines: Vec::new(),
            line_has_structure,
            rescan_pending: false,
            pending_prefix_until: None,
        }
    }
}
