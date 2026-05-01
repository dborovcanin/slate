use super::*;
use crate::terminal::app::WikiLinkSuggestion;

#[test]
fn apply_edit_operation_single_line_change_updates_in_place() {
    let (_db, mut app, path) = app_with_note("alpha\nbeta");
    app.cursor_line = 0;
    app.cursor_col = 2;

    let op = crate::editor_core::types::EditOperation {
        changes: vec![crate::editor_core::types::TextChange {
            from: 1,
            to: 4,
            insert: "XYZ".to_string(),
        }],
        selection: Some(crate::editor_core::types::OperationSelection {
            anchor: 3,
            head: Some(3),
        }),
    };
    app.apply_edit_operation(&op);

    assert_eq!(app.lines, vec!["aXYZa".to_string(), "beta".to_string()]);
    assert_eq!(app.cursor_line, 0);
    assert_eq!(app.cursor_col, 3);

    drop(app);
    cleanup_db_files(&path);
}

#[test]
fn apply_edit_operation_cross_line_change_still_merges_lines() {
    let (_db, mut app, path) = app_with_note("alpha\nbeta");
    app.cursor_line = 1;
    app.cursor_col = 0;

    let op = crate::editor_core::types::EditOperation {
        changes: vec![crate::editor_core::types::TextChange {
            from: 5,
            to: 6,
            insert: String::new(),
        }],
        selection: Some(crate::editor_core::types::OperationSelection {
            anchor: 5,
            head: Some(5),
        }),
    };
    app.apply_edit_operation(&op);

    assert_eq!(app.lines, vec!["alphabeta".to_string()]);
    assert_eq!(app.cursor_line, 0);
    assert_eq!(app.cursor_col, 5);

    drop(app);
    cleanup_db_files(&path);
}

#[test]
fn apply_edit_operation_multiline_insert_updates_lines_directly() {
    let (_db, mut app, path) = app_with_note("start end");
    app.cursor_line = 0;
    app.cursor_col = 6;

    let op = crate::editor_core::types::EditOperation {
        changes: vec![crate::editor_core::types::TextChange {
            from: 6,
            to: 6,
            insert: "a\nb\n".to_string(),
        }],
        selection: Some(crate::editor_core::types::OperationSelection {
            anchor: 10,
            head: Some(10),
        }),
    };
    app.apply_edit_operation(&op);

    assert_eq!(
        app.lines,
        vec!["start a".to_string(), "b".to_string(), "end".to_string()]
    );
    assert_eq!(app.cursor_line, 2);
    assert_eq!(app.cursor_col, 0);

    drop(app);
    cleanup_db_files(&path);
}

#[test]
fn apply_edit_operation_multi_change_updates_lines_without_full_rebuild() {
    let (_db, mut app, path) = app_with_note("alpha\nbeta\ngamma");
    app.cursor_line = 1;
    app.cursor_col = 2;

    let op = crate::editor_core::types::EditOperation {
        changes: vec![
            crate::editor_core::types::TextChange {
                from: 0,
                to: 1,
                insert: "A".to_string(),
            },
            crate::editor_core::types::TextChange {
                from: 11,
                to: 16,
                insert: "G\nH".to_string(),
            },
        ],
        selection: Some(crate::editor_core::types::OperationSelection {
            anchor: 13,
            head: Some(13),
        }),
    };
    app.apply_edit_operation(&op);

    assert_eq!(
        app.lines,
        vec![
            "Alpha".to_string(),
            "beta".to_string(),
            "G".to_string(),
            "H".to_string(),
        ]
    );
    assert_eq!(app.cursor_line, 3);
    assert_eq!(app.cursor_col, 0);

    drop(app);
    cleanup_db_files(&path);
}

