//! Framework-independent note state and editing coordination.
pub mod document;
pub mod reminders;
pub mod session;
pub use document::Document;
pub use reminders::*;
pub use session::NoteSession;
