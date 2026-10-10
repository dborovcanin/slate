use super::*;
use crate::editor_core::types::{EditOperation, TextChange};

const AT: i64 = 1_900_000_000_000;

/// `body` in normal mode with stored reminders on the given 0-based lines;
/// line `n`'s reminder is due at `AT + n` and displayed as `dn`.
fn app_with_reminders(body: &str, lines: &[usize]) -> (Db, TerminalApp, PathBuf) {
    let (db, mut app, path) = app_with_note(body);
    for &line in lines {
        let text = body.lines().nth(line).expect("line");
        db.upsert_reminder(
            "n1",
            line as i64 + 1,
            AT + line as i64,
            &format!("d{line}"),
            text,
        )
        .expect("reminder");
    }
    app.load_reminders(&db).expect("reminders");
    let marks = app.reminder_marks();
    app.session.history_mut_for_tests().set_marks(marks);
    app.autosave_enabled = false;
    app.mode = UiMode::Normal;
    (db, app, path)
}

/// Reminders shown: (line, due, display, notified), by line.
fn shown(app: &TerminalApp) -> Vec<(usize, i64, String, Option<i64>)> {
    let mut rows: Vec<_> = app
        .session
        .reminders()
        .iter()
        .map(|(line, ghost)| {
            (
                *line,
                ghost.remind_at_ms,
                ghost.display_at.clone(),
                ghost.reminded_at_ms,
            )
        })
        .collect();
    rows.sort();
    rows
}

/// Reminders stored: (line number, due, line text, notified), by line.
fn stored(db: &Db) -> Vec<(i64, i64, String, Option<i64>)> {
    db.list_reminders("n1")
        .expect("list")
        .into_iter()
        .map(|r| (r.line_number, r.remind_at_ms, r.line_text, r.reminded_at_ms))
        .collect()
}

fn reminder(line: usize) -> (usize, i64, String, Option<i64>) {
    (line, AT + line as i64, format!("d{line}"), None)
}

fn moved(from: usize, to: usize) -> (usize, i64, String, Option<i64>) {
    (to, AT + from as i64, format!("d{from}"), None)
}

fn command(app: &mut TerminalApp, db: &Db, text: &str) {
    app.mode = UiMode::Normal;
    app.command_bar_from_normal = true;
    app.execute_terminal_command(db, text);
    app.mode = UiMode::Normal;
}

fn replace_text(app: &mut TerminalApp, from: usize, to: usize, insert: &str) {
    app.apply_edit_operation(&EditOperation {
        changes: vec![TextChange {
            from,
            to,
            insert: insert.to_string(),
        }],
        selection: None,
    });
}

const DD: [Key; 2] = [Key::Char('d'), Key::Char('d')];

