use super::*;

fn record<R>(
    policy: &mut UndoPolicy<R>,
    history: &mut LineHistory,
    text: &str,
    millis: u64,
    session: UndoSession,
) {
    policy.record_text(
        history,
        &[text.to_string()],
        HistoryCursor {
            line: 0,
            col: text.chars().count(),
        },
        None,
        UndoGrouping {
            session,
            elapsed: Duration::from_millis(millis),
        },
    );
}

#[test]
fn typing_boundary_insert_pauses_and_command_boundaries() {
    let mut history = LineHistory::new(8, &[String::new()], 0, 0, ());
    let mut policy = UndoPolicy::<()>::default();
    record(&mut policy, &mut history, "a", 1000, UndoSession::Other);
    record(&mut policy, &mut history, "ab", 299, UndoSession::Other);
    assert_eq!(history.undo_depth(), 1);
    assert_eq!(policy.undo_depth(), 1);
    record(&mut policy, &mut history, "abc", 300, UndoSession::Other);
    assert_eq!(history.undo_depth(), 2);
    record(
        &mut policy,
        &mut history,
        "abcd",
        10_000,
        UndoSession::Insert,
    );
    assert_eq!(history.undo_depth(), 2);
    policy.begin_input(&mut history, UndoSession::Command);
    record(&mut policy, &mut history, "abcde", 0, UndoSession::Command);
    assert_eq!(history.undo_depth(), 3);
    assert_eq!(policy.undo_depth(), 3);
    let mut lines = vec!["abcde".to_string()];
    for expected in ["abcd", "ab", ""] {
        assert_eq!(policy.undo_action(), Some(&UndoAction::Text));
        history.undo(&mut lines).expect("undo");
        policy.complete_undo();
        assert_eq!(lines, [expected]);
    }
    assert!(policy.undo_action().is_none());
}

#[test]
fn self_cancelling_step_removes_only_its_text_marker() {
    let mut history = LineHistory::new(8, &[String::new()], 0, 0, ());
    let mut policy = UndoPolicy::<&str>::default();
    record(&mut policy, &mut history, "a", 1000, UndoSession::Other);
    policy.record_reminder("opaque reminder");
    history.set_marks(());
    record(&mut policy, &mut history, "ax", 0, UndoSession::Insert);
    record(&mut policy, &mut history, "a", 0, UndoSession::Insert);
    assert_eq!(history.undo_depth(), 1);
    assert_eq!(policy.undo_depth(), 2);
    assert_eq!(
        policy.undo_action(),
        Some(&UndoAction::Reminder("opaque reminder"))
    );
    policy.complete_undo();
    assert_eq!(policy.undo_action(), Some(&UndoAction::Text));
    let mut lines = vec!["a".to_string()];
    history.undo(&mut lines).expect("original text step");
    assert_eq!(lines, [""]);
}

#[test]
fn new_reminder_and_text_edits_truncate_the_action_redo_branch() {
    let mut history = LineHistory::new(8, &[String::new()], 0, 0, ());
    let mut policy = UndoPolicy::<&str>::default();
    record(&mut policy, &mut history, "a", 1000, UndoSession::Other);
    record(&mut policy, &mut history, "ab", 1000, UndoSession::Other);
    let mut lines = vec!["ab".to_string()];
    history.undo(&mut lines).unwrap();
    policy.complete_undo();
    assert_eq!(policy.redo_action(), Some(&UndoAction::Text));
    policy.record_reminder("new reminder");
    assert!(policy.redo_action().is_none());
    history.set_marks(());
    record(&mut policy, &mut history, "a!", 0, UndoSession::Insert);
    assert_eq!(history.redo_depth(), 0);
    assert!(policy.redo_action().is_none());
    assert_eq!(policy.undo_depth(), 3);
}

#[test]
fn text_and_reminder_actions_replay_in_order_with_attached_marks() {
    let mut history = LineHistory::new(8, &[String::new()], 0, 0, 0u8);
    let mut policy = UndoPolicy::<(u8, u8)>::default();
    let mut lines = vec!["a".to_string()];
    let grouping = UndoGrouping {
        session: UndoSession::Other,
        elapsed: Duration::from_secs(1),
    };
    policy.record_text(
        &mut history,
        &lines,
        HistoryCursor { line: 0, col: 1 },
        None,
        grouping,
    );
    history.record_marks(0);
    policy.record_reminder((0, 1));
    history.set_marks(1);
    lines[0] = "ab".to_string();
    policy.record_text(
        &mut history,
        &lines,
        HistoryCursor { line: 0, col: 2 },
        None,
        grouping,
    );
    history.record_marks(1);
    for (text, marks, expected_action) in [
        ("a", 1, UndoAction::Text),
        ("a", 0, UndoAction::Reminder((0, 1))),
        ("", 0, UndoAction::Text),
    ] {
        let action = policy.undo_action().cloned().expect("undo action");
        assert_eq!(action, expected_action);
        match action {
            UndoAction::Text => {
                history.undo(&mut lines).unwrap();
            }
            UndoAction::Reminder((before, _)) => history.set_marks(before),
        }
        policy.complete_undo();
        assert_eq!(lines, [text]);
        assert_eq!(*history.current_marks(), marks);
    }
    for (text, marks) in [("a", 0), ("a", 1), ("ab", 1)] {
        match policy.redo_action().cloned().expect("redo action") {
            UndoAction::Text => {
                history.redo(&mut lines).unwrap();
            }
            UndoAction::Reminder((_, after)) => history.set_marks(after),
        }
        policy.complete_redo();
        assert_eq!(lines, [text]);
        assert_eq!(*history.current_marks(), marks);
    }
    assert!(policy.redo_action().is_none());
}

