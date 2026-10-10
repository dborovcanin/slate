//! Owned jobs and results; hosts choose their executor and poll completions.
use app_core::calc::{CalcEngine, CrossNoteRef, NoteContextCache, NoteEvaluationOptions};
use app_core::{cross_note::CrossNoteVarIndex, storage::Db};
use editor_core::calc_plan::{CalcDependencyIndex, CalcFeatureMask, LineMetadata};
use std::sync::{Arc, Mutex};
#[derive(Debug, Clone)]
/// Identity of the note lifetime and calculation environment captured at dispatch.
pub struct JobTicket {
    pub session_id: u64,
    pub note_id: String,
    pub text_generation: u64,
    pub epoch: u64,
    pub currency_generation: u64,
    pub mask: CalcFeatureMask,
    pub cross_note_enabled: bool,
}
#[derive(Clone, Copy)]
pub struct CalcJobInputs {
    pub mask: CalcFeatureMask,
    pub variables_enabled: bool,
    pub cross_note_enabled: bool,
    pub epoch: u64,
    pub currency_generation: u64,
}
pub enum Job {
    PrepareCalc {
        ticket: JobTicket,
        lines: Vec<String>,
        options: NoteEvaluationOptions,
    },
    BuildCalcIndex {
        ticket: JobTicket,
        lines: Vec<String>,
    },
}
pub struct PreparedCalc {
    pub ticket: JobTicket,
    pub context: NoteContextCache,
    pub refs_scan: Option<(Vec<u64>, Vec<CrossNoteRef>)>,
}
pub struct BuiltCalcIndex {
    pub ticket: JobTicket,
    pub index: Option<CalcDependencyIndex>,
    pub metadata: Vec<LineMetadata>,
}
pub enum JobResult {
    PreparedCalc(PreparedCalc),
    BuiltCalcIndex(BuiltCalcIndex),
}
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Completion {
    Installed,
    Rejected,
}
pub fn run_prepare_calc(job: Job, db: &Db, index: &Arc<Mutex<CrossNoteVarIndex>>) -> JobResult {
    let Job::PrepareCalc {
        ticket,
        lines,
        mut options,
    } = job
    else {
        panic!("expected calc preparation job")
    };
    let engine = CalcEngine::new();
    let refs_scan = if options.cross_note_enabled {
        let refs = app_core::calc::scan_cross_note_refs(&lines);
        let ids: rustc_hash::FxHashSet<&str> = refs
            .iter()
            .map(|reference| reference.note_id.as_str())
            .collect();
        for id in ids {
            app_core::cross_note::load_from_epoch(db, &engine, index, id, ticket.epoch);
        }
        options.extern_vars = match index.lock() {
            Ok(index) if index.epoch() == ticket.epoch => index.extern_vars_for_refs(&refs),
            _ => Vec::new(),
        };
        Some((editor_core::calc_plan::hash_lines(&lines), refs))
    } else {
        None
    };
    let context = engine.prepare_note_context(&lines, &options);
    JobResult::PreparedCalc(PreparedCalc {
        ticket,
        context,
        refs_scan,
    })
}
pub fn run_build_calc_index(job: Job) -> JobResult {
    let Job::BuildCalcIndex { ticket, lines } = job else {
        panic!("expected calc index job")
    };
    let index = editor_core::calc_plan::build_calc_dependency_index(&lines, ticket.mask);
    let metadata = editor_core::calc_plan::line_metadata_for_lines_with_mask(&lines, ticket.mask);
    JobResult::BuiltCalcIndex(BuiltCalcIndex {
        ticket,
        index,
        metadata,
    })
}
impl crate::NoteSession {
    /// A reopened note is a new lifetime even if its identifier is unchanged.
    pub fn start_lifetime(&mut self, note_id: &str) {
        self.session_id = self.session_id.wrapping_add(1);
        self.note_id = note_id.to_owned();
    }
    fn calc_ticket(&self, doc: &crate::Document, inputs: CalcJobInputs) -> JobTicket {
        JobTicket {
            session_id: self.session_id,
            note_id: self.note_id.clone(),
            text_generation: doc.text_generation,
            epoch: inputs.epoch,
            currency_generation: inputs.currency_generation,
            mask: inputs.mask,
            cross_note_enabled: inputs.cross_note_enabled,
        }
    }
    pub fn prepare_calc_job(&self, doc: &crate::Document, inputs: CalcJobInputs) -> Job {
        Job::PrepareCalc {
            ticket: self.calc_ticket(doc, inputs),
            lines: doc.lines().to_vec(),
            options: NoteEvaluationOptions {
                variables_enabled: inputs.variables_enabled,
                cross_note_enabled: inputs.cross_note_enabled,
                table_enabled: inputs.mask.table_enabled,
                ..Default::default()
            },
        }
    }
    pub fn build_calc_index_job(&self, doc: &crate::Document, inputs: CalcJobInputs) -> Job {
        Job::BuildCalcIndex {
            ticket: self.calc_ticket(doc, inputs),
            lines: doc.lines().to_vec(),
        }
    }
    fn accepts_calc_ticket(&self, ticket: &JobTicket, inputs: CalcJobInputs) -> bool {
        ticket.session_id == self.session_id
            && ticket.note_id == self.note_id
            && ticket.epoch == inputs.epoch
            && ticket.currency_generation == inputs.currency_generation
            && ticket.mask == inputs.mask
            && ticket.cross_note_enabled == inputs.cross_note_enabled
    }
    /// Reject other lifetimes/environments. Stale prepared text is revalidated
    /// by CalcEngine; stale indexes are caught up before exposing names.
    pub fn complete(
        &mut self,
        doc: &crate::Document,
        result: JobResult,
        inputs: CalcJobInputs,
        index: &Arc<Mutex<CrossNoteVarIndex>>,
    ) -> Completion {
        match result {
            JobResult::PreparedCalc(build) => {
                if !self.accepts_calc_ticket(&build.ticket, inputs) {
                    return Completion::Rejected;
                }
                let Ok(mut shared) = index.lock() else {
                    return Completion::Rejected;
                };
                if shared.epoch() != build.ticket.epoch {
                    return Completion::Rejected;
                }
                if build.ticket.text_generation == doc.text_generation {
                    if let Some((_, refs)) = &build.refs_scan {
                        shared.update_deps(&self.note_id, refs);
                    }
                }
                self.calc.range_context = build.context;
                if let Some(scan) = build.refs_scan {
                    self.calc.cross_note_refs_scan = Some(scan);
                    self.calc.cross_note_refs_generation = (build.ticket.text_generation
                        == doc.text_generation)
                        .then_some(doc.text_generation);
                }
                // Prepared context revalidates text/options in CalcEngine before reuse.
                Completion::Installed
            }
            JobResult::BuiltCalcIndex(build) => {
                if !self.accepts_calc_ticket(&build.ticket, inputs)
                    || index
                        .lock()
                        .map_or(true, |index| index.epoch() != build.ticket.epoch)
                {
                    return Completion::Rejected;
                }
                if self.calc.calc_dependency_index.is_none() {
                    self.calc.calc_dependency_index = build.index;
                }
                if self.calc.line_metadata.is_empty() {
                    self.calc.line_metadata = build.metadata;
                    editor_core::calc_plan::sync_line_metadata(
                        &mut self.calc.line_metadata,
                        doc.lines(),
                        inputs.mask,
                    );
                    for (i, line) in doc.lines().iter().enumerate() {
                        if self.calc.line_metadata[i].hash
                            != editor_core::calc_plan::hash_line(line)
                        {
                            self.calc.line_metadata[i] =
                                editor_core::calc_plan::line_metadata_with_mask(line, inputs.mask);
                        }
                    }
                }
                self.calc.sync_index_after_idle(doc, inputs.mask, true);
                Completion::Installed
            }
        }
    }
}

