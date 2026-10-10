//! The calc provider front ends share: evaluation inputs, thresholds and the
//! rules for loading values from other notes. Front ends only supply handles.
use crate::calc::{CalcInputs, CalcState};
use crate::calc_recompute::{CalcRecompute, CalcRecomputeInputs, CalcThresholds};
use crate::calc_upkeep::CalcProvider;
use crate::{Document, NoteSession};
use app_core::calc::{CalcEngine, CrossNoteRef, ExternVar};
use app_core::cross_note::CrossNoteVarIndex;
use app_core::storage::Db;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

/// Notes at least this long with variables evaluate calc for the viewport only.
pub const CALC_VIEWPORT_ONLY_MIN_LINES: usize = 2_000;
/// Notes at least this long without formulas or assignments defer calc.
pub const LARGE_DOC_CALC_DEFER_LINES: usize = 20_000;
/// Longest wait for a background load of another note before evaluating.
const IN_FLIGHT_LOAD_WAIT: Duration = Duration::from_millis(200);

impl Default for CalcThresholds {
    fn default() -> Self {
        Self {
            calc_pathological_window_min_lines: 2_000,
            calc_pathological_window_percent: 85,
            calc_pathological_window_streak_threshold: 3,
            calc_forced_full_recompute_cycles: 2,
        }
    }
}

/// Handles for values from other notes: their shared index, storage, and the
/// condition variable background loads notify when they finish.
#[derive(Clone, Copy)]
pub struct CrossNoteSource<'a> {
    pub note_id: &'a str,
    pub index: &'a Arc<Mutex<CrossNoteVarIndex>>,
    pub db: &'a Db,
    pub loaded: &'a Condvar,
}

impl CrossNoteSource<'_> {
    /// Make the exports of every referenced note available. Notes a background
    /// load is already reading get a short bounded wait, so a recompute right
    /// after a completion still sees their values; the rest load synchronously.
    pub fn preload(&self, refs: &[CrossNoteRef]) {
        if refs.is_empty() {
            return;
        }
        let (missing, in_flight): (Vec<String>, Vec<String>) = {
            let dep_ids: rustc_hash::FxHashSet<String> =
                refs.iter().map(|r| r.note_id.clone()).collect();
            match self.index.lock() {
                Ok(index) => {
                    let mut missing = Vec::new();
                    let mut in_flight = Vec::new();
                    for id in dep_ids {
                        if index.was_full_eval_attempted(&id) {
                            // Already loaded.
                        } else if index.is_eval_done_or_in_flight(&id) {
                            in_flight.push(id);
                        } else {
                            missing.push(id);
                        }
                    }
                    (missing, in_flight)
                }
                Err(_) => (Vec::new(), Vec::new()),
            }
        };
        if !in_flight.is_empty() {
            if let Ok(lock) = self.index.lock() {
                let _ = self
                    .loaded
                    .wait_timeout_while(lock, IN_FLIGHT_LOAD_WAIT, |index| {
                        in_flight
                            .iter()
                            .any(|id| !index.was_full_eval_attempted(id))
                    });
            }
        }
        for id in missing {
            app_core::cross_note::load_note_exports(self.db, &CalcEngine::new(), self.index, &id);
        }
    }

    /// Record this note's references and return the values they resolve to.
    pub fn extern_vars(&self, refs: &[CrossNoteRef]) -> Vec<ExternVar> {
        match self.index.lock() {
            Ok(mut index) => {
                index.update_deps(self.note_id, refs);
                index.extern_vars_for(self.note_id)
            }
            Err(_) => Vec::new(),
        }
    }
}

/// The provider a front end passes to session calc calls.
pub struct NoteCalcProvider<'a> {
    pub base: CalcInputs,
    pub cross_note_enabled: bool,
    pub table_enabled: bool,
    pub cross_note: CrossNoteSource<'a>,
    /// Lines of an active selection, whose calc trailers are not rewritten.
    pub selection_range: Option<(usize, usize)>,
}

impl CalcProvider for NoteCalcProvider<'_> {
    fn inputs(&self, calc: &CalcState) -> CalcRecomputeInputs<'_> {
        CalcRecomputeInputs {
            base: self.base,
            variables_enabled: self.base.math_enabled
                && self.base.mask.variables_enabled
                && calc.cached_has_variable_assignment,
            cross_note_enabled: self.cross_note_enabled,
            table_enabled: self.table_enabled,
            note_id: self.cross_note.note_id,
            index: self.cross_note.index,
            selection_range: self.selection_range,
            thresholds: CalcThresholds::default(),
        }
    }
    fn preload_refs(&self, refs: &[CrossNoteRef]) {
        self.cross_note.preload(refs);
    }
    fn extern_vars(&self, lines: &[String]) -> Vec<ExternVar> {
        self.cross_note
            .extern_vars(&app_core::calc::scan_cross_note_refs(lines))
    }
}

impl NoteSession {
    /// Full or incremental recomputation; values from other notes are loaded
    /// first and fetched only on paths that evaluate references.
    pub fn recompute_calc_with(
        &mut self,
        doc: &mut Document,
        provider: &dyn CalcProvider,
    ) -> CalcRecompute {
        let inputs = provider.inputs(&self.calc);
        if inputs.base.math_enabled && inputs.cross_note_enabled {
            let refs = app_core::calc::scan_cross_note_refs(doc.lines());
            provider.preload_refs(&refs);
        }
        self.recompute_calc(doc, inputs, &mut |lines| provider.extern_vars(lines))
    }

