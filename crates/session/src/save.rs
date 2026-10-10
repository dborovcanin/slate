//! Owned save snapshots and acknowledgement policy.
use crate::{Document, NoteSession};
use app_core::storage::{Db, NoteRevision, ReminderLine};
#[derive(Clone, Copy, Default)]
pub struct SaveContext {
    pub force: bool,
    pub background: bool,
}
#[derive(Clone, Debug)]
pub struct SaveTicket {
    pub session_id: u64,
    pub note_id: String,
    pub expected_revision: String,
    pub edit_seq: u64,
    pub reminders_generation: Option<u64>,
}
pub struct SaveJob {
    pub ticket: SaveTicket,
    pub body: Option<String>,
    pub reminders: Option<Vec<ReminderLine>>,
    pub force: bool,
}
pub struct SaveResult {
    pub ticket: SaveTicket,
    pub result: Result<(NoteRevision, bool), String>,
    /// Returned snapshot allocation, reused as the joined cache when still current.
    pub saved_body: Option<String>,
}
#[derive(Debug, PartialEq, Eq)]
pub enum SaveCompletion {
    Ignored,
    Failed(String),
    Saved { text: bool, current: bool },
}
/// Execute the captured body and reminders together; hosts supply their executor.
pub fn run_save(job: SaveJob, db: &Db) -> SaveResult {
    let options = app_core::note_sources::SaveOptions {
        expected_revision: Some(job.ticket.expected_revision.clone()),
        force: job.force,
        reminders: job.reminders,
    };
    let result = match job.body.as_deref() {
        Some(body) => app_core::note_sources::NoteSourceService::new(db.clone())
            .save_note_revision_by_id(&job.ticket.note_id, body, options)
            .map(|revision| (revision, true)),
        None => db
            .replace_reminders_if(
                &job.ticket.note_id,
                (!job.force).then_some(job.ticket.expected_revision.as_str()),
                options.reminders.as_deref().unwrap_or_default(),
            )
            .map(|revision| (revision, false)),
    };
    SaveResult {
        ticket: job.ticket,
        result,
        saved_body: job.body,
    }
}
impl NoteSession {
    pub fn reminder_lines(&self, doc: &Document) -> Vec<ReminderLine> {
        let mut lines: Vec<_> = self
            .reminder_ghosts
            .iter()
            .map(|(i, ghost)| ReminderLine {
                line_number: *i as i64 + 1,
                remind_at_ms: ghost.remind_at_ms,
                display_at: ghost.display_at.clone(),
                line_text: doc
                    .lines()
                    .get(*i)
                    .cloned()
                    .unwrap_or_else(|| ghost.line_text.clone()),
                reminded_at_ms: ghost.reminded_at_ms,
            })
            .collect();
        lines.sort_by_key(|line| line.line_number);
        lines
    }
    pub fn holds_reminders(&self) -> bool {
        app_core::note_sources::markdown_file_path_from_note_id(&self.note_id).is_none()
    }
    pub fn reminders_for_save(
        &self,
        doc: &Document,
        saving_text: bool,
    ) -> Option<Vec<ReminderLine>> {
        (self.holds_reminders()
            && (self.reminders_unsaved() || (saving_text && !self.reminder_ghosts.is_empty())))
        .then(|| self.reminder_lines(doc))
    }
    pub fn autosave_allowed(&self) -> bool {
        self.autosave_paused_at != Some(self.edit_seq)
    }
    /// A clean request is allocation free; snapshots allocate only when a write is needed.
    pub fn request_save(&self, doc: &mut Document, ctx: SaveContext) -> Option<SaveJob> {
        if !self.editable() || (ctx.background && !self.autosave_allowed()) {
            return None;
        }
        let needs_reminders = self.holds_reminders()
            && (self.reminders_unsaved() || (self.dirty && !self.reminder_ghosts.is_empty()));
        if !self.dirty && !needs_reminders {
            return None;
        }
        let reminders = needs_reminders.then(|| self.reminder_lines(doc));
        Some(SaveJob {
            ticket: SaveTicket {
                session_id: self.session_id,
                note_id: self.note_id.clone(),
                expected_revision: self.stored_revision.clone(),
                edit_seq: self.edit_seq,
                reminders_generation: needs_reminders.then_some(self.reminders_generation),
            },
            body: self.dirty.then(|| {
                if ctx.background {
                    doc.joined_text_cache.clone()
                } else {
                    doc.joined_text_cache.take()
                }
                .unwrap_or_else(|| doc.lines().join("\n"))
            }),
            reminders,
            force: ctx.force,
        })
    }
    /// Acknowledge persisted snapshots even after typing. Revision and dirty checks are independent.
    /// Save results must be drained before opening another lifetime.
    pub fn complete_save(&mut self, doc: &mut Document, result: SaveResult) -> SaveCompletion {
        if result.ticket.session_id != self.session_id || result.ticket.note_id != self.note_id {
            return SaveCompletion::Ignored;
        }
        if self.edit_seq == result.ticket.edit_seq {
            if let Some(body) = result.saved_body {
                doc.joined_text_cache = Some(body);
            }
        }
        let (saved, text) = match result.result {
            Ok(saved) => saved,
            Err(error) => {
                self.autosave_paused_at = Some(self.edit_seq);
                return SaveCompletion::Failed(error);
            }
        };
        if let Some(generation) = result.ticket.reminders_generation {
            self.persisted_reminders_generation =
                self.persisted_reminders_generation.max(generation);
        }
        if self.stored_revision == result.ticket.expected_revision {
            self.stored_revision = saved.updated_at;
        }
        let current = self.edit_seq == result.ticket.edit_seq;
        if text && current {
            self.dirty = false;
            self.history
                .checkpoint(doc.lines(), doc.cursor_line, doc.cursor_col);
        }
        SaveCompletion::Saved { text, current }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{EditContext, LineReminderGhost, SessionEdit};
    use editor_core::{
        buffer::primitives::PrimitiveEdit,
        history::policy::{UndoGrouping, UndoSession},
    };
    use std::time::Duration;
    fn fixture() -> (Document, NoteSession) {
        let doc = Document::from_text("original");
        let history =
            crate::lifecycle::build_history_for_note(doc.lines(), 0, 0, Default::default());
        let mut session = NoteSession::new(history, Default::default(), Default::default());
        session.start_lifetime("note-a");
        session.stored_revision = "r1".into();
        (doc, session)
    }
    fn type_char(doc: &mut Document, session: &mut NoteSession, ch: char) {
        session
            .apply(
                doc,
                SessionEdit::Primitive(PrimitiveEdit::InsertChar(ch)),
                EditContext {
                    grouping: UndoGrouping {
                        session: UndoSession::Insert,
                        elapsed: Duration::ZERO,
                    },
                    folds: None,
                },
            )
            .unwrap();
    }
    fn success(job: SaveJob, revision: &str) -> SaveResult {
        let text = job.body.is_some();
        SaveResult {
            saved_body: job.body,
            ticket: job.ticket.clone(),
            result: Ok((
                NoteRevision {
                    id: job.ticket.note_id,
                    updated_at: revision.into(),
                },
                text,
            )),
        }
    }
    fn reminder(session: &mut NoteSession) {
        session.reminder_ghosts.insert(
            0,
            LineReminderGhost {
                remind_at_ms: 10,
                display_at: "soon".into(),
                line_text: "original".into(),
                reminded_at_ms: None,
            },
        );
        session.reminders_generation += 1;
        session.note_changed();
    }
    #[test]
    fn typing_during_save_acknowledges_revision_and_reminders_but_keeps_new_text_dirty() {
        let (mut doc, mut session) = fixture();
        reminder(&mut session);
        type_char(&mut doc, &mut session, 'x');
        let job = session
            .request_save(&mut doc, SaveContext::default())
            .unwrap();
        let saved_generation = job.ticket.reminders_generation.unwrap();
        assert_eq!(job.body.as_deref(), Some("xoriginal"));
        assert_eq!(job.reminders.as_ref().unwrap()[0].line_text, "xoriginal");
        type_char(&mut doc, &mut session, 'y');
        assert_eq!(
            session.complete_save(&mut doc, success(job, "r2")),
            SaveCompletion::Saved {
                text: true,
                current: false
            }
        );
        assert!(session.dirty);
        assert_eq!(session.stored_revision, "r2");
        assert_eq!(session.persisted_reminders_generation, saved_generation);
        let next = session
            .request_save(&mut doc, SaveContext::default())
            .unwrap();
        assert_eq!(next.ticket.expected_revision, "r2");
        assert_eq!(next.body.as_deref(), Some("xyoriginal"));
    }
    #[test]
    fn old_acknowledgement_never_rolls_back_a_newer_stored_revision() {
        let (mut doc, mut session) = fixture();
        type_char(&mut doc, &mut session, 'x');
        let job = session
            .request_save(&mut doc, SaveContext::default())
            .unwrap();
        session.stored_revision = "r-newer".into();
        session.complete_save(&mut doc, success(job, "r-old"));
        assert_eq!(session.stored_revision, "r-newer");
    }
    #[test]
    fn failure_pauses_the_current_edit_identity_until_another_edit() {
        let (mut doc, mut session) = fixture();
        type_char(&mut doc, &mut session, 'x');
        let job = session
            .request_save(
                &mut doc,
                SaveContext {
                    background: true,
                    ..Default::default()
                },
            )
            .unwrap();
        type_char(&mut doc, &mut session, 'y');
        assert_ne!(job.ticket.edit_seq, session.edit_seq);
        assert_eq!(
            session.complete_save(
                &mut doc,
                SaveResult {
                    saved_body: None,
                    ticket: job.ticket,
                    result: Err("write conflict".into())
                }
            ),
            SaveCompletion::Failed("write conflict".into())
        );
        assert_eq!(session.autosave_paused_at, Some(session.edit_seq));
        assert!(session.dirty && !session.autosave_allowed());
        assert!(session
            .request_save(
                &mut doc,
                SaveContext {
                    background: true,
                    ..Default::default()
                }
            )
            .is_none());
        assert!(session
            .request_save(&mut doc, SaveContext::default())
            .is_some());
        type_char(&mut doc, &mut session, 'z');
        assert!(session.autosave_allowed());
        assert!(session
            .request_save(
                &mut doc,
                SaveContext {
                    background: true,
                    ..Default::default()
                }
            )
            .is_some());
    }
    #[test]
    fn reminders_only_completion_keeps_newer_reminders_unsaved() {
        let (mut doc, mut session) = fixture();
        reminder(&mut session);
        let job = session
            .request_save(&mut doc, SaveContext::default())
            .unwrap();
        let generation = job.ticket.reminders_generation.unwrap();
        assert!(job.body.is_none());
        reminder(&mut session);
        assert_eq!(
            session.complete_save(&mut doc, success(job, "r2")),
            SaveCompletion::Saved {
                text: false,
                current: false
            }
        );
        assert!(!session.dirty && session.reminders_unsaved());
        assert_eq!(session.persisted_reminders_generation, generation);
        assert_eq!(session.stored_revision, "r2");
        let next = session
            .request_save(&mut doc, SaveContext::default())
            .unwrap();
        assert!(next.body.is_none());
        assert_eq!(
            next.ticket.reminders_generation,
            Some(session.reminders_generation)
        );
    }
    #[test]
    fn stale_lifetime_success_and_failure_are_ignored() {
        for fails in [false, true] {
            let (mut doc, mut session) = fixture();
            type_char(&mut doc, &mut session, 'x');
            let job = session
                .request_save(&mut doc, SaveContext::default())
                .unwrap();
            let result = if fails {
                SaveResult {
                    saved_body: None,
                    ticket: job.ticket,
                    result: Err("old error".into()),
                }
            } else {
                success(job, "r-old")
            };
            session.start_lifetime("b");
            session.start_lifetime("note-a");
            assert_eq!(
                session.complete_save(&mut doc, result),
                SaveCompletion::Ignored
            );
            assert_eq!(session.stored_revision, "r1");
            assert!(session.dirty);
            assert_eq!(session.autosave_paused_at, None);
        }
    }
    #[test]
    fn manual_save_transfers_cached_text_while_background_save_clones_it() {
        let (mut doc, mut session) = fixture();
        session.dirty = true;
        doc.joined_text_cache = Some("original".to_owned());
        let original_pointer = doc.joined_text_cache.as_ref().unwrap().as_ptr();
        let job = session
            .request_save(&mut doc, SaveContext::default())
            .unwrap();
        assert!(doc.joined_text_cache.is_none());
        assert_eq!(job.body.as_ref().unwrap().as_ptr(), original_pointer);
        assert_eq!(
            session.complete_save(&mut doc, success(job, "r2")),
            SaveCompletion::Saved {
                text: true,
                current: true
            }
        );
        assert_eq!(
            doc.joined_text_cache.as_ref().unwrap().as_ptr(),
            original_pointer
        );
        session.dirty = true;
        let job = session
            .request_save(
                &mut doc,
                SaveContext {
                    background: true,
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(
            doc.joined_text_cache.as_ref().unwrap().as_ptr(),
            original_pointer
        );
        assert_ne!(job.body.as_ref().unwrap().as_ptr(), original_pointer);
        assert_eq!(job.body.as_deref(), doc.joined_text_cache.as_deref());
    }
    #[test]
    fn save_snapshots_and_results_are_owned_executor_messages() {
        fn require<T: Send + 'static>() {}
        require::<SaveJob>();
        require::<SaveResult>();
    }
}
