pub use crate::editor_core::folding::{FoldKind, FoldRange};

pub use crate::editor_core::folding::build_fold_ranges;

#[cfg(test)]
pub fn describe_fold_ranges(lines: &[&str]) -> Vec<(usize, usize, &'static str)> {
    let owned = lines
        .iter()
        .map(|line| (*line).to_string())
        .collect::<Vec<_>>();
    build_fold_ranges(&owned)
        .into_iter()
        .map(|range| (range.start_line, range.end_line, range.kind.as_str()))
        .collect()
}
