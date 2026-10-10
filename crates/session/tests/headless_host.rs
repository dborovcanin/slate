//! A synchronous, headless executor exercising public session boundaries with SQLite.
use app_core::{cross_note::CrossNoteVarIndex, storage::Db};
use editor_core::{
    buffer::primitives::PrimitiveEdit,
    history::policy::{UndoGrouping, UndoSession},
    types::TextRange,
};
use note_session::{
    jobs::{self, CalcJobInputs, Completion},
    rates::{Fetched, RateResult, RateService},
    save::{run_save, SaveCompletion, SaveContext, SaveJob, SaveResult},
    scripts::ScriptRejection,
    Document, EditContext, LineReminderGhost, NoteSession, SessionEdit,
};
use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
static NEXT_DB: AtomicU64 = AtomicU64::new(0);
struct Host {
    doc: Document,
    session: NoteSession,
    index: Arc<Mutex<CrossNoteVarIndex>>,
    rates: RateService,
    db: Db,
    dir: PathBuf,
}
impl Host {
    fn new(body: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "slate-headless-{}-{}",
            std::process::id(),
            NEXT_DB.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let db = Db::open(dir.join("notes.db")).unwrap();
        db.create_note_with_context("a", Default::default(), None, None)
            .unwrap();
        db.create_note_with_context("b", Default::default(), None, None)
            .unwrap();
        let a = db.save_note("a", body).unwrap();
        let mut doc = Document::default();
        let history =
            note_session::lifecycle::build_history_for_note(doc.lines(), 0, 0, Default::default());
        let mut session = NoteSession::new(history, Default::default(), Default::default());
        session.open(&a, &mut doc);
        Self {
            doc,
            session,
            index: Arc::new(Mutex::new(Default::default())),
            rates: Default::default(),
            db,
            dir,
        }
    }
    fn context() -> EditContext {
        EditContext {
            grouping: UndoGrouping {
                session: UndoSession::Command,
                elapsed: Duration::from_secs(1),
            },
            folds: None,
        }
    }
    fn edit(&mut self, edit: SessionEdit<'_>) {
        self.session
            .apply(&mut self.doc, edit, Self::context())
            .expect("effective edit");
    }
    fn type_char(&mut self, c: char) {
        self.edit(SessionEdit::Primitive(PrimitiveEdit::InsertChar(c)));
    }
    fn save_job(&mut self, force: bool) -> SaveJob {
        self.session
            .request_save(
                &mut self.doc,
                SaveContext {
                    force,
                    background: true,
                },
            )
            .expect("save needed")
    }
    fn ack(&mut self, result: SaveResult) -> SaveCompletion {
        self.session.complete_save(&mut self.doc, result)
    }
    fn switch(&mut self, id: &str) {
        let note = self.db.get_note(id).unwrap().unwrap();
        self.session.open(&note, &mut self.doc);
    }
    fn calc_inputs(&self) -> CalcJobInputs {
        CalcJobInputs {
            mask: Default::default(),
            variables_enabled: true,
            cross_note_enabled: true,
            epoch: self.index.lock().unwrap().epoch(),
            currency_generation: 0,
        }
    }
    fn add_reminder(&mut self, line: usize) {
        let mark = LineReminderGhost {
            remind_at_ms: 10,
            display_at: "soon".into(),
            line_text: self.doc.lines()[line].clone(),
            reminded_at_ms: None,
        };
        assert!(self
            .session
            .set_reminder(&self.doc, line, Some(mark), false));
    }
}
impl Drop for Host {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}
fn duplicate_success(result: &SaveResult) -> SaveResult {
    let (revision, text) = result.result.as_ref().expect("successful persisted result");
    SaveResult {
        ticket: result.ticket.clone(),
        saved_body: result.saved_body.clone(),
        result: Ok((
            app_core::storage::NoteRevision {
                id: revision.id.clone(),
                updated_at: revision.updated_at.clone(),
            },
            *text,
        )),
    }
}
#[test]
fn synchronous_and_delayed_saves_acknowledge_storage_without_losing_newer_typing() {
    let mut host = Host::new("original");
    host.type_char('λ');
    let first = run_save(host.save_job(false), &host.db);
    let duplicate = duplicate_success(&first);
    host.type_char('é');
    assert_eq!(
        host.ack(first),
        SaveCompletion::Saved {
            text: true,
            current: false
        }
    );
    assert!(host.session.dirty());
    assert_eq!(host.doc.lines(), &["λéoriginal"]);
    assert_eq!(host.db.get_note("a").unwrap().unwrap().body, "λoriginal");
    let latest = run_save(host.save_job(false), &host.db);
    assert_eq!(
        host.ack(latest),
        SaveCompletion::Saved {
            text: true,
            current: true
        }
    );
    let revision = host.session.stored_revision().to_owned();
    assert_eq!(host.db.get_note("a").unwrap().unwrap().body, "λéoriginal");
    host.ack(duplicate);
    assert_eq!(
        host.session.stored_revision(),
        revision,
        "late acknowledgement cannot roll revision back"
    );
    assert!(!host.session.dirty());
    assert!(host
        .session
        .request_save(&mut host.doc, SaveContext::default())
        .is_none());
}
#[test]
fn conflicted_save_pauses_current_snapshot_and_new_edit_resumes_then_reminders_save_alone() {
    let mut host = Host::new("original");
    host.type_char('x');
    let captured = host.save_job(false);
    host.db.save_note("a", "outside").unwrap();
    let failed = run_save(captured, &host.db);
    assert!(matches!(host.ack(failed), SaveCompletion::Failed(_)));
    assert!(!host.session.autosave_allowed());
    assert!(host
        .session
        .request_save(
            &mut host.doc,
            SaveContext {
                force: false,
                background: true
            }
        )
        .is_none());
    host.type_char('y');
    assert!(host.session.autosave_allowed());
    let forced = run_save(host.save_job(true), &host.db);
    assert_eq!(
        host.ack(forced),
        SaveCompletion::Saved {
            text: true,
            current: true
        }
    );
    host.add_reminder(0);
    let reminder_job = host.save_job(false);
    assert!(reminder_job.body.is_none());
    assert_eq!(
        reminder_job.reminders.as_ref().unwrap()[0].line_text,
        "xyoriginal"
    );
    let saved = run_save(reminder_job, &host.db);
    assert_eq!(
        host.ack(saved),
        SaveCompletion::Saved {
            text: false,
            current: true
        }
    );
    assert!(!host.session.reminders_unsaved());
    assert_eq!(host.db.get_note("a").unwrap().unwrap().body, "xyoriginal");
}
#[test]
fn joins_and_block_replacements_preserve_reminder_ownership_through_history() {
    let mut host = Host::new("left\nright\nlast");
    host.add_reminder(0);
    host.add_reminder(2);
    // Installed marks are the persisted baseline for this history round.
    host.session
        .install_reminders(host.session.reminders().clone());
    host.doc.cursor_line = 1;
    host.doc.cursor_col = 0;
    host.edit(SessionEdit::Primitive(PrimitiveEdit::Backspace));
    assert_eq!(host.doc.lines(), &["leftright", "last"]);
    assert!(host.session.reminders().contains_key(&0));
    assert!(host.session.reminders().contains_key(&1));
    host.session.undo(&mut host.doc).unwrap();
    assert_eq!(host.doc.lines(), &["left", "right", "last"]);
    assert!(host.session.reminders().contains_key(&2));
    host.session.redo(&mut host.doc).unwrap();
    assert_eq!(host.doc.lines(), &["leftright", "last"]);
    let op = host
        .session
        .prepare_stored_replacement(&host.doc, "replacement\nlast")
        .unwrap();
    host.edit(SessionEdit::Operation(&op));
    assert_eq!(host.doc.lines(), &["replacement", "last"]);
    assert!(host.session.reminders().contains_key(&1));
    host.session.undo(&mut host.doc).unwrap();
    assert_eq!(host.doc.lines(), &["leftright", "last"]);
    host.session.redo(&mut host.doc).unwrap();
    assert_eq!(host.doc.lines(), &["replacement", "last"]);
}
#[test]
fn executor_results_validate_text_lifetimes_epochs_and_application_wide_rates() {
    let mut host = Host::new("base := 1\nbase + 2");
    let script = host.session.script_ticket(
        &host.doc,
        TextRange { from: 0, to: 4 },
        app_core::scripts::ScriptOutput::ReplaceSelection,
    );
    let prep = host.session.prepare_calc_job(&host.doc, host.calc_inputs());
    let prepared = jobs::run_prepare_calc(prep, &host.db, &host.index);
    host.type_char(' ');
    assert_eq!(
        host.session
            .complete(&host.doc, prepared, host.calc_inputs(), &host.index),
        Completion::Installed,
        "stale preparation must be revalidated at evaluation"
    );
    let inputs = host.calc_inputs();
    let stale_index = host.session.build_calc_index_job(&host.doc, inputs);
    host.type_char(' ');
    assert_eq!(
        host.session.complete(
            &host.doc,
            jobs::run_build_calc_index(stale_index),
            host.calc_inputs(),
            &host.index
        ),
        Completion::Installed
    );
    let fenced = host
        .session
        .build_calc_index_job(&host.doc, host.calc_inputs());
    host.index.lock().unwrap().reset();
    assert_eq!(
        host.session.complete(
            &host.doc,
            jobs::run_build_calc_index(fenced),
            host.calc_inputs(),
            &host.index
        ),
        Completion::Rejected
    );
    let old_lifetime = host
        .session
        .build_calc_index_job(&host.doc, host.calc_inputs());
    let rate_job = host
        .rates
        .request_refresh(
            app_core::currency::CurrencyConfig {
                argv: vec!["unused".into()],
                timeout_seconds: 1,
            },
            host.dir.join("rates.json"),
        )
        .unwrap();
    host.switch("b");
    host.switch("a");
    let response = app_core::scripts::ScriptResponse {
        text: "late".into(),
        message: None,
    };
    assert!(matches!(
        host.session
            .accept_script_result(&host.doc, &script, &response),
        Err(ScriptRejection::Lifetime)
    ));
    assert_eq!(
        host.session.complete(
            &host.doc,
            jobs::run_build_calc_index(old_lifetime),
            host.calc_inputs(),
            &host.index
        ),
        Completion::Rejected
    );
    let completion = host
        .rates
        .complete(RateResult {
            generation: rate_job.generation,
            result: Ok(Fetched {
                rates: app_core::currency::ExchangeRates {
                    base: "EUR".into(),
                    rates: [("USD".into(), 1.2345)].into_iter().collect(),
                    as_of: Some("headless".into()),
                    fetched_at: 42,
                },
                save_error: None,
            }),
        })
        .unwrap()
        .unwrap();
    assert_eq!(completion.as_of.as_deref(), Some("headless"));
    assert!(!host.rates.running());
    assert!(host
        .rates
        .complete(RateResult {
            generation: rate_job.generation,
            result: Err("duplicate".into())
        })
        .is_none());
}
#[test]
fn executor_messages_are_owned_and_sendable_without_an_event_loop() {
    fn owned<T: Send + 'static>() {}
    owned::<jobs::Job>();
    owned::<jobs::JobResult>();
    owned::<SaveJob>();
    owned::<SaveResult>();
    owned::<note_session::scripts::ScriptTicket>();
    owned::<note_session::rates::RateJob>();
    owned::<RateResult>();
}
