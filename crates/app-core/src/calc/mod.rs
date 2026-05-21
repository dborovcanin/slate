mod engine;

pub use engine::{
    current_eval_generation, scan_cross_note_refs, start_eval_generation, CalcEngine,
    CrossNoteRef, ExternVar, NoteEvaluationDiagnostic, NoteEvaluationOptions,
    NoteEvaluationResult, TableCellErrorKind, TableCellEvaluation, VariableIndexEntry,
};