pub use crate::save::{run_save, SaveJob, SaveResult};

pub use crate::rates::{run_rate_job, run_rate_startup};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Document, NoteSession};
    use editor_core::history::LineHistory;

    fn fixture(
        text: &str,
    ) -> (
        Document,
        NoteSession,
        CalcJobInputs,
        Arc<Mutex<CrossNoteVarIndex>>,
    ) {
        let doc = Document::from_text(text);
        let history = LineHistory::new(500, doc.lines(), 0, 0, Arc::new(Vec::new()));
        let mut session = NoteSession::new(history, Default::default(), Default::default());
        session.start_lifetime("note-a");
        let inputs = CalcJobInputs {
            mask: Default::default(),
            variables_enabled: true,
            cross_note_enabled: true,
            epoch: 0,
            currency_generation: 7,
        };
        (
            doc,
            session,
            inputs,
            Arc::new(Mutex::new(CrossNoteVarIndex::default())),
        )
    }
    fn prepared(job: Job) -> JobResult {
        let Job::PrepareCalc {
            ticket,
            lines,
            options,
        } = job
        else {
            panic!("prepare job")
        };
        JobResult::PreparedCalc(PreparedCalc {
            ticket,
            context: CalcEngine::new().prepare_note_context(&lines, &options),
            refs_scan: Some((
                editor_core::calc_plan::hash_lines(&lines),
                app_core::calc::scan_cross_note_refs(&lines),
            )),
        })
    }
    #[test]
    fn switched_away_and_back_rejects_preparation_from_prior_lifetime() {
        let (doc, mut session, inputs, index) = fixture("x := 1");
        let result = prepared(session.prepare_calc_job(&doc, inputs));
        session.start_lifetime("note-b");
        session.start_lifetime("note-a");
        assert_eq!(
            session.complete(&doc, result, inputs, &index),
            Completion::Rejected
        );
        assert!(session.calc.cross_note_refs_scan.is_none());
    }
    #[test]
    fn stale_preparation_is_reusable_without_publishing_old_dependencies() {
        let (mut doc, mut session, inputs, index) = fixture("[[dep]].x");
        let entries = vec![app_core::calc::VariableIndexEntry {
            name: "x".into(),
            normalized: "x".into(),
            line: 1,
        }];
        index.lock().unwrap().update_exports(
            "dep",
            &entries,
            &[("x".to_owned(), 3.0)].into_iter().collect(),
        );
        let result = prepared(session.prepare_calc_job(&doc, inputs));
        doc.set_text("1 + 2");
        assert_eq!(
            session.complete(&doc, result, inputs, &index),
            Completion::Installed
        );
        assert_eq!(session.calc.cross_note_refs_generation, None);
        assert!(session.calc.cross_note_refs_scan.is_some());
        assert!(index.lock().unwrap().extern_vars_for("note-a").is_empty());
        let result = prepared(session.prepare_calc_job(&Document::from_text("[[dep]].x"), inputs));
        // A generation-matched result really would publish: the negative assertion above is meaningful.
        let original = Document::from_text("[[dep]].x");
        assert_eq!(
            session.complete(&original, result, inputs, &index),
            Completion::Installed
        );
        assert_eq!(index.lock().unwrap().extern_vars_for("note-a").len(), 1);
    }
    #[test]
    fn environment_changes_reject_both_job_kinds() {
        for change in 0..5 {
            let (doc, mut session, inputs, index) = fixture("old := 1");
            let mut current = inputs;
            match change {
                0 => current.epoch += 1,
                1 => current.currency_generation += 1,
                2 => current.mask.variables_enabled = false,
                3 => current.cross_note_enabled = false,
                _ => index.lock().unwrap().reset(),
            }
            let result = prepared(session.prepare_calc_job(&doc, inputs));
            assert_eq!(
                session.complete(&doc, result, current, &index),
                Completion::Rejected
            );
            let result = run_build_calc_index(session.build_calc_index_job(&doc, inputs));
            assert_eq!(
                session.complete(&doc, result, current, &index),
                Completion::Rejected
            );
            assert!(session.calc.calc_dependency_index.is_none());
        }
    }
    #[test]
    fn stale_index_catches_up_before_exposing_latest_variable_names() {
        let (mut doc, mut session, inputs, index) = fixture("old := 1");
        let result = run_build_calc_index(session.build_calc_index_job(&doc, inputs));
        doc.set_text("latest := 2\nother := 3");
        assert_eq!(
            session.complete(&doc, result, inputs, &index),
            Completion::Installed
        );
        assert_eq!(session.calc.line_metadata.len(), 2);
        assert!(session
            .calc
            .variable_names
            .iter()
            .any(|name| name == "latest"));
        assert!(session
            .calc
            .variable_names
            .iter()
            .any(|name| name == "other"));
        assert!(!session.calc.variable_names.iter().any(|name| name == "old"));
    }
    #[test]
    fn owned_jobs_and_results_can_cross_executor_boundaries() {
        fn require<T: Send + 'static>() {}
        require::<Job>();
        require::<JobResult>();
    }
}
