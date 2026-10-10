//! Script result lifetime checks; process execution and status text belong to hosts.
use crate::{Document, NoteSession, SessionEdit};
use app_core::scripts::{ScriptOutput, ScriptResponse};
use editor_core::types::TextRange;
#[derive(Clone, Debug)]
pub struct ScriptTicket {
    pub session_id: u64,
    pub note_id: String,
    pub text_generation: u64,
    pub editable: bool,
    pub range: TextRange,
    pub output: ScriptOutput,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScriptRejection {
    Lifetime,
    TextChanged,
    NotEditable,
}
impl NoteSession {
    /// Range uses UTF-8 byte offsets in the document captured at dispatch.
    pub fn script_ticket(
        &self,
        doc: &Document,
        range: TextRange,
        output: ScriptOutput,
    ) -> ScriptTicket {
        ScriptTicket {
            session_id: self.session_id,
            note_id: self.note_id.clone(),
            text_generation: doc.text_generation,
            editable: self.editable(),
            range,
            output,
        }
    }
    pub(crate) fn validate_script(
        &self,
        doc: &Document,
        ticket: &ScriptTicket,
    ) -> Result<(), ScriptRejection> {
        if self.session_id != ticket.session_id || self.note_id != ticket.note_id {
            return Err(ScriptRejection::Lifetime);
        }
        if doc.text_generation != ticket.text_generation {
            return Err(ScriptRejection::TextChanged);
        }
        if !ticket.editable || !self.editable() {
            return Err(ScriptRejection::NotEditable);
        }
        Ok(())
    }
    /// Returned requests revalidate at apply, so delaying consumption cannot overwrite a newer edit.
    pub fn accept_script_result<'a>(
        &self,
        doc: &Document,
        ticket: &'a ScriptTicket,
        response: &'a ScriptResponse,
    ) -> Result<Option<SessionEdit<'a>>, ScriptRejection> {
        self.validate_script(doc, ticket)?;
        Ok(
            (ticket.output != ScriptOutput::Message).then_some(SessionEdit::ScriptReplace {
                ticket,
                text: &response.text,
            }),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::EditContext;
    use editor_core::{
        buffer::primitives::PrimitiveEdit,
        history::policy::{UndoGrouping, UndoSession},
    };
    use std::time::Duration;
    fn fixture() -> (Document, NoteSession) {
        let doc = Document::from_text("aé日z");
        let history =
            crate::lifecycle::build_history_for_note(doc.lines(), 0, 0, Default::default());
        let mut session = NoteSession::new(history, Default::default(), Default::default());
        session.start_lifetime("a");
        (doc, session)
    }
    fn ctx() -> EditContext {
        EditContext {
            grouping: UndoGrouping {
                session: UndoSession::Insert,
                elapsed: Duration::ZERO,
            },
            folds: None,
        }
    }
    fn response() -> ScriptResponse {
        ScriptResponse {
            text: "ü🙂".into(),
            message: None,
        }
    }
    fn ticket(session: &NoteSession, doc: &Document) -> ScriptTicket {
        session.script_ticket(
            doc,
            TextRange { from: 1, to: 6 },
            ScriptOutput::ReplaceSelection,
        )
    }
    #[test]
    fn reopening_same_identifier_rejects_prior_script_lifetime() {
        let (doc, mut session) = fixture();
        let ticket = ticket(&session, &doc);
        session.start_lifetime("b");
        session.start_lifetime("a");
        assert!(matches!(
            session.accept_script_result(&doc, &ticket, &response()),
            Err(ScriptRejection::Lifetime)
        ));
    }
    #[test]
    fn intervening_unicode_edit_rejects_response() {
        let (mut doc, mut session) = fixture();
        let ticket = ticket(&session, &doc);
        session
            .apply(
                &mut doc,
                SessionEdit::Primitive(PrimitiveEdit::InsertChar('λ')),
                ctx(),
            )
            .unwrap();
        assert!(matches!(
            session.accept_script_result(&doc, &ticket, &response()),
            Err(ScriptRejection::TextChanged)
        ));
        assert_eq!(doc.lines(), &["λaé日z".to_owned()]);
    }
    #[test]
    fn access_loss_and_initially_locked_tickets_reject() {
        let (doc, mut session) = fixture();
        let allowed = ticket(&session, &doc);
        session.access_mode = app_core::storage::NoteAccessMode::Encrypted;
        session.is_unlocked = false;
        assert!(matches!(
            session.accept_script_result(&doc, &allowed, &response()),
            Err(ScriptRejection::NotEditable)
        ));
        let locked = ticket(&session, &doc);
        session.is_unlocked = true;
        assert!(matches!(
            session.accept_script_result(&doc, &locked, &response()),
            Err(ScriptRejection::NotEditable)
        ));
    }
    #[test]
    fn delayed_accepted_request_revalidates_before_mutation() {
        let (mut doc, mut session) = fixture();
        let ticket = ticket(&session, &doc);
        let response = response();
        let edit = session
            .accept_script_result(&doc, &ticket, &response)
            .unwrap()
            .unwrap();
        session
            .apply(
                &mut doc,
                SessionEdit::Primitive(PrimitiveEdit::InsertChar('λ')),
                ctx(),
            )
            .unwrap();
        let generation = doc.text_generation;
        let sequence = session.edit_seq;
        assert!(session.apply(&mut doc, edit, ctx()).is_none());
        assert_eq!(doc.lines(), &["λaé日z".to_owned()]);
        assert_eq!(
            (doc.text_generation, session.edit_seq),
            (generation, sequence)
        );
    }
    #[test]
    fn unicode_script_replacement_places_character_cursor_and_isolates_undo() {
        let (mut doc, mut session) = fixture();
        // A neighboring Insert transaction must not coalesce with script output.
        doc.cursor_col = 4;
        session
            .apply(
                &mut doc,
                SessionEdit::Primitive(PrimitiveEdit::InsertChar('!')),
                ctx(),
            )
            .unwrap();
        let ticket = ticket(&session, &doc);
        let response = response();
        let edit = session
            .accept_script_result(&doc, &ticket, &response)
            .unwrap()
            .unwrap();
        session.apply(&mut doc, edit, ctx()).unwrap();
        assert_eq!(doc.lines(), &["aü🙂z!".to_owned()]);
        assert_eq!(doc.cursor_col, 3);
        assert_eq!(session.history.undo_depth(), 2);
        session
            .apply(
                &mut doc,
                SessionEdit::Primitive(PrimitiveEdit::InsertChar('?')),
                ctx(),
            )
            .unwrap();
        session.undo(&mut doc).unwrap();
        assert_eq!(doc.lines(), &["aü🙂z!".to_owned()]);
        session.undo(&mut doc).unwrap();
        assert_eq!(doc.lines(), &["aé日z!".to_owned()]);
        session.undo(&mut doc).unwrap();
        assert_eq!(doc.lines(), &["aé日z".to_owned()]);
    }
    #[test]
    fn message_output_is_validated_without_returning_an_edit() {
        let (doc, mut session) = fixture();
        let ticket =
            session.script_ticket(&doc, TextRange { from: 0, to: 0 }, ScriptOutput::Message);
        assert!(session
            .accept_script_result(&doc, &ticket, &response())
            .unwrap()
            .is_none());
        assert!(!session.dirty);
        assert_eq!(session.history.undo_depth(), 0);
        session.start_lifetime("b");
        assert!(matches!(
            session.accept_script_result(&doc, &ticket, &response()),
            Err(ScriptRejection::Lifetime)
        ));
    }
    #[test]
    fn ticket_is_an_owned_executor_message() {
        fn require<T: Send + 'static>() {}
        require::<ScriptTicket>();
    }
}