#[test]
fn unacknowledged_reminder_stays_pending_and_clear_resets_both_directions() {
    let mut policy = UndoPolicy::<&str>::default();
    policy.record_reminder("reminder");
    let action = policy.undo_action().cloned();
    // A host-side failure leaves the same action available for retry.
    assert_eq!(policy.undo_action().cloned(), action);
    assert_eq!(policy.undo_depth(), 1);
    assert!(policy.redo_action().is_none());
    policy.complete_undo();
    assert_eq!(
        policy.redo_action(),
        Some(&UndoAction::Reminder("reminder"))
    );
    policy.clear();
    assert_eq!(policy.undo_depth(), 0);
    assert!(policy.undo_action().is_none());
    assert!(policy.redo_action().is_none());
}

#[test]
fn large_note_span_recording_keeps_one_step_per_edit() {
    let mut lines = vec!["plain".to_string(); COALESCE_ANCHOR_MAX_LINES + 1];
    let mut history = LineHistory::new(8, &lines, 4000, 0, ());
    let mut policy = UndoPolicy::<()>::default();
    let delta = EditDelta {
        start_line: 4000,
        old_span: 1,
        new_span: 1,
    };
    let grouping = UndoGrouping {
        session: UndoSession::Insert,
        elapsed: Duration::ZERO,
    };
    for text in ["first", "second"] {
        lines[4000] = text.to_string();
        policy.record_text(
            &mut history,
            &lines,
            HistoryCursor {
                line: 4000,
                col: text.len(),
            },
            Some(delta),
            grouping,
        );
        let delta = history.take_last_delta().expect("span delta");
        assert_eq!(delta.start, 4000);
        assert_eq!(delta.removed.len(), 1);
        assert_eq!(delta.inserted, [text]);
    }
    assert_eq!(history.undo_depth(), 2);
    assert_eq!(policy.undo_depth(), 2);
    history.undo(&mut lines).unwrap();
    policy.complete_undo();
    assert_eq!(lines[4000], "first");
    history.undo(&mut lines).unwrap();
    policy.complete_undo();
    assert_eq!(lines[4000], "plain");
}

#[test]
fn retention_keeps_new_text_actions_after_interleaved_reminders() {
    let mut lines = vec![String::new()];
    let mut history = LineHistory::new(2, &lines, 0, 0, ());
    let mut policy = UndoPolicy::<&str>::default();
    record(&mut policy, &mut history, "a", 1000, UndoSession::Other);
    policy.record_reminder("reminder");
    history.set_marks(());
    record(&mut policy, &mut history, "ab", 0, UndoSession::Insert);
    history.break_coalescing();
    record(&mut policy, &mut history, "abc", 0, UndoSession::Insert);
    lines[0] = "abc".into();
    for expected in ["ab", "a"] {
        assert_eq!(policy.undo_action(), Some(&UndoAction::Text));
        history.undo(&mut lines).expect("retained text step");
        policy.complete_undo();
        assert_eq!(lines, [expected]);
    }
    assert_eq!(
        policy.undo_action(),
        Some(&UndoAction::Reminder("reminder"))
    );
    policy.complete_undo();
    assert!(policy.undo_action().is_none());
    assert_eq!(history.undo_depth(), 0);
}

#[test]
fn retention_aligns_large_note_span_markers_and_redo() {
    let mut lines = vec!["plain".to_string(); COALESCE_ANCHOR_MAX_LINES + 1];
    let mut history = LineHistory::new(2, &lines, 0, 0, ());
    let mut policy = UndoPolicy::<&str>::default();
    let grouping = UndoGrouping {
        session: UndoSession::Insert,
        elapsed: Duration::ZERO,
    };
    let delta = EditDelta {
        start_line: 0,
        old_span: 1,
        new_span: 1,
    };
    for text in ["a", "ab", "abc"] {
        lines[0] = text.into();
        policy.record_text(
            &mut history,
            &lines,
            HistoryCursor {
                line: 0,
                col: text.len(),
            },
            Some(delta),
            grouping,
        );
        if text == "a" {
            policy.record_reminder("reminder");
            history.set_marks(());
        }
    }
    for text in ["ab", "a"] {
        assert_eq!(policy.undo_action(), Some(&UndoAction::Text));
        history.undo(&mut lines).unwrap();
        policy.complete_undo();
        assert_eq!(lines[0], text);
    }
    assert_eq!(
        policy.undo_action(),
        Some(&UndoAction::Reminder("reminder"))
    );
    policy.complete_undo();
    assert!(policy.undo_action().is_none());
    policy.complete_redo(); // Host reapplies the reminder.
    for text in ["ab", "abc"] {
        assert_eq!(policy.redo_action(), Some(&UndoAction::Text));
        history.redo(&mut lines).unwrap();
        policy.complete_redo();
        assert_eq!(lines[0], text);
    }
    assert!(policy.redo_action().is_none());
}
