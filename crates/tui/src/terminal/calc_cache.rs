/// Thread receivers owned exclusively by the terminal runtime.
#[derive(Default)]
pub struct CalcWorkers {
    pub range_context_build: Option<std::sync::mpsc::Receiver<note_session::jobs::JobResult>>,
    pub index_build: Option<std::sync::mpsc::Receiver<note_session::jobs::JobResult>>,
}
