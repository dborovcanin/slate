mod engine;

pub use engine::{
    current_eval_generation, start_eval_generation, CalcEngine, NoteEvaluationDiagnostic,
    NoteEvaluationOptions, NoteEvaluationResult, TableCellErrorKind, TableCellEvaluation,
    VariableIndexEntry,
};
