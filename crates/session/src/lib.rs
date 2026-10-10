//! Framework-independent note state and editing coordination.
pub mod document;
pub mod reminders;
pub mod session;
pub use document::Document;
pub use reminders::*;
pub use session::NoteSession;

pub mod edit;
pub use edit::{EditContext, EditOutcome, SessionEdit};

mod operations;

mod undo;
pub use undo::{SessionUndoOutcome, UndoContext};

pub mod calc;
pub mod variables;

pub mod calc_eval;

pub mod calc_recompute;

pub mod calc_upkeep;

pub mod jobs;

pub mod folds;

pub mod lifecycle;

pub mod save;

pub mod scripts;

pub mod rates;

pub mod display;