#[test]
fn editor_paste_multiline_inserts_as_single_bulk_edit() {
    let (db, mut app, path) = app_with_note("start end");
    app.mode = UiMode::Editor;
    app.cursor_line = 0;
    app.cursor_col = 6; // after "start "

    app.handle_editor_key(&db, Key::Paste("a\nb\n".to_string()))
        .expect("paste applies");

    assert_eq!(
        app.lines,
        vec!["start a".to_string(), "b".to_string(), "end".to_string()]
    );
    assert_eq!(app.cursor_line, 2);
    assert_eq!(app.cursor_col, 0);

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn ctrl_q_quits_from_normal_mode() {
    let (db, mut app, path) = app_with_note("one\ntwo\nthree");
    app.mode = UiMode::Normal;

    run_keys(&mut app, &db, &[Key::Ctrl('q')]);
    assert!(app.quit);

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn write_command_saves_active_note_without_quit() {
    let (db, mut app, path) = app_with_note("one");
    app.mode = UiMode::Normal;
    app.command_bar_from_normal = true;
    app.lines = vec!["one updated".to_string()];
    app.dirty = true;

    app.execute_terminal_command(&db, "w");

    assert_eq!(app.status, "written");
    assert!(!app.quit);
    assert!(!app.dirty);
    let persisted = db
        .get_note("n1")
        .expect("note lookup")
        .expect("note exists");
    assert_eq!(persisted.body, "one updated");

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn write_command_syncs_markdown_file_backed_note() {
    let db_path = temp_db_path();
    let db = Db::open(db_path.clone()).expect("db opens");
    let markdown_path =
        std::env::temp_dir().join(format!("note-terminal-mdfile-{}.md", Ulid::new()));
    fs::write(&markdown_path, "file body").expect("markdown seed");
    let note_id = crate::note_id_for_markdown_file(&markdown_path);
    let opts = TerminalOptions {
        create_new: false,
        note_id: Some(note_id.clone()),
        list_only: false,
    };
    let (mut app, _) = TerminalApp::new_with_startup_metrics(
        &db,
        &opts,
        true,
        true,
        false,
        true,
        true,
        3,
        crate::terminal::render::RenderPalette::default(),
        "%Y-%m-%d".to_string(),
        "%Y-%m-%d %H:%M".to_string(),
    )
    .expect("terminal app");
    assert_eq!(app.lines, vec!["file body".to_string()]);
    app.mode = UiMode::Normal;
    app.command_bar_from_normal = true;
    app.lines = vec!["updated body".to_string()];
    app.dirty = true;

    app.execute_terminal_command(&db, "w");

    assert_eq!(app.status, "written");
    assert!(!app.quit);
    assert!(!app.dirty);
    assert_eq!(
        fs::read_to_string(&markdown_path).expect("markdown read"),
        "updated body"
    );
    let persisted = db.get_note(&note_id).expect("note lookup");
    assert!(persisted.is_none());

    drop(app);
    drop(db);
    let _ = fs::remove_file(&markdown_path);
    cleanup_db_files(&db_path);
}

#[test]
fn write_command_detects_conflict_and_w_bang_forces_db_save() {
    let (db, mut app, path) = app_with_note("one");
    app.mode = UiMode::Normal;
    app.command_bar_from_normal = true;
    app.lines = vec!["local body".to_string()];
    app.dirty = true;

    db.save_note("n1", "external body")
        .expect("external save should succeed");

    app.execute_terminal_command(&db, "w");
    assert!(app.status.contains("use :w!"));
    assert!(app.dirty);
    let persisted = db
        .get_note("n1")
        .expect("note lookup")
        .expect("note exists");
    assert_eq!(persisted.body, "external body");

    app.execute_terminal_command(&db, "w!");
    assert_eq!(app.status, "written");
    assert!(!app.dirty);
    let persisted_forced = db
        .get_note("n1")
        .expect("note lookup")
        .expect("note exists");
    assert_eq!(persisted_forced.body, "local body");

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn write_command_detects_conflict_and_w_bang_forces_file_save() {
    let db_path = temp_db_path();
    let db = Db::open(db_path.clone()).expect("db opens");
    let markdown_path =
        std::env::temp_dir().join(format!("note-terminal-mdfile-conflict-{}.md", Ulid::new()));
    fs::write(&markdown_path, "initial").expect("markdown seed");
    let note_id = crate::note_id_for_markdown_file(&markdown_path);
    let opts = TerminalOptions {
        create_new: false,
        note_id: Some(note_id),
        list_only: false,
    };
    let (mut app, _) = TerminalApp::new_with_startup_metrics(
        &db,
        &opts,
        true,
        true,
        false,
        true,
        true,
        3,
        crate::terminal::render::RenderPalette::default(),
        "%Y-%m-%d".to_string(),
        "%Y-%m-%d %H:%M".to_string(),
    )
    .expect("terminal app");
    app.mode = UiMode::Normal;
    app.command_bar_from_normal = true;
    app.lines = vec!["local body".to_string()];
    app.dirty = true;

    fs::write(&markdown_path, "external body").expect("external write");

    app.execute_terminal_command(&db, "w");
    assert!(app.status.contains("use :w!"));
    assert!(app.dirty);
    assert_eq!(
        fs::read_to_string(&markdown_path).expect("markdown read"),
        "external body"
    );

    app.execute_terminal_command(&db, "w!");
    assert_eq!(app.status, "written");
    assert!(!app.dirty);
    assert_eq!(
        fs::read_to_string(&markdown_path).expect("markdown read"),
        "local body"
    );

    drop(app);
    drop(db);
    let _ = fs::remove_file(&markdown_path);
    cleanup_db_files(&db_path);
}

#[test]
fn write_quit_command_saves_then_exits() {
    let (db, mut app, path) = app_with_note("one");
    app.mode = UiMode::Normal;
    app.command_bar_from_normal = true;
    app.lines = vec!["one updated".to_string()];
    app.dirty = true;

    app.execute_terminal_command(&db, "wq");

    assert_eq!(app.status, "written");
    assert!(app.quit);
    assert!(!app.dirty);
    let persisted = db
        .get_note("n1")
        .expect("note lookup")
        .expect("note exists");
    assert_eq!(persisted.body, "one updated");

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn autosave_disabled_only_write_command_persists_changes() {
    let (db, mut app, path) = app_with_note("one");
    app.mode = UiMode::Editor;
    app.lines = vec!["one updated".to_string()];
    app.dirty = true;
    app.autosave_enabled = false;
    app.last_edit =
        Instant::now() - Duration::from_millis(crate::terminal::app::AUTOSAVE_DEBOUNCE_MS + 5);

    app.maybe_autosave(&db)
        .expect("autosave-disabled idle tick should not fail");
    assert!(app.dirty);
    let persisted_before = db
        .get_note("n1")
        .expect("note lookup")
        .expect("note exists");
    assert_eq!(persisted_before.body, "one");

    app.handle_editor_key(&db, Key::Ctrl('s'))
        .expect("ctrl+s should not fail when autosave is disabled");
    assert_eq!(app.status, "autosave off; use :w");
    let persisted_after_ctrl_s = db
        .get_note("n1")
        .expect("note lookup")
        .expect("note exists");
    assert_eq!(persisted_after_ctrl_s.body, "one");

    app.mode = UiMode::Normal;
    app.command_bar_from_normal = true;
    app.execute_terminal_command(&db, "w");
    assert_eq!(app.status, "written");
    assert!(!app.dirty);
    let persisted_after_write = db
        .get_note("n1")
        .expect("note lookup")
        .expect("note exists");
    assert_eq!(persisted_after_write.body, "one updated");

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn ctrl_s_in_normal_mode_still_saves_when_autosave_is_disabled() {
    let (db, mut app, path) = app_with_note("one");
    app.mode = UiMode::Normal;
    app.lines = vec!["one updated".to_string()];
    app.dirty = true;
    app.autosave_enabled = false;

    app.handle_normal_key(&db, Key::Ctrl('s'))
        .expect("ctrl+s in normal mode should save");

    assert_eq!(app.status, "saved n1");
    assert!(!app.dirty);
    let persisted = db
        .get_note("n1")
        .expect("note lookup")
        .expect("note exists");
    assert_eq!(persisted.body, "one updated");

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn clip_watch_commands_toggle_terminal_watcher() {
    let (db, mut app, path) = app_with_note("alpha");
    app.mode = UiMode::Normal;

    app.execute_terminal_command(&db, "clip-watch");
    assert!(app.clipboard_watch_enabled);
    assert_eq!(app.status, "clip-watch started");

    app.execute_terminal_command(&db, "clip-watch");
    assert!(app.clipboard_watch_enabled);
    assert_eq!(app.status, "clip-watch already active");

    app.execute_terminal_command(&db, "clip-watch-stop");
    assert!(!app.clipboard_watch_enabled);
    assert_eq!(app.status, "clip-watch stopped");

    app.execute_terminal_command(&db, "clip-watch-stop");
    assert!(!app.clipboard_watch_enabled);
    assert_eq!(app.status, "clip-watch not active");

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn module_commands_update_and_persist_note_modules() {
    let (db, mut app, path) = app_with_note("alpha := 10\nalp");
    app.mode = UiMode::Editor;
    app.cursor_line = 1;
    app.cursor_col = line_char_len(app.current_line());
    app.refresh_variable_autocomplete_popup();
    assert!(app.variable_autocomplete_popup.visible);

    app.execute_terminal_command(&db, "module status");
    assert_eq!(app.status, "modules math=on table=on variables=on style=on");

    app.execute_terminal_command(&db, "modules variables off");
    assert_eq!(
        app.status,
        "modules math=on table=on variables=off style=on"
    );
    assert!(!app.active_note.modules.variables);
    assert!(!app.variable_autocomplete_popup.visible);
    assert!(app.calc.variable_names.is_empty());

    let persisted = db
        .get_note("n1")
        .expect("note lookup")
        .expect("note exists");
    assert!(!persisted.modules.variables);

    app.execute_terminal_command(&db, "module variables toggle");
    assert_eq!(app.status, "modules math=on table=on variables=on style=on");
    assert!(app.active_note.modules.variables);

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn module_math_toggle_disables_calc_tab_path() {
    let (db, mut app, path) = app_with_note("1 + 1");
    app.mode = UiMode::Editor;
    app.cursor_line = 0;
    app.cursor_col = line_char_len(app.current_line());

    app.execute_terminal_command(&db, "module math off");
    app.handle_editor_key(&db, Key::Tab)
        .expect("tab falls back when math module is off");
    assert_eq!(app.lines[0], "1 + 1  ");

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn module_table_toggle_disables_table_cursor_clamping() {
    let (db, mut app, path) = app_with_note("| aaa |     | bb  |");
    app.mode = UiMode::Editor;
    app.cursor_col = 9;

    app.execute_terminal_command(&db, "module table off");
    app.adjust_cursor();
    assert_eq!(app.cursor_col, 9);

    app.execute_terminal_command(&db, "module table on");
    app.cursor_col = 9;
    app.adjust_cursor();
    assert_eq!(app.cursor_col, 8);

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn module_style_toggle_disables_enter_autoformat_rules() {
    let (db, mut app, path) = app_with_note("- [ ] task");
    app.mode = UiMode::Editor;
    app.cursor_col = line_char_len(app.current_line());

    app.execute_terminal_command(&db, "module style off");
    app.handle_editor_key(&db, Key::Enter)
        .expect("enter uses plain newline when style module is off");
    assert_eq!(app.lines, vec!["- [ ] task".to_string(), String::new()]);

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn module_style_off_keeps_table_autoformat_when_table_module_is_on() {
    let (db, mut app, path) = app_with_note("| a | b |\n| --- | --- |\n|1|2|");
    app.mode = UiMode::Editor;
    app.cursor_line = 2;
    app.cursor_col = 4; // before trailing pipe in "|1|2|"

    app.execute_terminal_command(&db, "module style off");
    app.handle_editor_key(&db, Key::Char('0'))
        .expect("typing still triggers table autoformat");

    assert_ne!(app.lines[2], "|1|20|");
    assert!(app.lines[2].contains("20"));
    assert!(app.lines[2].starts_with("| "));

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn module_style_off_keeps_table_ctrl_navigation_when_table_module_is_on() {
    let (db, mut app, path) = app_with_note("| aaa | bb  |");
    app.mode = UiMode::Editor;
    app.cursor_col = 5; // first cell end anchor

    app.execute_terminal_command(&db, "module style off");

    app.handle_editor_key(&db, Key::CtrlArrowRight)
        .expect("ctrl-right jumps to next table cell");
    assert_eq!(app.cursor_col, 10);

    app.handle_editor_key(&db, Key::CtrlArrowLeft)
        .expect("ctrl-left jumps back to previous table cell");
    assert_eq!(app.cursor_col, 5);

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn note_unprotect_command_removes_lock() {
    let (db, mut app, path) = app_with_note("top secret");

    app.execute_terminal_command(&db, "note lock pass123");
    assert_eq!(app.active_note.access_mode, NoteAccessMode::Locked);
    assert_eq!(app.status, "note locked");

    app.execute_terminal_command(&db, "note unprotect pass123");
    assert_eq!(app.active_note.access_mode, NoteAccessMode::None);
    assert_eq!(app.lines, vec!["top secret".to_string()]);
    assert_eq!(app.status, "note unprotected");

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn note_unprotect_command_removes_at_rest_encryption() {
    let (db, mut app, path) = app_with_note("classified");

    app.execute_terminal_command(&db, "note encrypt enc123");
    assert_eq!(app.active_note.access_mode, NoteAccessMode::Encrypted);
    assert_eq!(app.status, "note encrypted at rest");

    app.execute_terminal_command(&db, "note unprotect enc123");
    assert_eq!(app.active_note.access_mode, NoteAccessMode::None);
    assert_eq!(app.lines, vec!["classified".to_string()]);
    assert_eq!(app.status, "note unprotected");

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn note_security_aliases_accept_password_arguments() {
    let (db, mut app, path) = app_with_note("top secret");

    app.execute_terminal_command(&db, "lock-note pass123");
    assert_eq!(app.active_note.access_mode, NoteAccessMode::Locked);
    assert!(!app.active_note.is_unlocked);
    assert_eq!(app.status, "note locked");

    app.execute_terminal_command(&db, "unlock-note pass123");
    assert_eq!(app.active_note.access_mode, NoteAccessMode::Locked);
    assert!(app.active_note.is_unlocked);
    assert_eq!(app.status, "note unlocked");

    app.execute_terminal_command(&db, "encrypt-note enc123");
    assert_eq!(app.active_note.access_mode, NoteAccessMode::Encrypted);
    assert!(app.active_note.is_unlocked);
    assert_eq!(app.status, "note encrypted at rest");

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn locked_notes_block_editor_mutations_and_autosave_errors() {
    let (db, mut app, path) = app_with_note("top secret");

    app.execute_terminal_command(&db, "note lock pass123");
    assert_eq!(app.active_note.access_mode, NoteAccessMode::Locked);
    assert!(!app.active_note.is_unlocked);
    assert_eq!(app.lines, vec![String::new()]);
    assert!(!app.dirty);

    app.handle_editor_key(&db, Key::Char('x'))
        .expect("locked edit should not fail");
    assert_eq!(app.lines, vec![String::new()]);
    assert!(!app.dirty);
    assert!(app.status.contains("unlock first"));

    app.dirty = true;
    app.last_edit =
        Instant::now() - Duration::from_millis(crate::terminal::app::AUTOSAVE_DEBOUNCE_MS + 5);
    app.maybe_autosave(&db)
        .expect("locked autosave should not terminate loop");
    assert!(app.status.contains("unlock first"));

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn switcher_delete_cancel_keeps_note() {
    let (db, mut app, path) = app_with_note("first note");
    db.save_note("n2", "second note")
        .expect("second note saved");
    app.refresh_switcher_items(&db)
        .expect("switcher items refreshed");

    run_keys(
        &mut app,
        &db,
        &[Key::Ctrl('p'), Key::Char('s'), Key::Delete],
    );
    assert_eq!(app.mode, UiMode::Switcher);
    assert!(app.switcher_delete_confirm.is_some());

    run_keys(&mut app, &db, &[Key::Char('n')]);
    assert!(app.switcher_delete_confirm.is_none());
    assert!(db.get_note("n2").expect("lookup works").is_some());

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn switcher_ctrl_backspace_delete_removes_active_after_confirmation() {
    let (db, mut app, path) = app_with_note("first note");
    db.save_note("n2", "second note")
        .expect("second note saved");
    app.refresh_switcher_items(&db)
        .expect("switcher items refreshed");

    run_keys(
        &mut app,
        &db,
        &[
            Key::Ctrl('p'),
            Key::Paste("first".to_string()),
            Key::CtrlBackspace,
        ],
    );
    assert_eq!(app.mode, UiMode::Switcher);
    let pending = app
        .switcher_delete_confirm
        .as_ref()
        .expect("delete confirmation requested");
    assert_eq!(pending.note_id, "n1");

    run_keys(&mut app, &db, &[Key::Enter]);
    assert!(app.switcher_delete_confirm.is_none());
    assert!(db.get_note("n1").expect("lookup works").is_none());
    assert_ne!(app.active_note.id, "n1");
    assert!(db
        .get_note(&app.active_note.id)
        .expect("active note lookup")
        .is_some());

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn switcher_enter_opens_note_in_normal_mode() {
    let (db, mut app, path) = app_with_note("first note");
    db.save_note("n2", "second note")
        .expect("second note saved");
    app.refresh_switcher_items(&db)
        .expect("switcher items refreshed");
    app.mode = UiMode::Editor;

    run_keys(
        &mut app,
        &db,
        &[Key::Ctrl('p'), Key::Paste("second".to_string()), Key::Enter],
    );

    assert_eq!(app.active_note.id, "n2");
    assert_eq!(app.mode, UiMode::Normal);
    assert_eq!(app.vim_state.mode, crate::editor_core::vim::VimMode::Normal);

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn switcher_enter_prompts_password_for_locked_note_and_unlocks_on_confirm() {
    let (db, mut app, path) = app_with_note("first note");
    db.save_note("n2", "second note")
        .expect("second note saved");
    db.lock_note("n2", "pass123").expect("lock second note");
    app.refresh_switcher_items(&db)
        .expect("switcher items refreshed");
    app.mode = UiMode::Editor;

    run_keys(
        &mut app,
        &db,
        &[Key::Ctrl('p'), Key::Paste("second".to_string()), Key::Enter],
    );
    assert_eq!(app.mode, UiMode::Switcher);
    assert!(app.switcher_open_confirm.is_some());
    assert_eq!(app.active_note.id, "n1");

    run_keys(
        &mut app,
        &db,
        &[Key::Paste("wrong".to_string()), Key::Enter],
    );
    assert!(app.switcher_open_confirm.is_some());
    assert_eq!(app.active_note.id, "n1");

    run_keys(
        &mut app,
        &db,
        &[Key::Paste("pass123".to_string()), Key::Enter],
    );
    assert!(app.switcher_open_confirm.is_none());
    assert_eq!(app.active_note.id, "n2");
    assert_eq!(app.mode, UiMode::Normal);
    assert_eq!(app.vim_state.mode, crate::editor_core::vim::VimMode::Normal);

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn content_search_matches_ui_fallback_behavior_for_empty_and_pending_queries() {
    let (db, mut app, path) = app_with_note("alpha body");
    db.save_note("n2", "second note")
        .expect("second note saved");
    db.save_note("n3", "misc title").expect("third note saved");
    app.refresh_switcher_items(&db)
        .expect("switcher items refreshed");

    app.open_content_search(&db).expect("open content search");
    assert_eq!(app.mode, UiMode::ContentSearch);
    assert!(
        !app.content_search_results.is_empty(),
        "empty content search query should show title list fallback like UI"
    );

    let (_tx, rx) = std::sync::mpsc::channel();
    app.content_search_rx = Some(rx);
    run_keys(&mut app, &db, &[Key::Paste("apb".to_string())]);
    let fuzzy_only = app.content_search_results.clone();
    assert_eq!(
        fuzzy_only.first().map(|entry| entry.id.as_str()),
        Some("n1"),
        "fuzzy fallback should include current note title matches"
    );
    assert!(
        app.content_search_pending,
        "query edits should stay pending while an in-flight worker is active"
    );
    assert!(
        app.content_search_rx.is_some(),
        "existing worker receiver should remain active until it resolves"
    );
    assert_eq!(app.content_search_query, "apb");

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn content_search_ctrl_backspace_trims_query_word() {
    let (db, mut app, path) = app_with_note("alpha body");
    app.refresh_switcher_items(&db)
        .expect("switcher items refreshed");
    app.open_content_search(&db).expect("open content search");

    run_keys(
        &mut app,
        &db,
        &[Key::Paste("alpha beta".to_string()), Key::CtrlBackspace],
    );
    assert_eq!(app.content_search_query, "alpha ");
    assert!(
        !app.content_search_results.is_empty(),
        "word deletion should refresh fallback results instead of freezing the view"
    );

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn content_search_enter_opens_note_at_result_line() {
    let (db, mut app, path) = app_with_note("first note");
    db.save_note("n2", "line one\nline two\nneedle line\nline four")
        .expect("second note saved");
    app.refresh_switcher_items(&db)
        .expect("switcher items refreshed");
    app.open_content_search(&db).expect("content search opens");

    app.content_search_results = vec![app_core::storage::NoteSearchResult {
        id: "n2".to_string(),
        title: "line one".to_string(),
        snippet: "[[needle]] line".to_string(),
        line_number: 3,
        rank: 0.0,
        updated_at: String::new(),
    }];
    app.content_search_selected = 0;

    run_keys(&mut app, &db, &[Key::Enter]);
    assert_eq!(app.active_note.id, "n2");
    assert_eq!(app.cursor_line, 2);
    assert_eq!(app.mode, UiMode::Normal);

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn content_search_can_reopen_and_find_results_after_opening_match() {
    let (db, mut app, path) = app_with_note("alpha body");
    db.save_note("n2", "first line\nneedle appears here\ntail line")
        .expect("second note saved");
    app.refresh_switcher_items(&db)
        .expect("switcher items refreshed");

    app.open_content_search(&db).expect("content search opens");
    run_keys(&mut app, &db, &[Key::Paste("needle".to_string())]);
    for _ in 0..40 {
        app.maybe_collect_search_results(&db);
        if !app.content_search_results.is_empty() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(
        !app.content_search_results.is_empty(),
        "first content search should produce results"
    );
    assert_eq!(
        app.content_search_results
            .iter()
            .map(|entry| entry.id.as_str())
            .next(),
        Some("n2")
    );

    run_keys(&mut app, &db, &[Key::Enter]);
    assert_eq!(app.active_note.id, "n2");
    assert_eq!(app.mode, UiMode::Normal);

    run_keys(&mut app, &db, &[Key::Ctrl('p'), Key::Tab]);
    assert_eq!(app.mode, UiMode::ContentSearch);
    run_keys(&mut app, &db, &[Key::Paste("needle".to_string())]);
    for _ in 0..40 {
        app.maybe_collect_search_results(&db);
        if !app.content_search_results.is_empty() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(
        app.content_search_results
            .iter()
            .any(|entry| entry.id == "n2"),
        "second content search should still find the opened note"
    );

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn opening_note_from_content_search_clears_search_session_state() {
    let (db, mut app, path) = app_with_note("alpha body");
    db.save_note("n2", "needle in this note")
        .expect("second note saved");
    app.refresh_switcher_items(&db)
        .expect("switcher items refreshed");

    app.open_content_search(&db).expect("content search opens");
    app.content_search_query = "needle".to_string();
    app.content_search_results = vec![app_core::storage::NoteSearchResult {
        id: "n2".to_string(),
        title: "needle in this note".to_string(),
        snippet: "[[needle]] in this note".to_string(),
        line_number: 1,
        rank: 0.0,
        updated_at: String::new(),
    }];
    app.content_search_selected = 0;
    app.content_search_pending = true;
    let (_tx, rx) = std::sync::mpsc::channel();
    app.content_search_rx = Some(rx);

    run_keys(&mut app, &db, &[Key::Enter]);
    assert_eq!(app.active_note.id, "n2");
    assert_eq!(app.mode, UiMode::Normal);
    assert!(app.content_search_query.is_empty());
    assert!(app.content_search_results.is_empty());
    assert_eq!(app.content_search_selected, 0);
    assert!(!app.content_search_pending);
    assert!(
        app.content_search_rx.is_none(),
        "active content-search receiver should be cleared when session closes"
    );
    assert_eq!(
        app.content_search_detached_rxs.len(),
        1,
        "stale receiver should move into detached drain pool"
    );

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn tab_from_content_search_clears_search_session_state() {
    let (db, mut app, path) = app_with_note("alpha body");
    db.save_note("n2", "needle in this note")
        .expect("second note saved");
    app.refresh_switcher_items(&db)
        .expect("switcher items refreshed");

    app.open_content_search(&db).expect("content search opens");
    app.content_search_query = "needle".to_string();
    app.content_search_results = vec![app_core::storage::NoteSearchResult {
        id: "n2".to_string(),
        title: "needle in this note".to_string(),
        snippet: "[[needle]] in this note".to_string(),
        line_number: 1,
        rank: 0.0,
        updated_at: String::new(),
    }];
    app.content_search_selected = 0;
    app.content_search_pending = true;
    let (_tx, rx) = std::sync::mpsc::channel();
    app.content_search_rx = Some(rx);

    run_keys(&mut app, &db, &[Key::Tab]);
    assert_eq!(app.mode, UiMode::Switcher);
    assert!(app.content_search_query.is_empty());
    assert!(app.content_search_results.is_empty());
    assert_eq!(app.content_search_selected, 0);
    assert!(!app.content_search_pending);
    assert!(
        app.content_search_rx.is_none(),
        "active content-search receiver should be cleared when session closes"
    );
    assert_eq!(
        app.content_search_detached_rxs.len(),
        1,
        "stale receiver should move into detached drain pool"
    );

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn reopening_content_search_detaches_stale_receiver_and_dispatches_new_query() {
    let (db, mut app, path) = app_with_note("alpha body");
    db.save_note("n2", "needle in this note")
        .expect("second note saved");
    app.refresh_switcher_items(&db)
        .expect("switcher items refreshed");

    app.open_content_search(&db).expect("content search opens");
    let (_tx, rx) = std::sync::mpsc::channel();
    app.content_search_rx = Some(rx);

    // Close and reopen while a worker is still in-flight.
    run_keys(&mut app, &db, &[Key::Esc]);
    assert_eq!(app.mode, UiMode::Editor);
    run_keys(&mut app, &db, &[Key::Ctrl('p'), Key::Tab]);
    assert_eq!(app.mode, UiMode::ContentSearch);
    assert!(
        app.content_search_rx.is_none(),
        "reopen should start with no active receiver"
    );
    assert_eq!(
        app.content_search_detached_rxs.len(),
        1,
        "stale receiver should move to detached pool on session close"
    );

    // New query should dispatch in reopened session after debounce.
    run_keys(&mut app, &db, &[Key::Paste("needle".to_string())]);
    for _ in 0..60 {
        app.maybe_collect_search_results(&db);
        if app.content_search_rx.is_some() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(
        app.content_search_rx.is_some(),
        "reopen should dispatch new search after debounce without waiting on stale session receiver"
    );

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn content_search_cursor_stays_on_prompt_row_with_fixed_overlay_height() {
    let (db, mut app, path) = app_with_note("alpha body");
    app.open_content_search(&db).expect("content search opens");
    app.content_search_query = "franc".to_string();
    app.content_search_results = vec![app_core::storage::NoteSearchResult {
        id: "n1".to_string(),
        title: "alpha body".to_string(),
        snippet: "[[franc]]".to_string(),
        line_number: 18,
        rank: 0.0,
        updated_at: String::new(),
    }];

    let rows = 24usize;
    let cols = 80usize;
    let (cursor_row, cursor_col) = app.cursor_position(rows, cols);
    let box_w = std::cmp::min(cols.saturating_sub(4).max(30), 72);
    let box_h = std::cmp::min(rows.saturating_sub(4).max(9), 14);
    let x = (cols.saturating_sub(box_w)) / 2 + 1;
    let y = (rows.saturating_sub(box_h)) / 2 + 1;
    let prompt = " content: ";
    let expected_col = x + 1 + prompt.chars().count() + app.content_search_query.chars().count();

    assert_eq!(cursor_row, y + 1);
    assert_eq!(cursor_col, expected_col);

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn content_search_cursor_row_stays_stable_when_result_count_changes() {
    let (db, mut app, path) = app_with_note("alpha body");
    app.open_content_search(&db).expect("content search opens");
    app.content_search_query = "franc".to_string();

    app.content_search_results = vec![app_core::storage::NoteSearchResult {
        id: "n1".to_string(),
        title: "alpha body".to_string(),
        snippet: "[[franc]]".to_string(),
        line_number: 18,
        rank: 0.0,
        updated_at: String::new(),
    }];
    let (row_single, col_single) = app.cursor_position(24, 80);

    app.content_search_results = (0..25)
        .map(|idx| app_core::storage::NoteSearchResult {
            id: format!("n{idx}"),
            title: format!("title {idx}"),
            snippet: format!("[[franc]] {idx}"),
            line_number: idx + 1,
            rank: 0.0,
            updated_at: String::new(),
        })
        .collect();
    let (row_many, col_many) = app.cursor_position(24, 80);

    assert_eq!(row_single, row_many);
    assert_eq!(col_single, col_many);

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn startup_with_locked_recent_note_prompts_for_password() {
    let path = temp_db_path();
    let db = Db::open(path.clone()).expect("db opens");
    db.save_note("n1", "first note").expect("first note saved");
    db.save_note("n2", "second note")
        .expect("second note saved");
    db.lock_note("n2", "pass123").expect("lock second note");
    let opts = TerminalOptions {
        create_new: false,
        note_id: None,
        list_only: false,
    };

    let (app, _) = TerminalApp::new_with_startup_metrics(
        &db,
        &opts,
        true,
        true,
        false,
        true,
        true,
        3,
        crate::terminal::render::RenderPalette::default(),
        "%Y-%m-%d".to_string(),
        "%Y-%m-%d %H:%M".to_string(),
    )
    .expect("terminal app");

    assert_eq!(app.active_note.id, "n2");
    assert_eq!(app.mode, UiMode::Switcher);
    assert_eq!(app.status, "password required to open protected note");
    let confirm = app
        .switcher_open_confirm
        .as_ref()
        .expect("startup should request password");
    assert_eq!(confirm.note_id, "n2");
    assert_eq!(confirm.password, "");

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn fold_commands_toggle_terminal_folds_and_aliases() {
    let (db, mut app, path) = app_with_note("# h1\none\ntwo\n# h2\nthree");
    app.mode = UiMode::Normal;
    app.cursor_line = 0;

    app.execute_terminal_command(&db, "fold");
    assert!(app.folds.collapsed_starts.contains(&0));
    assert_eq!(app.folds.visible_to_real, vec![0, 3, 4]);

    app.execute_terminal_command(&db, "fold");
    assert_eq!(app.status, "fold: already folded");

    app.execute_terminal_command(&db, "unfold");
    assert!(!app.folds.collapsed_starts.contains(&0));

    app.execute_terminal_command(&db, "za");
    assert!(app.folds.collapsed_starts.contains(&0));

    app.execute_terminal_command(&db, "zo");
    assert!(!app.folds.collapsed_starts.contains(&0));

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn command_bar_arrow_history_cycles_latest_commands() {
    let (db, mut app, path) = app_with_note("alpha");
    app.mode = UiMode::Normal;

    run_keys(
        &mut app,
        &db,
        &[
            Key::Char(':'),
            Key::Char('f'),
            Key::Char('o'),
            Key::Char('o'),
            Key::Enter,
        ],
    );
    run_keys(
        &mut app,
        &db,
        &[
            Key::Char(':'),
            Key::Char('b'),
            Key::Char('a'),
            Key::Char('r'),
            Key::Enter,
        ],
    );

    run_keys(&mut app, &db, &[Key::Char(':')]);
    assert_eq!(app.mode, UiMode::CommandBar);
    assert_eq!(app.command_input, "");

    run_keys(&mut app, &db, &[Key::ArrowUp]);
    assert_eq!(app.command_input, "bar");
    run_keys(&mut app, &db, &[Key::ArrowUp]);
    assert_eq!(app.command_input, "foo");
    run_keys(&mut app, &db, &[Key::ArrowUp]);
    assert_eq!(app.command_input, "bar");

    run_keys(&mut app, &db, &[Key::ArrowDown]);
    assert_eq!(app.command_input, "foo");
    run_keys(&mut app, &db, &[Key::ArrowDown]);
    assert_eq!(app.command_input, "bar");

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn command_bar_tab_autocompletes_single_option_then_opens_picker_for_multiple_options() {
    let (db, mut app, path) = app_with_note("alpha");

    run_keys(
        &mut app,
        &db,
        &[Key::Ctrl('e'), Key::Char('m'), Key::Char('o'), Key::Tab],
    );
    assert_eq!(app.mode, UiMode::CommandBar);
    assert!(!app.command_completion.visible);
    assert_eq!(app.command_input, "module ");

    run_keys(&mut app, &db, &[Key::Tab]);
    assert!(app.command_completion.visible);
    assert_eq!(
        app.command_completion
            .options
            .iter()
            .map(|entry| entry.token.as_str())
            .collect::<Vec<_>>(),
        vec!["math", "status", "style", "table", "variables"]
    );
    assert_eq!(app.command_completion.selected_index, 0);

    run_keys(&mut app, &db, &[Key::Tab]);
    assert!(app.command_completion.visible);
    assert_eq!(app.command_input, "module ");
    assert_eq!(app.command_completion.selected_index, 1);

    run_keys(&mut app, &db, &[Key::Tab, Key::Tab, Key::Tab]);
    assert!(app.command_completion.visible);
    assert_eq!(app.command_input, "module ");
    assert_eq!(app.command_completion.selected_index, 4);

    run_keys(&mut app, &db, &[Key::Enter]);
    assert!(!app.command_completion.visible);
    assert_eq!(app.command_input, "module variables ");

    run_keys(&mut app, &db, &[Key::Tab]);
    assert!(app.command_completion.visible);
    assert_eq!(
        app.command_completion
            .options
            .iter()
            .map(|entry| entry.token.as_str())
            .collect::<Vec<_>>(),
        vec!["off", "on", "toggle"]
    );
    assert_eq!(app.command_completion.selected_index, 0);

    run_keys(&mut app, &db, &[Key::Tab]);
    assert!(app.command_completion.visible);
    assert_eq!(app.command_input, "module variables ");
    assert_eq!(app.command_completion.selected_index, 1);

    run_keys(&mut app, &db, &[Key::Enter]);
    assert!(!app.command_completion.visible);
    assert_eq!(app.command_input, "module variables on");

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn command_bar_enter_accepts_picker_selection_before_execute() {
    let (db, mut app, path) = app_with_note("alpha");

    run_keys(
        &mut app,
        &db,
        &[
            Key::Ctrl('e'),
            Key::Char('m'),
            Key::Char('o'),
            Key::Tab,
            Key::Tab,
            Key::Enter,
        ],
    );
    assert_eq!(app.mode, UiMode::CommandBar);
    assert_eq!(app.command_input, "module math ");
    assert!(!app.command_completion.visible);

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn command_bar_ctrl_w_deletes_word_and_stays_in_command_mode() {
    let (db, mut app, path) = app_with_note("alpha");

    run_keys(
        &mut app,
        &db,
        &[Key::Ctrl('e'), Key::Paste("module table on".to_string())],
    );
    assert_eq!(app.mode, UiMode::CommandBar);
    assert_eq!(app.command_input, "module table on");

    run_keys(&mut app, &db, &[Key::Ctrl('w')]);
    assert_eq!(app.command_input, "module table ");
    assert_eq!(app.mode, UiMode::CommandBar);

    run_keys(&mut app, &db, &[Key::Ctrl('w')]);
    assert_eq!(app.command_input, "module ");
    assert_eq!(app.mode, UiMode::CommandBar);

    run_keys(&mut app, &db, &[Key::Ctrl('w')]);
    assert_eq!(app.command_input, "");
    assert_eq!(app.mode, UiMode::CommandBar);

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn wiki_link_heading_autocomplete_loads_headings_beyond_first_32() {
    let (db, mut app, path) = app_with_note("[[01HX4VHR]]");
    let note_id = "01HX4VHR9ABCDEFGHJKMNPQRS";
    let mut body_lines = Vec::new();
    for idx in 1..=500 {
        body_lines.push(format!("## Section {idx}: ABCD"));
    }
    db.save_note(note_id, &body_lines.join("\n"))
        .expect("target note saved");

    app.cursor_line = 0;
    app.cursor_col = 10; // [[ + 8-char short id
    run_keys(&mut app, &db, &[Key::Char('#')]);

    assert!(app.wiki_link_autocomplete_popup.visible);
    assert_eq!(app.wiki_link_autocomplete_popup.query, "01HX4VHR#");
    assert!(
        app.wiki_link_autocomplete_popup
            .suggestions
            .iter()
            .any(|entry| entry.title == "Section 456: ABCD"),
        "heading autocomplete should include headings beyond the previous 32-item cap"
    );

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn wiki_link_heading_autocomplete_reopens_when_hash_is_typed_again() {
    let (db, mut app, path) = app_with_note("[[01HX4VHR]]");
    let note_id = "01HX4VHR9ABCDEFGHJKMNPQRS";
    db.save_note(note_id, "# Intro\n## Deep Dive")
        .expect("target note saved");

    app.cursor_line = 0;
    app.cursor_col = 10; // [[ + 8-char short id
    run_keys(&mut app, &db, &[Key::Char('#')]);
    assert!(app.wiki_link_autocomplete_popup.visible);

    run_keys(&mut app, &db, &[Key::Esc, Key::Backspace, Key::Char('#')]);
    assert!(
        app.wiki_link_autocomplete_popup.visible,
        "typing # inside an existing wiki-link should reopen heading suggestions"
    );
    assert_eq!(app.wiki_link_autocomplete_popup.query, "01HX4VHR#");

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn wiki_link_autocomplete_selection_can_move_and_apply_beyond_first_sixteen_results() {
    let (db, mut app, path) = app_with_note("");
    run_keys(&mut app, &db, &[Key::Char('['), Key::Char('[')]);
    assert!(app.wiki_link_autocomplete_popup.visible);

    let suggestions: Vec<WikiLinkSuggestion> = (0..25)
        .map(|idx| {
            let short_id = format!("A{idx:07}");
            let title = format!("Note {idx:02}");
            WikiLinkSuggestion {
                short_id,
                title: title.clone(),
                title_lower: title.to_lowercase(),
                heading: None,
            }
        })
        .collect();
    app.wiki_link_autocomplete_popup.suggestions = suggestions.clone();
    app.wiki_link_autocomplete_popup.note_suggestions = suggestions;
    app.wiki_link_autocomplete_popup.selected_index = 0;

    for _ in 0..20 {
        run_keys(&mut app, &db, &[Key::ArrowDown]);
    }
    assert_eq!(app.wiki_link_autocomplete_popup.selected_index, 20);
    assert_eq!(app.filtered_wiki_link_suggestions().len(), 16);

    run_keys(&mut app, &db, &[Key::Tab]);
    assert_eq!(app.current_line(), "[[A0000020]]");
    assert_eq!(app.status, "link: Note 20");

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn startup_with_wiki_links_primes_resolution_for_first_render() {
    let path = temp_db_path();
    let db = Db::open(path.clone()).expect("db opens");
    db.save_note("n-active", "[[01HX4VHR]]")
        .expect("active note saved");
    db.save_note("01HX4VHR9ABCDEFGHJKMNPQRS", "Destination Title\nBody")
        .expect("target note saved");
    let opts = TerminalOptions {
        create_new: false,
        note_id: Some("n-active".to_string()),
        list_only: false,
    };
    let (app, _) = TerminalApp::new_with_startup_metrics(
        &db,
        &opts,
        true,
        true,
        false,
        true,
        true,
        3,
        crate::terminal::render::RenderPalette::default(),
        "%Y-%m-%d".to_string(),
        "%Y-%m-%d %H:%M".to_string(),
    )
    .expect("terminal app");

    let entry = app
        .wiki_link_prefix_index
        .get("01HX4VHR")
        .expect("short id should be resolved on startup");
    assert_eq!(entry.title, "Destination Title");

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}
