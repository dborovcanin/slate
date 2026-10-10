/// Whole-note calc preparation built off the input thread when a large
/// viewport note opens.
pub struct RangeContextBuild {
    pub note_id: String,
    pub context: app_core::calc::NoteContextCache,
    /// Line hashes and cross-note refs the build scanned, if it did.
    pub refs_scan: Option<(Vec<u64>, Vec<app_core::calc::CrossNoteRef>)>,
}

/// Thread receivers owned exclusively by the terminal runtime.
#[derive(Default)]
pub struct CalcWorkers {
    pub range_context_build: Option<std::sync::mpsc::Receiver<RangeContextBuild>>,
    pub index_build: Option<
        std::sync::mpsc::Receiver<(
            Option<crate::editor_core::calc_plan::CalcDependencyIndex>,
            Vec<crate::editor_core::calc_plan::LineMetadata>,
        )>,
    >,
}
