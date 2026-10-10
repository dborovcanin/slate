use rustc_hash::FxHashSet;
use std::time::Instant;

/// All folding index state, kept separate from the editor cursor/content state.
pub struct FoldingState {
    pub collapsed_starts: FxHashSet<usize>,
    pub visible_to_real: Vec<usize>,
    pub real_to_visible: Vec<usize>,
    pub hidden_owner: Vec<Option<usize>>,
    pub placeholder_hidden_lines: Vec<Option<usize>>,
    pub pending_prefix_until: Option<Instant>,
}

impl FoldingState {
    pub fn empty() -> Self {
        Self {
            collapsed_starts: FxHashSet::default(),
            visible_to_real: Vec::new(),
            real_to_visible: Vec::new(),
            hidden_owner: Vec::new(),
            placeholder_hidden_lines: Vec::new(),
            pending_prefix_until: None,
        }
    }
}