    /// Evaluate lines `from..to` (the viewport). References to other notes are
    /// rescanned only where the text changed since the last scan.
    pub fn evaluate_calc_range(
        &mut self,
        doc: &Document,
        from: usize,
        to: usize,
        provider: &NoteCalcProvider<'_>,
    ) {
        let inputs = provider.inputs(&self.calc);
        if !inputs.base.math_enabled {
            self.clear_disabled_calc(doc);
            return;
        }
        let extern_vars = if inputs.cross_note_enabled {
            let generation = doc.text_generation();
            let (line_hashes, refs) = self.calc.cross_note_refs(doc);
            provider.cross_note.preload(&refs);
            let extern_vars = provider.cross_note.extern_vars(&refs);
            self.calc.cross_note_refs_scan = Some((line_hashes, refs));
            self.calc.cross_note_refs_generation = Some(generation);
            extern_vars
        } else {
            Vec::new()
        };
        self.calc.evaluate_range(
            doc,
            from,
            to,
            app_core::calc::NoteEvaluationOptions {
                variables_enabled: inputs.variables_enabled,
                cross_note_enabled: inputs.cross_note_enabled,
                table_enabled: inputs.table_enabled,
                extern_vars,
                ..Default::default()
            },
        );
    }
}

/// Values for the first note shown at startup: referenced notes are refreshed
/// and loaded synchronously, before any background loader exists.
pub fn load_extern_vars_at_startup(
    db: &Db,
    engine: &CalcEngine,
    index: &Arc<Mutex<CrossNoteVarIndex>>,
    note_id: &str,
    lines: &[String],
) -> Vec<ExternVar> {
    let refs = app_core::calc::scan_cross_note_refs(lines);
    if refs.is_empty() {
        return Vec::new();
    }
    app_core::cross_note::refresh_referenced_notes(db, index, lines);
    let dep_ids: rustc_hash::FxHashSet<&str> = refs.iter().map(|r| r.note_id.as_str()).collect();
    for dep_id in dep_ids {
        app_core::cross_note::load_note_exports(db, engine, index, dep_id);
    }
    match index.lock() {
        Ok(mut index) => {
            index.update_deps(note_id, &refs);
            index.extern_vars_for(note_id)
        }
        Err(_) => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use editor_core::calc_plan::CalcFeatureMask;
    use std::cell::Cell;

    fn base(math_enabled: bool) -> CalcInputs {
        CalcInputs {
            mask: CalcFeatureMask {
                math_enabled,
                table_enabled: true,
                variables_enabled: true,
            },
            math_enabled,
            viewport_only: false,
        }
    }
    fn session(doc: &Document) -> NoteSession {
        NoteSession::new(
            crate::lifecycle::build_history_for_note(doc.lines(), 0, 0, Default::default()),
            Default::default(),
            Default::default(),
        )
    }
    struct DisabledProvider {
        index: Arc<Mutex<CrossNoteVarIndex>>,
        preloads: Cell<usize>,
    }
    impl CalcProvider for DisabledProvider {
        fn inputs(&self, _: &CalcState) -> CalcRecomputeInputs<'_> {
            CalcRecomputeInputs {
                base: base(false),
                variables_enabled: true,
                cross_note_enabled: true,
                table_enabled: true,
                note_id: "active",
                index: &self.index,
                selection_range: None,
                thresholds: Default::default(),
            }
        }
        fn preload_refs(&self, refs: &[CrossNoteRef]) {
            assert!(!refs.is_empty());
            self.preloads.set(self.preloads.get() + 1);
        }
        fn extern_vars(&self, _: &[String]) -> Vec<ExternVar> {
            panic!("disabled math must not request external values")
        }
    }
    #[test]
    fn disabled_full_recompute_does_not_preload() {
        let mut doc = Document::from_text("[[01KP0YD099X9TQENYJQ1SE9X8V]].price * 2");
        let mut session = session(&doc);
        let provider = DisabledProvider {
            index: Arc::new(Mutex::new(Default::default())),
            preloads: Cell::new(0),
        };
        assert!(matches!(
            session.recompute_calc_with(&mut doc, &provider),
            CalcRecompute::Disabled
        ));
        assert_eq!(provider.preloads.get(), 0);
        assert!(session.calc().results.iter().all(Option::is_none));
    }
    #[test]
    fn disabled_viewport_clears_results_without_loading_references() {
        let mut doc = Document::from_text("1 + 1");
        let mut session = session(&doc);
        let root =
            std::env::temp_dir().join(format!("slate-disabled-range-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let db = Db::open(root.join("notes.db")).unwrap();
        let index = Arc::new(Mutex::new(Default::default()));
        let loaded = Condvar::new();
        let mut provider = NoteCalcProvider {
            base: base(true),
            cross_note_enabled: false,
            table_enabled: true,
            cross_note: CrossNoteSource {
                note_id: "active",
                index: &index,
                db: &db,
                loaded: &loaded,
            },
            selection_range: None,
        };
        session.evaluate_calc_range(&doc, 0, 1, &provider);
        assert_eq!(session.calc().results[0].as_deref(), Some("2"));
        doc.set_text("1 + 1\n[[01KP0YD099X9TQENYJQ1SE9X8V]].price");
        provider.base = base(false);
        provider.cross_note_enabled = true;
        session.evaluate_calc_range(&doc, 0, 2, &provider);
        assert!(session.calc().results.iter().all(Option::is_none));
        assert!(session.calc().cross_note_refs_scan.is_none());
        assert!(session.calc().cross_note_refs_generation.is_none());
        assert!(!index
            .lock()
            .unwrap()
            .was_full_eval_attempted("01KP0YD099X9TQENYJQ1SE9X8V"));
        drop(db);
        std::fs::remove_dir_all(root).unwrap();
    }
}