#[test]
fn a_deleted_task_stays_deleted_after_save_and_reload() {
    let (db, mut app, path) = app_with_reminders("- [ ] buy milk\n- [ ] buy eggs", &[0]);
    run_keys(&mut app, &db, &DD);
    assert_eq!(shown(&app), vec![]);
    app.save(&db).expect("save");
    assert_eq!(stored(&db), vec![]);
    app.reload_active_note(&db).expect("reload");
    assert_eq!(app.editor.lines(), vec!["- [ ] buy eggs".to_string()]);
    assert_eq!(shown(&app), vec![]);

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn backward_word_delete_preserves_unicode_reminders_and_physical_line_joins() {
    let (db, mut app, path) = app_with_reminders("αβ_γ !!\nhidden\n界", &[0, 1, 2]);
    app.mode = UiMode::Editor;
    app.editor.cursor_col = 7;
    assert!(app.delete_word_backward());
    assert_eq!(app.editor.lines()[0], "αβ_");
    assert_eq!(app.editor.cursor_col, 3);
    assert_eq!(shown(&app), vec![reminder(0), reminder(1), reminder(2)]);
    app.undo(&db);
    assert_eq!(app.editor.lines()[0], "αβ_γ !!");
    app.session.break_undo_coalescing();
    app.editor.cursor_line = 2;
    app.editor.cursor_col = 0;
    app.folds.visible_to_real = vec![0, 2];
    app.folds.real_to_visible = vec![0, 0, 1];
    assert!(app.delete_word_backward());
    assert_eq!(app.editor.lines(), vec!["αβ_γ !!", "hidden界"]);
    assert_eq!(shown(&app), vec![reminder(0), reminder(1)]);
    app.undo(&db);
    assert_eq!(app.editor.lines(), vec!["αβ_γ !!", "hidden", "界"]);
    assert_eq!(shown(&app), vec![reminder(0), reminder(1), reminder(2)]);
    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn undo_and_redo_restore_reminders_by_identity() {
    let (db, mut app, path) = app_with_reminders("- [ ] buy milk\n- [ ] buy eggs", &[0]);
    run_keys(&mut app, &db, &DD);
    assert_eq!(shown(&app), vec![]);
    run_keys(&mut app, &db, &[Key::Char('u')]);
    assert_eq!(shown(&app), vec![reminder(0)]);
    run_keys(&mut app, &db, &[Key::Ctrl('r')]);
    assert_eq!(shown(&app), vec![], "redo does not move it onto buy eggs");
    run_keys(&mut app, &db, &[Key::Char('u')]);
    assert_eq!(shown(&app), vec![reminder(0)]);

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn reminders_follow_edits_with_math_off() {
    let (db, mut app, path) = app_with_reminders("- [ ] buy milk\n- [ ] buy eggs", &[0]);
    command(&mut app, &db, "module math off");
    assert!(!app.active_note.modules.math);
    run_keys(&mut app, &db, &DD);
    assert_eq!(shown(&app), vec![]);

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn a_replaced_task_list_drops_the_reminder_of_the_removed_task() {
    let body = "- [ ] buy milk\n- [ ] buy eggs";
    let (db, mut app, path) = app_with_reminders(body, &[0]);
    replace_text(&mut app, 0, body.len(), "- [ ] buy eggs\n- [ ] buy bread");
    assert_eq!(shown(&app), vec![]);

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn a_deleted_reminder_never_attaches_to_an_identical_task() {
    let (db, mut app, path) = app_with_reminders("- [ ] buy milk\nx\n- [ ] buy milk", &[0]);
    run_keys(&mut app, &db, &DD);
    assert_eq!(shown(&app), vec![]);
    app.save(&db).expect("save");
    app.reload_active_note(&db).expect("reload");
    assert_eq!(shown(&app), vec![]);

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn replacing_a_line_beside_its_duplicate_keeps_the_reminder_on_it() {
    let (db, mut app, path) = app_with_reminders("- [ ] buy milk\n- [ ] buy milk", &[0]);
    replace_text(&mut app, 0, 14, "call Ana");
    assert_eq!(shown(&app), vec![reminder(0)]);
    app.save(&db).expect("save");
    assert_eq!(stored(&db), vec![(1, AT, "call Ana".to_string(), None)]);

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn splitting_a_line_keeps_the_reminder_with_its_start() {
    let (db, mut app, path) = app_with_reminders("abc def", &[0]);
    app.editor.cursor_col = 3;
    run_keys(&mut app, &db, &[Key::Char('i'), Key::Enter, Key::Esc]);
    assert_eq!(
        app.editor.lines(),
        vec!["abc".to_string(), " def".to_string()]
    );
    assert_eq!(shown(&app), vec![reminder(0)]);

    // Enter at the start pushes the line, and its reminder, down.
    app.editor.cursor_line = 0;
    app.editor.cursor_col = 0;
    run_keys(&mut app, &db, &[Key::Char('i'), Key::Enter, Key::Esc]);
    assert_eq!(shown(&app), vec![moved(0, 1)]);

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn a_join_keeps_the_first_reminder_and_undo_brings_back_the_second() {
    let (db, mut app, path) = app_with_reminders("call Ana\nabout lunch\nend", &[0, 1]);
    app.editor.cursor_line = 1;
    app.editor.cursor_col = 0;
    run_keys(&mut app, &db, &[Key::Char('i'), Key::Backspace, Key::Esc]);
    assert_eq!(app.editor.lines()[0], "call Anaabout lunch");
    assert_eq!(shown(&app), vec![reminder(0)]);
    run_keys(&mut app, &db, &[Key::Char('u')]);
    assert_eq!(shown(&app), vec![reminder(0), reminder(1)]);

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn backspace_join_moves_a_lone_reminder_onto_the_joined_line() {
    let (db, mut app, path) = app_with_reminders("call Ana\nabout lunch", &[1]);
    app.editor.cursor_line = 1;
    app.editor.cursor_col = 0;
    run_keys(&mut app, &db, &[Key::Char('i'), Key::Backspace, Key::Esc]);
    assert_eq!(app.editor.lines(), vec!["call Anaabout lunch".to_string()]);
    assert_eq!(shown(&app), vec![moved(1, 0)]);

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn discarding_edits_keeps_the_stored_reminders() {
    let (db, mut app, path) = app_with_reminders("- [ ] buy milk\n- [ ] buy eggs", &[0]);
    run_keys(&mut app, &db, &DD);
    assert_eq!(shown(&app), vec![]);
    assert_eq!(stored(&db).len(), 1, "nothing stored before the text");
    assert!(!app.can_leave_note(&db), "unsaved changes are refused once");
    command(&mut app, &db, "e!");
    assert_eq!(app.editor.lines().len(), 2);
    assert_eq!(shown(&app), vec![reminder(0)]);
    assert_eq!(
        stored(&db),
        vec![(1, AT, "- [ ] buy milk".to_string(), None)]
    );

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn a_conflicting_save_stores_neither_text_nor_reminders() {
    let (db, mut app, path) = app_with_reminders("- [ ] buy milk\n- [ ] buy eggs", &[0]);
    run_keys(&mut app, &db, &DD);
    std::thread::sleep(Duration::from_millis(2));
    db.save_note("n1", "- [ ] buy milk\n- [ ] buy eggs\nelsewhere")
        .expect("external edit");
    let error = app.save(&db).expect_err("conflict");
    assert!(error.contains("changed since last load"), "{error}");
    assert_eq!(stored(&db).len(), 1);
    assert!(app.reminders_unsaved());

    app.save_with_options(&db, true).expect("forced save");
    assert_eq!(stored(&db), vec![]);
    assert!(!app.reminders_unsaved());

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn one_insert_session_is_one_undo_step_for_text_and_reminders() {
    let (db, mut app, path) = app_with_reminders("milk\nend", &[0]);
    run_keys(
        &mut app,
        &db,
        &[Key::Char('i'), Key::Enter, Key::Char('x'), Key::Esc],
    );
    assert_eq!(app.editor.lines(), vec!["", "xmilk", "end"]);
    assert_eq!(shown(&app), vec![moved(0, 1)]);
    run_keys(&mut app, &db, &[Key::Char('u')]);
    assert_eq!(app.editor.lines(), vec!["milk", "end"]);
    assert_eq!(shown(&app), vec![reminder(0)]);

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn a_merged_edit_that_cancels_itself_keeps_history_aligned() {
    let (db, mut app, path) = app_with_reminders("a\nmilk\nc", &[1]);
    run_keys(&mut app, &db, &DD);
    assert_eq!(shown(&app), vec![moved(1, 0)]);
    // Typing and erasing a character on the reminded line, where the step
    // began: the merged step cancels itself and is dropped.
    app.editor.cursor_col = 0;
    let depth = app.session.history().undo_depth();
    run_keys(&mut app, &db, &[Key::Char('i'), Key::Char('x')]);
    assert_eq!(app.session.history().undo_depth(), depth + 1);
    run_keys(&mut app, &db, &[Key::Backspace]);
    assert_eq!(
        app.session.history().undo_depth(),
        depth,
        "the step cancelled itself"
    );
    run_keys(&mut app, &db, &[Key::Esc]);
    assert_eq!(app.editor.lines(), vec!["milk", "c"]);
    assert_eq!(shown(&app), vec![moved(1, 0)]);
    run_keys(&mut app, &db, &[Key::Char('u')]);
    assert_eq!(app.editor.lines(), vec!["a", "milk", "c"]);
    assert_eq!(shown(&app), vec![reminder(1)]);

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn a_new_edit_after_undo_drops_the_redo_branch_for_reminders_too() {
    let (db, mut app, path) = app_with_reminders("milk\nend", &[0]);
    run_keys(&mut app, &db, &DD);
    run_keys(&mut app, &db, &[Key::Char('u')]);
    assert_eq!(shown(&app), vec![reminder(0)]);
    app.editor.cursor_line = 1;
    run_keys(&mut app, &db, &[Key::Char('o'), Key::Char('y'), Key::Esc]);
    run_keys(&mut app, &db, &[Key::Ctrl('r')]);
    assert_eq!(app.editor.lines(), vec!["milk", "end", "y"]);
    assert_eq!(shown(&app), vec![reminder(0)]);

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn reminder_changes_interleaved_with_text_undo_in_order() {
    let (db, mut app, path) = app_with_reminders("a\nb\nc", &[0]);
    // T1: delete line c.
    app.editor.cursor_line = 2;
    run_keys(&mut app, &db, &DD);
    // R: remove line a's reminder.
    app.editor.cursor_line = 0;
    command(&mut app, &db, "remind toggle");
    assert_eq!(shown(&app), vec![]);
    // T2: delete line a.
    run_keys(&mut app, &db, &DD);
    assert_eq!(app.editor.lines(), vec!["b"]);

    run_keys(&mut app, &db, &[Key::Char('u')]);
    assert_eq!(app.editor.lines(), vec!["a", "b"]);
    assert_eq!(shown(&app), vec![], "T2's undo keeps R");
    run_keys(&mut app, &db, &[Key::Char('u')]);
    assert_eq!(shown(&app), vec![reminder(0)], "R's undo restores it");
    run_keys(&mut app, &db, &[Key::Char('u')]);
    assert_eq!(app.editor.lines(), vec!["a", "b", "c"]);
    assert_eq!(shown(&app), vec![reminder(0)]);

    for _ in 0..3 {
        run_keys(&mut app, &db, &[Key::Ctrl('r')]);
    }
    assert_eq!(app.editor.lines(), vec!["b"]);
    assert_eq!(shown(&app), vec![]);

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn notification_state_moves_and_saves_with_the_reminder() {
    let (db, mut app, path) = app_with_reminders("milk\nend", &[0]);
    app.session
        .reminders_mut_for_tests()
        .get_mut(&0)
        .expect("ghost")
        .reminded_at_ms = Some(5);
    app.reminders_changed_outside_text(&db);
    assert_eq!(
        stored(&db),
        vec![(1, AT, "milk".to_string(), Some(5))],
        "stored at once while the text is saved"
    );
    run_keys(&mut app, &db, &[Key::Char('O'), Key::Char('x'), Key::Esc]);
    assert_eq!(shown(&app), vec![(1, AT, "d0".to_string(), Some(5))]);
    app.save(&db).expect("save");
    assert_eq!(stored(&db), vec![(2, AT, "milk".to_string(), Some(5))]);

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn an_edit_beside_an_identical_task_keeps_the_reminder_after_reload() {
    let (db, mut app, path) = app_with_reminders("- [ ] buy milk\n- [ ] buy milk", &[0]);
    run_keys(&mut app, &db, &[Key::Char('A'), Key::Char('!'), Key::Esc]);
    app.save(&db).expect("save");
    assert_eq!(
        stored(&db),
        vec![(1, AT, "- [ ] buy milk!".to_string(), None)]
    );
    app.reload_active_note(&db).expect("reload");
    assert_eq!(shown(&app), vec![reminder(0)]);

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn deleting_the_only_line_deletes_its_reminder() {
    let (db, mut app, path) = app_with_reminders("buy milk", &[0]);
    run_keys(&mut app, &db, &DD);
    assert_eq!(app.editor.lines(), vec!["".to_string()]);
    assert_eq!(shown(&app), vec![]);
    run_keys(&mut app, &db, &[Key::Char('u')]);
    assert_eq!(shown(&app), vec![reminder(0)]);

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn changing_a_line_keeps_its_reminder() {
    let (db, mut app, path) = app_with_reminders("buy milk\nbuy eggs", &[0]);
    run_keys(
        &mut app,
        &db,
        &[
            Key::Char('c'),
            Key::Char('c'),
            Key::Char('t'),
            Key::Char('e'),
            Key::Char('a'),
            Key::Esc,
        ],
    );
    assert_eq!(app.editor.lines(), vec!["tea", "buy eggs"]);
    assert_eq!(shown(&app), vec![reminder(0)]);

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn reminders_deferred_at_startup_load_before_the_first_edit() {
    let (db, mut app, path) = app_with_reminders("- [ ] buy milk\n- [ ] buy eggs", &[0]);
    // As at startup with background tasks: not loaded yet.
    app.session.reminders_mut_for_tests().clear();
    let marks = app.reminder_marks();
    app.session.history_mut_for_tests().set_marks(marks);
    app.startup_reminder_hydration_pending = true;
    run_keys(&mut app, &db, &DD);
    assert_eq!(shown(&app), vec![]);
    app.save(&db).expect("save");
    app.reload_active_note(&db).expect("reload");
    assert_eq!(shown(&app), vec![], "milk's reminder is not given to eggs");

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn a_stale_session_cannot_write_over_reminder_changes() {
    let (db, mut app, path) = app_with_reminders("milk\nend", &[0]);
    let stale_revision = app.session.stored_revision().to_owned();
    // This session removes the reminder; the text is saved, so it is stored.
    command(&mut app, &db, "remind toggle");
    assert_eq!(stored(&db), vec![]);
    assert_ne!(
        app.session.stored_revision(),
        stale_revision,
        "the revision moved"
    );
    // Another session still holding the old revision is refused.
    let error = db
        .replace_reminders_if("n1", Some(&stale_revision), &[])
        .expect_err("stale");
    assert!(error.contains("changed since last load"), "{error}");
    // This session's next save is no conflict.
    run_keys(&mut app, &db, &[Key::Char('A'), Key::Char('!'), Key::Esc]);
    app.save(&db).expect("save");

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn deleting_one_of_two_identical_lines_keeps_the_others_reminder() {
    let (db, mut app, path) = app_with_reminders("- [ ] buy milk\n- [ ] buy milk", &[1]);
    // Linewise visual delete of the first line.
    run_keys(&mut app, &db, &[Key::Char('V'), Key::Char('d')]);
    assert_eq!(app.editor.lines(), vec!["- [ ] buy milk".to_string()]);
    assert_eq!(shown(&app), vec![moved(1, 0)]);
    // dd says the same.
    run_keys(&mut app, &db, &[Key::Char('u')]);
    assert_eq!(shown(&app), vec![reminder(1)]);
    app.editor.cursor_line = 0;
    run_keys(&mut app, &db, &DD);
    assert_eq!(shown(&app), vec![moved(1, 0)]);
    // Deleting the reminded second line with V d drops only its reminder.
    run_keys(&mut app, &db, &[Key::Char('u')]);
    app.editor.cursor_line = 1;
    run_keys(&mut app, &db, &[Key::Char('V'), Key::Char('d')]);
    assert_eq!(shown(&app), vec![]);

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn reminders_follow_text_reloaded_after_an_outside_change() {
    let (db, mut app, path) = app_with_reminders("a\nb\nc", &[1]);
    app.session.acknowledge_locked_revision(
        db.get_note_updated_at("n1")
            .expect("revision")
            .expect("note"),
    );
    std::thread::sleep(std::time::Duration::from_millis(2));
    db.save_note("n1", "new\na\nb\nc").expect("outside edit");

    app.outside_change_checked_at = Instant::now() - std::time::Duration::from_secs(2);
    app.maybe_take_outside_change(&db);
    assert_eq!(app.editor.lines(), ["new", "a", "b", "c"]);
    assert_eq!(shown(&app), vec![moved(1, 2)]);

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn compound_text_edits_preserve_exact_reminder_mapping_and_one_undo_step() {
    let original = "é task\nβ task\nend";
    let (db, mut app, path) = app_with_reminders(original, &[0, 1, 2]);
    // The operation uses pre-operation byte offsets; the host applies it
    // back to front. One split is mid-line, the other at column zero.
    app.apply_edit_operation(&EditOperation {
        changes: vec![
            TextChange {
                from: 2,
                to: 2,
                insert: "\n".into(),
            },
            TextChange {
                from: 8,
                to: 8,
                insert: "\n".into(),
            },
        ],
        selection: None,
    });
    assert_eq!(app.editor.lines().join("\n"), "é\n task\n\nβ task\nend");
    assert_eq!(shown(&app), vec![reminder(0), moved(1, 3), moved(2, 4)]);
    run_keys(&mut app, &db, &[Key::Char('u')]);
    assert_eq!(app.editor.lines().join("\n"), original);
    assert_eq!(shown(&app), vec![reminder(0), reminder(1), reminder(2)]);
    run_keys(&mut app, &db, &[Key::Ctrl('r')]);
    assert_eq!(app.editor.lines().join("\n"), "é\n task\n\nβ task\nend");
    assert_eq!(shown(&app), vec![reminder(0), moved(1, 3), moved(2, 4)]);
    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn primitive_unicode_edits_preserve_reminders_and_undo_boundaries() {
    let (db, mut app, path) = app_with_reminders("é β\nlast", &[0, 1]);
    app.mode = UiMode::Editor;
    app.editor.cursor_col = 1;
    app.insert_char('λ');
    app.insert_text("猫");
    app.backspace();
    app.delete_forward();
    assert_eq!(app.editor.lines().join("\n"), "éλβ\nlast");
    assert_eq!(app.editor.cursor_col, 2);
    assert_eq!(shown(&app), vec![reminder(0), reminder(1)]);
    app.insert_newline();
    assert_eq!(app.editor.lines().join("\n"), "éλ\nβ\nlast");
    assert_eq!(shown(&app), vec![reminder(0), moved(1, 2)]);
    run_keys(&mut app, &db, &[Key::Esc, Key::Char('u')]);
    assert_eq!(app.editor.lines().join("\n"), "é β\nlast");
    assert_eq!(shown(&app), vec![reminder(0), reminder(1)]);
    run_keys(&mut app, &db, &[Key::Ctrl('r')]);
    assert_eq!(app.editor.lines().join("\n"), "éλ\nβ\nlast");
    assert_eq!(shown(&app), vec![reminder(0), moved(1, 2)]);
    // Both join directions retain the first line's reminder and move later marks.
    app.backspace();
    assert_eq!(app.editor.lines().join("\n"), "éλβ\nlast");
    assert_eq!(app.editor.cursor_col, 2);
    app.editor.cursor_col = 3;
    app.delete_forward();
    assert_eq!(app.editor.lines().join("\n"), "éλβlast");
    assert_eq!(shown(&app), vec![reminder(0)]);
    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn pasted_line_endings_and_unicode_preserve_reminder_identity_and_one_undo_step() {
    for column in [0, 1] {
        let original = "é task\nlast";
        let (db, mut app, path) = app_with_reminders(original, &[0, 1]);
        app.mode = UiMode::Editor;
        app.editor.cursor_col = column;
        app.insert_paste("猫\r\nβ\r");
        let expected = if column == 0 {
            "猫\nβ\né task\nlast"
        } else {
            "é猫\nβ\n task\nlast"
        };
        assert_eq!(app.editor.lines().join("\n"), expected);
        assert_eq!((app.editor.cursor_line, app.editor.cursor_col), (2, 0));
        let marks = vec![moved(0, if column == 0 { 2 } else { 0 }), moved(1, 3)];
        assert_eq!(shown(&app), marks);
        run_keys(&mut app, &db, &[Key::Esc, Key::Char('u')]);
        assert_eq!(app.editor.lines().join("\n"), original);
        assert_eq!(shown(&app), vec![reminder(0), reminder(1)]);
        run_keys(&mut app, &db, &[Key::Ctrl('r')]);
        assert_eq!(app.editor.lines().join("\n"), expected);
        assert_eq!(shown(&app), marks);
        drop(app);
        drop(db);
        cleanup_db_files(&path);
    }
}

#[test]
fn table_cell_paste_preserves_block_reminder_mapping_and_undo() {
    let mut lines = crate::editor_core::table::format_table_lines(&[
        "| A | B |".into(),
        "| --- | --- |".into(),
        "| xy | z |".into(),
        "| keep | z |".into(),
    ]);
    lines.push("end".into());
    let original = lines.join("\n");
    let (db, mut app, path) = app_with_reminders(&original, &[0, 3, 4]);
    app.mode = UiMode::Editor;
    app.editor.cursor_line = 2;
    app.editor.cursor_col = app.editor.lines()[2].find('y').unwrap();
    app.insert_paste("1\r\n2");
    assert_eq!(
        crate::editor_core::table::split_table_cells(&app.editor.lines()[2])[0],
        "x1"
    );
    assert_eq!(
        crate::editor_core::table::split_table_cells(&app.editor.lines()[3])[0],
        "2y"
    );
    assert_eq!(shown(&app), vec![reminder(0), moved(3, 4), moved(4, 5)]);
    let pasted = app.editor.lines().to_vec();
    run_keys(&mut app, &db, &[Key::Esc, Key::Char('u')]);
    assert_eq!(app.editor.lines().join("\n"), original);
    assert_eq!(shown(&app), vec![reminder(0), reminder(3), reminder(4)]);
    run_keys(&mut app, &db, &[Key::Ctrl('r')]);
    assert_eq!(app.editor.lines(), pasted);
    assert_eq!(shown(&app), vec![reminder(0), moved(3, 4), moved(4, 5)]);
    drop(app);
    drop(db);
    cleanup_db_files(&path);
}
