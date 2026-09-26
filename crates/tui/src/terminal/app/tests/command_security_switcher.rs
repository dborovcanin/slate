use super::*;
use crate::terminal::app::{WebSearchResponse, WikiLinkSuggestion};

#[test]
fn apply_edit_operation_single_line_change_updates_in_place() {
    let (_db, mut app, path) = app_with_note("alpha\nbeta");
    app.editor.cursor_line = 0;
    app.editor.cursor_col = 2;

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

    assert_eq!(
        app.editor.lines,
        vec!["aXYZa".to_string(), "beta".to_string()]
    );
    assert_eq!(app.editor.cursor_line, 0);
    assert_eq!(app.editor.cursor_col, 3);

    drop(app);
    cleanup_db_files(&path);
}

#[test]
fn apply_edit_operation_cross_line_change_still_merges_lines() {
    let (_db, mut app, path) = app_with_note("alpha\nbeta");
    app.editor.cursor_line = 1;
    app.editor.cursor_col = 0;

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

    assert_eq!(app.editor.lines, vec!["alphabeta".to_string()]);
    assert_eq!(app.editor.cursor_line, 0);
    assert_eq!(app.editor.cursor_col, 5);

    drop(app);
    cleanup_db_files(&path);
}

#[test]
fn apply_edit_operation_multiline_insert_updates_lines_directly() {
    let (_db, mut app, path) = app_with_note("start end");
    app.editor.cursor_line = 0;
    app.editor.cursor_col = 6;

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
        app.editor.lines,
        vec!["start a".to_string(), "b".to_string(), "end".to_string()]
    );
    assert_eq!(app.editor.cursor_line, 2);
    assert_eq!(app.editor.cursor_col, 0);

    drop(app);
    cleanup_db_files(&path);
}

#[test]
fn apply_edit_operation_multi_change_updates_lines_without_full_rebuild() {
    let (_db, mut app, path) = app_with_note("alpha\nbeta\ngamma");
    app.editor.cursor_line = 1;
    app.editor.cursor_col = 2;

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
        app.editor.lines,
        vec![
            "Alpha".to_string(),
            "beta".to_string(),
            "G".to_string(),
            "H".to_string(),
        ]
    );
    assert_eq!(app.editor.cursor_line, 3);
    assert_eq!(app.editor.cursor_col, 0);

    drop(app);
    cleanup_db_files(&path);
}

#[test]
fn editor_paste_multiline_inserts_as_single_bulk_edit() {
    let (db, mut app, path) = app_with_note("start end");
    app.mode = UiMode::Editor;
    app.editor.cursor_line = 0;
    app.editor.cursor_col = 6; // after "start "

    app.handle_editor_key(&db, Key::Paste("a\nb\n".to_string()))
        .expect("paste applies");

    assert_eq!(
        app.editor.lines,
        vec!["start a".to_string(), "b".to_string(), "end".to_string()]
    );
    assert_eq!(app.editor.cursor_line, 2);
    assert_eq!(app.editor.cursor_col, 0);

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
    app.editor.lines = vec!["one updated".to_string()];
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
    let note_id = crate::note_id_for_file(&markdown_path);
    let opts = TerminalOptions {
        create_new: false,
        note_id: Some(note_id.clone()),
        list_only: false,
        open_switcher: false,
        open_at_end: false,
    };
    let (mut app, _) = TerminalApp::new_with_startup_metrics(
        &db,
        &opts,
        crate::config::ThemeConfig::default(),
        true,
        true,
        false,
        true,
        true,
        3,
        crate::terminal::render::RenderPalette::default(),
        "%Y-%m-%d".to_string(),
        "%Y-%m-%d %H:%M".to_string(),
        std::sync::Arc::new(std::sync::Mutex::new(
            app_core::cross_note::CrossNoteVarIndex::default(),
        )),
    )
    .expect("terminal app");
    assert_eq!(app.editor.lines, vec!["file body".to_string()]);
    app.mode = UiMode::Normal;
    app.command_bar_from_normal = true;
    app.editor.lines = vec!["updated body".to_string()];
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
    app.editor.lines = vec!["local body".to_string()];
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
    let note_id = crate::note_id_for_file(&markdown_path);
    let opts = TerminalOptions {
        create_new: false,
        note_id: Some(note_id),
        list_only: false,
        open_switcher: false,
        open_at_end: false,
    };
    let (mut app, _) = TerminalApp::new_with_startup_metrics(
        &db,
        &opts,
        crate::config::ThemeConfig::default(),
        true,
        true,
        false,
        true,
        true,
        3,
        crate::terminal::render::RenderPalette::default(),
        "%Y-%m-%d".to_string(),
        "%Y-%m-%d %H:%M".to_string(),
        std::sync::Arc::new(std::sync::Mutex::new(
            app_core::cross_note::CrossNoteVarIndex::default(),
        )),
    )
    .expect("terminal app");
    app.mode = UiMode::Normal;
    app.command_bar_from_normal = true;
    app.editor.lines = vec!["local body".to_string()];
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
    app.editor.lines = vec!["one updated".to_string()];
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
    app.editor.lines = vec!["one updated".to_string()];
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
    app.editor.lines = vec!["one updated".to_string()];
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

    app.execute_terminal_command(&db, "clip-watch on");
    assert!(app.clipboard_watch.enabled);
    assert_eq!(app.status, "clip-watch started");

    app.execute_terminal_command(&db, "clip-watch on");
    assert!(app.clipboard_watch.enabled);
    assert_eq!(app.status, "clip-watch already active");

    app.execute_terminal_command(&db, "clip-watch off");
    assert!(!app.clipboard_watch.enabled);
    assert_eq!(app.status, "clip-watch stopped");

    app.execute_terminal_command(&db, "clip-watch off");
    assert!(!app.clipboard_watch.enabled);
    assert_eq!(app.status, "clip-watch not active");

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn export_md_without_path_uses_terminal_clipboard_fallback() {
    let (db, mut app, path) = app_with_note("alpha\nbeta");
    app.mode = UiMode::Normal;

    app.execute_terminal_command(&db, "export md");

    assert!(
        app.status.starts_with("exported md to clipboard"),
        "status was: {}",
        app.status
    );
    assert_eq!(app.clipboard.mode, VimRegisterMode::Charwise);
    assert_eq!(app.clipboard.text, "alpha\nbeta");

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn export_txt_with_path_writes_file() {
    let (db, mut app, path) = app_with_note("plain text body");
    app.mode = UiMode::Normal;
    let export_path = std::env::temp_dir().join(format!("slate-export-{}.txt", ulid::Ulid::new()));
    let export_path_str = export_path.to_string_lossy().to_string();

    app.execute_terminal_command(&db, &format!("export txt {}", export_path_str));

    assert_eq!(app.status, format!("exported txt to {}", export_path_str));
    assert_eq!(
        std::fs::read_to_string(&export_path).expect("exported text file should be readable"),
        "plain text body"
    );

    let _ = std::fs::remove_file(&export_path);
    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn export_pdf_with_path_writes_pdf_file() {
    let (db, mut app, path) = app_with_note("# Export\n\n- [x] done");
    app.mode = UiMode::Normal;
    let export_path = std::env::temp_dir().join(format!("slate-export-{}.pdf", ulid::Ulid::new()));
    let export_path_str = export_path.to_string_lossy().to_string();

    app.execute_terminal_command(&db, &format!("export pdf {}", export_path_str));

    assert_eq!(app.status, format!("exported pdf to {}", export_path_str));
    let bytes = std::fs::read(&export_path).expect("exported pdf should be readable");
    assert!(bytes.starts_with(b"%PDF-"), "missing PDF header");
    assert!(bytes.len() > 100, "pdf output unexpectedly small");

    let _ = std::fs::remove_file(&export_path);
    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn export_pdf_without_path_returns_usage_error() {
    let (db, mut app, path) = app_with_note("alpha");
    app.mode = UiMode::Normal;

    app.execute_terminal_command(&db, "export pdf");

    assert_eq!(app.status, "usage: export pdf <path>");

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn module_commands_update_and_persist_note_modules() {
    let (db, mut app, path) = app_with_note("alpha := 10\nalp");
    app.mode = UiMode::Editor;
    app.editor.cursor_line = 1;
    app.editor.cursor_col = line_char_len(app.current_line());
    app.refresh_variable_autocomplete_popup();
    assert!(app.variable_autocomplete_popup.visible);

    app.execute_terminal_command(&db, "module status");
    assert_eq!(
        app.status,
        "modules math=on table=on variables=on style=on cross_note=on"
    );

    app.execute_terminal_command(&db, "modules variables off");
    assert_eq!(
        app.status,
        "modules math=on table=on variables=off style=on cross_note=on"
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
    assert_eq!(
        app.status,
        "modules math=on table=on variables=on style=on cross_note=on"
    );
    assert!(app.active_note.modules.variables);

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn module_math_toggle_disables_calc_tab_path() {
    let (db, mut app, path) = app_with_note("1 + 1");
    app.mode = UiMode::Editor;
    app.editor.cursor_line = 0;
    app.editor.cursor_col = line_char_len(app.current_line());

    app.execute_terminal_command(&db, "module math off");
    app.handle_editor_key(&db, Key::Tab)
        .expect("tab falls back when math module is off");
    assert_eq!(app.editor.lines[0], "1 + 1  ");

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn module_table_toggle_disables_table_cursor_clamping() {
    let (db, mut app, path) = app_with_note("| aaa |     | bb  |");
    app.mode = UiMode::Editor;
    app.editor.cursor_col = 9;

    app.execute_terminal_command(&db, "module table off");
    app.adjust_cursor();
    assert_eq!(app.editor.cursor_col, 9);

    app.execute_terminal_command(&db, "module table on");
    app.editor.cursor_col = 9;
    app.adjust_cursor();
    assert_eq!(app.editor.cursor_col, 8);

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn module_style_toggle_disables_enter_autoformat_rules() {
    let (db, mut app, path) = app_with_note("- [ ] task");
    app.mode = UiMode::Editor;
    app.editor.cursor_col = line_char_len(app.current_line());

    app.execute_terminal_command(&db, "module style off");
    app.handle_editor_key(&db, Key::Enter)
        .expect("enter uses plain newline when style module is off");
    assert_eq!(
        app.editor.lines,
        vec!["- [ ] task".to_string(), String::new()]
    );

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn module_style_off_keeps_table_autoformat_when_table_module_is_on() {
    let (db, mut app, path) = app_with_note("| a | b |\n| --- | --- |\n|1|2|");
    app.mode = UiMode::Editor;
    app.editor.cursor_line = 2;
    app.editor.cursor_col = 4; // before trailing pipe in "|1|2|"

    app.execute_terminal_command(&db, "module style off");
    app.handle_editor_key(&db, Key::Char('0'))
        .expect("typing still triggers table autoformat");

    assert_ne!(app.editor.lines[2], "|1|20|");
    assert!(app.editor.lines[2].contains("20"));
    assert!(app.editor.lines[2].starts_with("| "));

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn module_style_off_keeps_table_ctrl_navigation_when_table_module_is_on() {
    let (db, mut app, path) = app_with_note("| aaa | bb  |");
    app.mode = UiMode::Editor;
    app.editor.cursor_col = 5; // first cell end anchor

    app.execute_terminal_command(&db, "module style off");

    app.handle_editor_key(&db, Key::CtrlArrowRight)
        .expect("ctrl-right jumps to next table cell");
    assert_eq!(app.editor.cursor_col, 10);

    app.handle_editor_key(&db, Key::CtrlArrowLeft)
        .expect("ctrl-left jumps back to previous table cell");
    assert_eq!(app.editor.cursor_col, 5);

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn note_unprotect_command_removes_lock() {
    let (db, mut app, path) = app_with_note("top secret");

    app.execute_terminal_command(&db, "note lock pass123");
    assert_eq!(app.active_note.access_mode, NoteAccessMode::Locked);
    assert_eq!(app.status, "note session-locked; not encrypted at rest");

    app.execute_terminal_command(&db, "note unprotect pass123");
    assert_eq!(app.active_note.access_mode, NoteAccessMode::None);
    assert_eq!(app.editor.lines, vec!["top secret".to_string()]);
    assert_eq!(app.status, "note decrypted; at-rest encryption removed");

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
    assert_eq!(app.editor.lines, vec!["classified".to_string()]);
    assert_eq!(app.status, "note decrypted; at-rest encryption removed");

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
    assert_eq!(app.status, "note session-locked; not encrypted at rest");

    app.execute_terminal_command(&db, "unlock-note pass123");
    assert_eq!(app.active_note.access_mode, NoteAccessMode::Locked);
    assert!(app.active_note.is_unlocked);
    assert_eq!(app.status, "note unlocked for this session");

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
    assert_eq!(app.editor.lines, vec![String::new()]);
    assert!(!app.dirty);

    app.handle_editor_key(&db, Key::Char('x'))
        .expect("locked edit should not fail");
    assert_eq!(app.editor.lines, vec![String::new()]);
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
    assert!(app.switcher.delete_confirm.is_some());

    run_keys(&mut app, &db, &[Key::Char('n')]);
    assert!(app.switcher.delete_confirm.is_none());
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
        .switcher
        .delete_confirm
        .as_ref()
        .expect("delete confirmation requested");
    assert_eq!(pending.note_id, "n1");

    run_keys(&mut app, &db, &[Key::Enter]);
    assert!(app.switcher.delete_confirm.is_none());
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
    assert!(app.switcher.open_confirm.is_some());
    assert_eq!(app.active_note.id, "n1");

    run_keys(
        &mut app,
        &db,
        &[Key::Paste("wrong".to_string()), Key::Enter],
    );
    assert!(app.switcher.open_confirm.is_some());
    assert_eq!(app.active_note.id, "n1");

    run_keys(
        &mut app,
        &db,
        &[Key::Paste("pass123".to_string()), Key::Enter],
    );
    assert!(app.switcher.open_confirm.is_none());
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
        !app.content_search.results.is_empty(),
        "empty content search query should show title list fallback"
    );

    let (_tx, rx) = std::sync::mpsc::channel();
    app.content_search.rx = Some(rx);
    run_keys(&mut app, &db, &[Key::Paste("apb".to_string())]);
    let fuzzy_only = app.content_search.results.clone();
    assert_eq!(
        fuzzy_only.first().map(|entry| entry.id.as_str()),
        Some("n1"),
        "fuzzy fallback should include current note title matches"
    );
    assert!(
        app.content_search.pending,
        "query edits should stay pending after superseding an in-flight worker"
    );
    assert!(app.content_search.rx.is_none());
    assert_eq!(app.content_search.detached_rxs.len(), 1);
    assert_eq!(app.content_search.query, "apb");

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
    assert_eq!(app.content_search.query, "alpha ");
    assert!(
        !app.content_search.results.is_empty(),
        "word deletion should refresh fallback results instead of freezing the view"
    );

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn content_search_allows_mid_query_cursor_editing() {
    let (db, mut app, path) = app_with_note("alpha body");
    app.refresh_switcher_items(&db)
        .expect("switcher items refreshed");
    app.open_content_search(&db).expect("open content search");

    run_keys(
        &mut app,
        &db,
        &[
            Key::Paste("alpha".to_string()),
            Key::ArrowLeft,
            Key::ArrowLeft,
            Key::Char('Z'),
        ],
    );
    assert_eq!(app.content_search.query, "alpZha");
    assert_eq!(app.content_search.cursor_col, 4);

    run_keys(&mut app, &db, &[Key::Delete]);
    assert_eq!(app.content_search.query, "alpZa");
    assert_eq!(app.content_search.cursor_col, 4);

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

    app.content_search.results = vec![app_core::storage::NoteSearchResult {
        id: "n2".to_string(),
        title: "line one".to_string(),
        snippet: "[[needle]] line".to_string(),
        line_number: 3,
        rank: 0.0,
        updated_at: String::new(),
    }];
    app.content_search.selected = 0;

    run_keys(&mut app, &db, &[Key::Enter]);
    assert_eq!(app.active_note.id, "n2");
    assert_eq!(app.editor.cursor_line, 2);
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
        if !app.content_search.results.is_empty() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(
        !app.content_search.results.is_empty(),
        "first content search should produce results"
    );
    assert_eq!(
        app.content_search
            .results
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
        if !app.content_search.results.is_empty() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(
        app.content_search
            .results
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
    app.content_search.query = "needle".to_string();
    app.content_search.results = vec![app_core::storage::NoteSearchResult {
        id: "n2".to_string(),
        title: "needle in this note".to_string(),
        snippet: "[[needle]] in this note".to_string(),
        line_number: 1,
        rank: 0.0,
        updated_at: String::new(),
    }];
    app.content_search.selected = 0;
    app.content_search.pending = true;
    let (_tx, rx) = std::sync::mpsc::channel();
    app.content_search.rx = Some(rx);

    run_keys(&mut app, &db, &[Key::Enter]);
    assert_eq!(app.active_note.id, "n2");
    assert_eq!(app.mode, UiMode::Normal);
    assert!(app.content_search.query.is_empty());
    assert!(app.content_search.results.is_empty());
    assert_eq!(app.content_search.selected, 0);
    assert!(!app.content_search.pending);
    assert!(
        app.content_search.rx.is_none(),
        "active content-search receiver should be cleared when session closes"
    );
    assert_eq!(
        app.content_search.detached_rxs.len(),
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
    app.content_search.query = "needle".to_string();
    app.content_search.results = vec![app_core::storage::NoteSearchResult {
        id: "n2".to_string(),
        title: "needle in this note".to_string(),
        snippet: "[[needle]] in this note".to_string(),
        line_number: 1,
        rank: 0.0,
        updated_at: String::new(),
    }];
    app.content_search.selected = 0;
    app.content_search.pending = true;
    let (_tx, rx) = std::sync::mpsc::channel();
    app.content_search.rx = Some(rx);

    run_keys(&mut app, &db, &[Key::Tab]);
    assert_eq!(app.mode, UiMode::Switcher);
    assert!(app.content_search.query.is_empty());
    assert!(app.content_search.results.is_empty());
    assert_eq!(app.content_search.selected, 0);
    assert!(!app.content_search.pending);
    assert!(
        app.content_search.rx.is_none(),
        "active content-search receiver should be cleared when session closes"
    );
    assert_eq!(
        app.content_search.detached_rxs.len(),
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
    app.content_search.rx = Some(rx);

    // Close and reopen while a worker is still in-flight.
    run_keys(&mut app, &db, &[Key::Esc]);
    assert_eq!(app.mode, UiMode::Editor);
    run_keys(&mut app, &db, &[Key::Ctrl('p'), Key::Tab]);
    assert_eq!(app.mode, UiMode::ContentSearch);
    assert!(
        app.content_search.rx.is_none(),
        "reopen should start with no active receiver"
    );
    assert_eq!(
        app.content_search.detached_rxs.len(),
        1,
        "stale receiver should move to detached pool on session close"
    );

    // New query should dispatch in reopened session after debounce.
    run_keys(&mut app, &db, &[Key::Paste("needle".to_string())]);
    for _ in 0..60 {
        app.maybe_collect_search_results(&db);
        if app.content_search.rx.is_some() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(
        app.content_search.rx.is_some(),
        "reopen should dispatch new search after debounce without waiting on stale session receiver"
    );

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn content_search_dispatches_even_with_full_detached_pool() {
    let (db, mut app, path) = app_with_note("alpha body");
    app.refresh_switcher_items(&db)
        .expect("switcher items refreshed");
    app.open_content_search(&db).expect("content search opens");

    for _ in 0..2 {
        let (_tx, rx) = std::sync::mpsc::channel();
        app.content_search.detached_rxs.push(rx);
    }
    assert_eq!(app.content_search.detached_rxs.len(), 2);

    run_keys(&mut app, &db, &[Key::Paste("alpha".to_string())]);
    for _ in 0..60 {
        app.maybe_collect_search_results(&db);
        if app.content_search.rx.is_some() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }

    assert!(
        app.content_search.rx.is_some(),
        "new content search should dispatch even when detached pool already has stale workers"
    );

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn content_search_cursor_stays_on_prompt_row_with_fixed_overlay_height() {
    let (db, mut app, path) = app_with_note("alpha body");
    app.open_content_search(&db).expect("content search opens");
    app.content_search.query = "franc".to_string();
    app.content_search.cursor_col = app.content_search.query.chars().count();
    app.content_search.results = vec![app_core::storage::NoteSearchResult {
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
    let prompt = crate::terminal::switcher::PROMPT_PREFIX;
    let expected_col = x + 1 + prompt.chars().count() + app.content_search.query.chars().count();

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
    app.content_search.query = "franc".to_string();
    app.content_search.cursor_col = app.content_search.query.chars().count();

    app.content_search.results = vec![app_core::storage::NoteSearchResult {
        id: "n1".to_string(),
        title: "alpha body".to_string(),
        snippet: "[[franc]]".to_string(),
        line_number: 18,
        rank: 0.0,
        updated_at: String::new(),
    }];
    let (row_single, col_single) = app.cursor_position(24, 80);

    app.content_search.results = (0..25)
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
        open_switcher: false,
        open_at_end: false,
    };

    let (app, _) = TerminalApp::new_with_startup_metrics(
        &db,
        &opts,
        crate::config::ThemeConfig::default(),
        true,
        true,
        false,
        true,
        true,
        3,
        crate::terminal::render::RenderPalette::default(),
        "%Y-%m-%d".to_string(),
        "%Y-%m-%d %H:%M".to_string(),
        std::sync::Arc::new(std::sync::Mutex::new(
            app_core::cross_note::CrossNoteVarIndex::default(),
        )),
    )
    .expect("terminal app");

    assert_eq!(app.active_note.id, "n2");
    assert_eq!(app.mode, UiMode::Switcher);
    assert_eq!(app.status, "password required to open session-locked note");
    let confirm = app
        .switcher
        .open_confirm
        .as_ref()
        .expect("startup should request password");
    assert_eq!(confirm.note_id, "n2");
    assert_eq!(confirm.access_mode, NoteAccessMode::Locked);
    assert_eq!(confirm.password, "");

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn fold_commands_toggle_terminal_folds_and_aliases() {
    let (db, mut app, path) = app_with_note("# h1\none\ntwo\n# h2\nthree");
    app.mode = UiMode::Normal;
    app.editor.cursor_line = 0;

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
        vec![
            "cross_note",
            "math",
            "status",
            "style",
            "table",
            "variables"
        ]
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
    assert_eq!(app.command_input, "module table ");

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
    assert_eq!(app.command_input, "module table ");
    assert_eq!(app.command_completion.selected_index, 1);

    run_keys(&mut app, &db, &[Key::Enter]);
    assert!(!app.command_completion.visible);
    assert_eq!(app.command_input, "module table on");

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn command_bar_perf_prefix_exposes_terminal_perf_completion_options() {
    let (db, mut app, path) = app_with_note("alpha");

    run_keys(
        &mut app,
        &db,
        &[
            Key::Ctrl('e'),
            Key::Char('p'),
            Key::Char('e'),
            Key::Char('r'),
            Key::Char('f'),
            Key::Tab,
        ],
    );
    assert_eq!(app.command_input, "perf ");

    run_keys(&mut app, &db, &[Key::Tab]);
    assert!(app.command_completion.visible);
    assert_eq!(
        app.command_completion
            .options
            .iter()
            .map(|entry| entry.token.as_str())
            .collect::<Vec<_>>(),
        vec!["cap", "clear", "dump", "off", "on", "status", "toggle", "where"]
    );

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn execute_terminal_perf_where_reports_log_path() {
    let (db, mut app, path) = app_with_note("alpha");
    app.execute_terminal_command(&db, "perf where");
    assert!(app.status.starts_with("perf logs: "));
    assert!(app.status.contains("slate-log.log"));

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn execute_terminal_perf_on_and_status_toggle_runtime_trace_state() {
    let (db, mut app, path) = app_with_note("alpha");
    assert!(!app.perf_trace.enabled);

    app.execute_terminal_command(&db, "perf on");
    assert!(app.perf_trace.enabled);
    assert!(app.status.starts_with("perf on"));

    app.execute_terminal_command(&db, "perf status");
    assert!(app.status.starts_with("perf on"));

    app.execute_terminal_command(&db, "perf off");
    assert!(!app.perf_trace.enabled);
    assert!(app.status.starts_with("perf off"));

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn normal_mode_movement_does_not_trigger_table_autoformat() {
    let (db, mut app, path) = app_with_note("|a|b|\n|---|---|\n|1|2|");
    app.mode = UiMode::Normal;
    app.vim_state = crate::editor_core::vim::VimState::default();
    app.editor.cursor_line = 0;
    app.editor.cursor_col = 0;

    run_keys(&mut app, &db, &[Key::Char('l')]);

    assert_eq!(app.editor.lines[0], "|a|b|".to_string());
    assert_eq!(app.editor.lines[1], "|---|---|".to_string());
    assert_eq!(app.editor.lines[2], "|1|2|".to_string());

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
    assert_eq!(app.command_input, "module cross_note ");
    assert!(!app.command_completion.visible);

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn command_bar_esc_closes_picker_and_returns_to_normal_mode() {
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
        ],
    );
    assert_eq!(app.mode, UiMode::CommandBar);
    assert!(app.command_completion.visible);

    run_keys(&mut app, &db, &[Key::Esc]);
    assert_eq!(app.mode, UiMode::Normal);
    assert!(!app.command_completion.visible);
    assert!(app.command_input.is_empty());

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn command_bar_backtab_cycles_completion_backward() {
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
        ],
    );
    assert_eq!(app.command_completion.selected_index, 0);

    run_keys(&mut app, &db, &[Key::BackTab]);
    assert_eq!(app.command_completion.selected_index, 5);

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

    app.editor.cursor_line = 0;
    app.editor.cursor_col = 10; // [[ + 8-char short id
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

    app.editor.cursor_line = 0;
    app.editor.cursor_col = 10; // [[ + 8-char short id
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
fn wiki_link_autocomplete_respects_working_collection_filter() {
    let (db, mut app, path) = app_with_note("");
    let work = db
        .create_collection("Work", "work notes")
        .expect("work collection created");
    let personal = db
        .create_collection("Personal", "personal notes")
        .expect("personal collection created");
    db.save_note("01HXWORK9ABCDEFGHJKMNPQRS", "Alpha Work")
        .expect("work note saved");
    db.save_note("01HXPERS9ABCDEFGHJKMNPQRS", "Alpha Personal")
        .expect("personal note saved");
    db.set_note_collections("01HXWORK9ABCDEFGHJKMNPQRS", std::slice::from_ref(&work.id))
        .expect("work note collection set");
    db.set_note_collections(
        "01HXPERS9ABCDEFGHJKMNPQRS",
        std::slice::from_ref(&personal.id),
    )
    .expect("personal note collection set");

    app.execute_terminal_command(&db, "collection choose Work");
    run_keys(
        &mut app,
        &db,
        &[
            Key::Char('['),
            Key::Char('['),
            Key::Char('a'),
            Key::Char('l'),
        ],
    );

    assert!(app.wiki_link_autocomplete_popup.visible);
    let titles = app
        .wiki_link_autocomplete_popup
        .suggestions
        .iter()
        .map(|entry| entry.title.clone())
        .collect::<Vec<_>>();
    assert!(
        titles.iter().any(|title| title == "Alpha Work"),
        "expected work note in suggestions, got: {:?}",
        titles
    );
    assert!(
        !titles.iter().any(|title| title == "Alpha Personal"),
        "personal note should be filtered out, got: {:?}",
        titles
    );

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
    assert_eq!(app.current_line(), "[[A0000020#]]");
    assert_eq!(app.status, "link: Note 20");

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn wiki_link_heading_autocomplete_cancel_removes_auto_hash() {
    let (db, mut app, path) = app_with_note("");
    db.save_note("01HX4VHR9ABCDEFGHJKMNPQRS", "# Intro\n## Deep Dive")
        .expect("target note saved");
    run_keys(&mut app, &db, &[Key::Char('['), Key::Char('[')]);
    assert!(app.wiki_link_autocomplete_popup.visible);

    app.wiki_link_autocomplete_popup.suggestions = vec![WikiLinkSuggestion {
        short_id: "01HX4VHR".to_string(),
        title: "Target".to_string(),
        title_lower: "target".to_string(),
        heading: None,
    }];
    app.wiki_link_autocomplete_popup.note_suggestions =
        app.wiki_link_autocomplete_popup.suggestions.clone();

    run_keys(&mut app, &db, &[Key::Tab]);
    assert_eq!(app.current_line(), "[[01HX4VHR#]]");
    assert!(app.wiki_link_autocomplete_popup.visible);

    run_keys(&mut app, &db, &[Key::Esc]);
    assert_eq!(app.current_line(), "[[01HX4VHR]]");
    assert!(!app.wiki_link_autocomplete_popup.visible);

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn startup_with_wiki_links_keeps_switcher_metadata_lazy() {
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
        open_switcher: false,
        open_at_end: false,
    };
    let (app, _) = TerminalApp::new_with_startup_metrics(
        &db,
        &opts,
        crate::config::ThemeConfig::default(),
        true,
        true,
        false,
        true,
        true,
        3,
        crate::terminal::render::RenderPalette::default(),
        "%Y-%m-%d".to_string(),
        "%Y-%m-%d %H:%M".to_string(),
        std::sync::Arc::new(std::sync::Mutex::new(
            app_core::cross_note::CrossNoteVarIndex::default(),
        )),
    )
    .expect("terminal app");

    assert!(app.switcher.items.is_empty());
    assert!(app.wiki_link_prefix_index.is_empty());

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn collection_commands_choose_add_and_remove_match_expected_outcomes() {
    let (db, mut app, path) = app_with_note("base");
    let collection = db
        .create_collection("Projects", "project notes")
        .expect("collection created");

    app.execute_terminal_command(&db, "collection choose Projects");
    assert_eq!(app.status, "working collection: Projects");
    assert_eq!(
        app.working_collection_id.as_deref(),
        Some(collection.id.as_str())
    );
    assert_eq!(app.working_collection_name.as_deref(), Some("Projects"));

    app.execute_terminal_command(&db, "collection join Projects");
    assert_eq!(app.status, "added to collection Projects");
    assert_eq!(
        db.get_note_collection_ids("n1")
            .expect("membership lookup after add"),
        vec![collection.id.clone()]
    );

    app.execute_terminal_command(&db, "collection leave Projects");
    assert_eq!(app.status, "removed from collection Projects");
    assert!(db
        .get_note_collection_ids("n1")
        .expect("membership lookup after remove")
        .is_empty());

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn clear_collection_command_resets_terminal_working_collection_context() {
    let (db, mut app, path) = app_with_note("base");
    db.create_collection("Projects", "project notes")
        .expect("collection created");

    app.execute_terminal_command(&db, "collection choose Projects");
    assert_eq!(app.working_collection_name.as_deref(), Some("Projects"));

    app.execute_terminal_command(&db, "collection clear");
    assert_eq!(app.status, "working collection cleared");
    assert!(app.working_collection_id.is_none());
    assert!(app.working_collection_name.is_none());

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn ctrl_n_creates_note_in_working_collection_context() {
    let (db, mut app, path) = app_with_note("base");
    let collection = db
        .create_collection("Projects", "project notes")
        .expect("collection created");

    app.execute_terminal_command(&db, "collection choose Projects");
    let previous_id = app.active_note.id.clone();
    app.mode = UiMode::Normal;
    app.handle_editor_key(&db, Key::Ctrl('n'))
        .expect("ctrl+n creates note");

    let new_id = app.active_note.id.clone();
    assert_ne!(new_id, previous_id);
    assert_eq!(
        db.get_note_collection_ids(&new_id)
            .expect("new note membership lookup"),
        vec![collection.id]
    );

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn refresh_switcher_items_clears_stale_working_collection_after_delete() {
    let (db, mut app, path) = app_with_note("base");
    let collection = db
        .create_collection("Projects", "project notes")
        .expect("collection created");

    app.execute_terminal_command(&db, "collection choose Projects");
    assert_eq!(
        app.working_collection_id.as_deref(),
        Some(collection.id.as_str())
    );

    db.delete_collection(&collection.id)
        .expect("delete collection succeeds");
    app.refresh_switcher_items(&db)
        .expect("refresh switcher succeeds");

    assert!(app.working_collection_id.is_none());
    assert!(app.working_collection_name.is_none());

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn collection_create_delete_and_update_commands_behave_as_expected() {
    let (db, mut app, path) = app_with_note("base");

    app.execute_terminal_command(&db, "collection create Projects");
    assert_eq!(app.status, "collection created: Projects");
    let created = db
        .get_collection_by_name("Projects")
        .expect("collection lookup")
        .expect("collection exists");

    app.execute_terminal_command(&db, "collection update Projects");
    assert_eq!(
        app.status,
        "collection update: editing Projects (Enter save, Esc cancel)"
    );
    assert_eq!(app.mode, UiMode::CollectionSwitcher);
    assert!(app.collection_switcher.edit_dialog.is_some());

    run_keys(
        &mut app,
        &db,
        &[
            Key::Tab,
            Key::Paste("Project notes".to_string()),
            Key::Tab,
            Key::Paste("alpha, beta, alpha".to_string()),
            Key::Enter,
        ],
    );
    assert_eq!(app.status, "collection updated: Projects");
    let updated = db
        .get_collection(&created.id)
        .expect("collection lookup by id")
        .expect("collection still exists");
    assert_eq!(updated.description, "Project notes");
    assert_eq!(
        db.list_collection_default_tags(&created.id)
            .expect("default tags lookup"),
        vec!["alpha".to_string(), "beta".to_string()]
    );

    app.execute_terminal_command(&db, "collection delete Projects");
    assert_eq!(app.status, "collection deleted: Projects");
    assert!(db
        .get_collection(&created.id)
        .expect("collection lookup by id")
        .is_none());

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn ctrl_g_opens_collection_switcher_and_enter_sets_working_collection() {
    let (db, mut app, path) = app_with_note("base");
    db.create_collection("Projects", "project notes")
        .expect("projects collection created");

    app.mode = UiMode::Editor;
    run_keys(
        &mut app,
        &db,
        &[Key::Ctrl('g'), Key::Paste("proj".to_string()), Key::Enter],
    );

    assert_eq!(app.status, "working collection: Projects");
    assert_eq!(app.working_collection_name.as_deref(), Some("Projects"));
    assert_eq!(app.mode, UiMode::Editor);

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn ctrl_l_toggles_note_search_collection_limit_with_working_collection() {
    let (db, mut app, path) = app_with_note("base");
    db.create_collection("Projects", "project notes")
        .expect("projects collection created");
    app.execute_terminal_command(&db, "collection choose Projects");
    assert_eq!(app.working_collection_name.as_deref(), Some("Projects"));

    run_keys(&mut app, &db, &[Key::Ctrl('p')]);
    assert_eq!(app.mode, UiMode::Switcher);
    assert_eq!(
        app.switcher.collection_filter_name.as_deref(),
        Some("Projects")
    );

    run_keys(&mut app, &db, &[Key::Ctrl('l')]);
    assert_eq!(app.mode, UiMode::Switcher);
    assert_eq!(app.switcher.collection_filter_id, None);

    run_keys(&mut app, &db, &[Key::Ctrl('l')]);
    assert_eq!(app.mode, UiMode::Switcher);
    assert_eq!(
        app.switcher.collection_filter_name.as_deref(),
        Some("Projects")
    );

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn ctrl_l_does_not_change_note_search_filter_without_working_collection() {
    let (db, mut app, path) = app_with_note("base");
    assert!(app.working_collection_id.is_none());

    run_keys(&mut app, &db, &[Key::Ctrl('p')]);
    assert_eq!(app.mode, UiMode::Switcher);
    assert!(app.switcher.collection_filter_id.is_none());

    run_keys(&mut app, &db, &[Key::Ctrl('l')]);
    assert_eq!(app.mode, UiMode::Switcher);
    assert!(app.switcher.collection_filter_id.is_none());

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn collection_switcher_ctrl_e_opens_edit_dialog_and_saves_updates() {
    let (db, mut app, path) = app_with_note("base");
    let collection = db
        .create_collection("Projects", "")
        .expect("collection created");

    app.mode = UiMode::Editor;
    run_keys(
        &mut app,
        &db,
        &[
            Key::Ctrl('g'),
            Key::Paste("proj".to_string()),
            Key::Ctrl('e'),
            Key::Tab,
            Key::Paste("updated description".to_string()),
            Key::Tab,
            Key::Paste("ops, urgent".to_string()),
            Key::Enter,
            Key::Esc,
        ],
    );

    let updated = db
        .get_collection(&collection.id)
        .expect("collection lookup")
        .expect("collection exists");
    assert_eq!(updated.description, "updated description");
    assert_eq!(
        db.list_collection_default_tags(&collection.id)
            .expect("default tags lookup"),
        vec!["ops".to_string(), "urgent".to_string()]
    );
    assert_eq!(app.mode, UiMode::Editor);

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn collection_purge_replaces_active_note_when_it_is_deleted() {
    let (db, mut app, path) = app_with_note("base");
    let collection = db
        .create_collection("Projects", "project notes")
        .expect("collection created");
    db.set_note_collections("n1", std::slice::from_ref(&collection.id))
        .expect("active note membership set");

    app.execute_terminal_command(&db, "collection purge Projects");
    assert!(
        app.status
            .contains("collection purged: Projects (1 notes deleted)"),
        "unexpected status: {}",
        app.status
    );
    assert_ne!(app.active_note.id, "n1");
    assert!(db.get_note("n1").expect("note lookup").is_none());

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn content_search_reopen_resets_filter_to_working_collection() {
    let (db, mut app, path) = app_with_note("base");
    let work = db
        .create_collection("Work", "work notes")
        .expect("work collection created");
    db.create_collection("Personal", "personal notes")
        .expect("personal collection created");

    app.execute_terminal_command(&db, "collection choose Work");
    assert_eq!(app.working_collection_id.as_deref(), Some(work.id.as_str()));

    run_keys(&mut app, &db, &[Key::Ctrl('p'), Key::Tab]);
    assert_eq!(app.mode, UiMode::ContentSearch);
    assert_eq!(
        app.content_search.collection_filter_id.as_deref(),
        Some(work.id.as_str())
    );

    run_keys(&mut app, &db, &[Key::Ctrl('l')]);
    assert_ne!(
        app.content_search.collection_filter_id.as_deref(),
        Some(work.id.as_str())
    );

    run_keys(&mut app, &db, &[Key::Esc]);
    assert_eq!(app.mode, UiMode::Editor);

    run_keys(&mut app, &db, &[Key::Ctrl('p'), Key::Tab]);
    assert_eq!(app.mode, UiMode::ContentSearch);
    assert_eq!(
        app.content_search.collection_filter_id.as_deref(),
        Some(work.id.as_str())
    );

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn content_search_ctrl_l_toggles_fallback_results_between_working_collection_and_all_notes() {
    let (db, mut app, path) = app_with_note("base");
    let work = db
        .create_collection("Work", "work notes")
        .expect("work collection created");
    let personal = db
        .create_collection("Personal", "personal notes")
        .expect("personal collection created");

    db.save_note("n2", "work note").expect("work note saved");
    db.save_note("n3", "personal note")
        .expect("personal note saved");

    db.set_note_collections("n1", std::slice::from_ref(&work.id))
        .expect("active note membership set");
    db.set_note_collections("n2", std::slice::from_ref(&work.id))
        .expect("work note membership set");
    db.set_note_collections("n3", std::slice::from_ref(&personal.id))
        .expect("personal note membership set");

    app.execute_terminal_command(&db, "collection choose Work");
    assert_eq!(app.working_collection_id.as_deref(), Some(work.id.as_str()));

    run_keys(&mut app, &db, &[Key::Ctrl('p'), Key::Tab]);
    assert_eq!(app.mode, UiMode::ContentSearch);

    let scoped_ids = app
        .content_search
        .results
        .iter()
        .map(|entry| entry.id.as_str())
        .collect::<Vec<_>>();
    assert!(
        scoped_ids.contains(&"n1") && scoped_ids.contains(&"n2"),
        "working-collection fallback should include work notes"
    );
    assert!(
        !scoped_ids.contains(&"n3"),
        "working-collection fallback should exclude non-work notes"
    );

    run_keys(&mut app, &db, &[Key::Ctrl('l')]);
    let unscoped_ids = app
        .content_search
        .results
        .iter()
        .map(|entry| entry.id.as_str())
        .collect::<Vec<_>>();
    assert!(
        unscoped_ids.contains(&"n3"),
        "toggled-all fallback should include notes outside working collection"
    );

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn web_search_query_editing_is_unicode_safe_and_invalidates_stale_results() {
    let (db, mut app, path) = app_with_note("base");
    app.open_web_search(None);

    app.handle_web_search_key(Key::Char('é'));
    app.handle_web_search_key(Key::Char('界'));
    assert_eq!(app.web_search.query, "é界");
    assert_eq!(app.web_search.cursor_col, 2);

    app.web_search
        .results
        .push(app_core::web_search::WebSearchItem {
            title: "Old result".to_string(),
            url: "https://example.com/old".to_string(),
            snippet: String::new(),
            markdown_link: "[Old result](https://example.com/old)".to_string(),
        });
    app.web_search.answer = Some("stale answer".to_string());
    app.web_search.summary = Some("stale summary".to_string());
    app.web_search.answer_card = Some(app_core::web_search::WebSearchAnswerCard {
        title: "Stale".to_string(),
        text: "stale card".to_string(),
        sources: Vec::new(),
    });
    app.web_search.pending = true;
    app.handle_web_search_key(Key::Char('!'));

    assert_eq!(app.web_search.query, "é界!");
    assert_eq!(app.web_search.cursor_col, 3);
    assert!(app.web_search.results.is_empty());
    assert!(app.web_search.answer.is_none());
    assert!(app.web_search.summary.is_none());
    assert!(app.web_search.answer_card.is_none());
    assert!(!app.web_search.pending);

    app.handle_web_search_key(Key::Backspace);
    app.handle_web_search_key(Key::Backspace);
    assert_eq!(app.web_search.query, "é");
    assert_eq!(app.web_search.cursor_col, 1);

    app.handle_web_search_key(Key::Home);
    app.handle_web_search_key(Key::Delete);
    assert!(app.web_search.query.is_empty());
    assert_eq!(app.web_search.cursor_col, 0);

    app.handle_web_search_key(Key::Paste("35\r\ncm".to_string()));
    app.handle_web_search_key(Key::ArrowLeft);
    app.handle_web_search_key(Key::ArrowLeft);
    app.handle_web_search_key(Key::Char(' '));
    assert_eq!(app.web_search.query, "35 cm");
    assert_eq!(app.web_search.cursor_col, 3);

    app.handle_web_search_key(Key::Delete);
    assert_eq!(app.web_search.query, "35 m");

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn question_mark_opens_web_search_from_normal_mode() {
    let (db, mut app, path) = app_with_note("base");
    app.mode = UiMode::Normal;

    app.handle_normal_key(&db, Key::Char('?'))
        .expect("web search shortcut");

    assert_eq!(app.mode, UiMode::WebSearch);
    assert!(app.web_search.query.is_empty());
    assert!(!app.web_search.pending);

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn completed_web_search_marks_terminal_for_redraw_without_an_extra_key() {
    let (db, mut app, path) = app_with_note("base");
    let (tx, rx) = std::sync::mpsc::channel();
    app.open_web_search(None);
    app.web_search.query = "Novak".to_string();
    app.web_search.pending = true;
    app.web_search.rx = Some(rx);
    app.render_state.dirty = false;

    tx.send(WebSearchResponse {
        result: Ok(app_core::web_search::WebSearchResult {
            query: "Novak".to_string(),
            answer: None,
            summary: Some("Novak Djokovic is a Serbian tennis player.".to_string()),
            answer_card: Some(app_core::web_search::WebSearchAnswerCard {
                title: "Best result".to_string(),
                text: "Novak Djokovic is a Serbian tennis player.".to_string(),
                sources: vec![app_core::web_search::WebSearchSource {
                    title: "Novak Djokovic".to_string(),
                    url: "https://example.com/novak".to_string(),
                }],
            }),
            items: vec![app_core::web_search::WebSearchItem {
                title: "Novak Djokovic".to_string(),
                url: "https://example.com/novak".to_string(),
                snippet: "Novak Djokovic is a Serbian tennis player.".to_string(),
                markdown_link: "[Novak Djokovic](https://example.com/novak)".to_string(),
            }],
        }),
    })
    .expect("queued search response");

    app.poll_web_search();

    assert!(app.render_state.dirty);
    assert!(!app.web_search.pending);
    assert_eq!(app.web_search.results.len(), 1);
    assert_eq!(
        app.web_search.summary.as_deref(),
        Some("Novak Djokovic is a Serbian tennis player.")
    );
    assert_eq!(
        app.web_search
            .answer_card
            .as_ref()
            .map(|card| card.title.as_str()),
        Some("Best result")
    );

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn disconnected_web_search_worker_clears_searching_state_and_redraws() {
    let (db, mut app, path) = app_with_note("base");
    let (tx, rx) = std::sync::mpsc::channel::<WebSearchResponse>();
    drop(tx);
    app.open_web_search(None);
    app.web_search.pending = true;
    app.web_search.rx = Some(rx);
    app.render_state.dirty = false;

    app.poll_web_search();

    assert!(app.render_state.dirty);
    assert!(!app.web_search.pending);
    assert!(app.web_search.rx.is_none());
    assert!(app
        .web_search
        .error
        .as_deref()
        .is_some_and(|error| error.contains("worker stopped")));

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn command_bar_arrows_edit_mid_command() {
    let (db, mut app, path) = app_with_note("alpha");
    app.mode = UiMode::CommandBar;
    for ch in "note lck".chars() {
        app.handle_command_bar_key(&db, Key::Char(ch)).expect("type");
    }
    app.handle_command_bar_key(&db, Key::ArrowLeft).expect("left");
    app.handle_command_bar_key(&db, Key::ArrowLeft).expect("left");
    app.handle_command_bar_key(&db, Key::Char('o')).expect("insert");
    assert_eq!(app.command_input, "note lock");

    // Cursor sits after the inserted char: ":" + "note lo" -> column 9.
    let (_, col) = app.cursor_position(24, 80);
    assert_eq!(col, 9);

    app.handle_command_bar_key(&db, Key::Home).expect("home");
    app.handle_command_bar_key(&db, Key::Delete).expect("delete");
    assert_eq!(app.command_input, "ote lock");
    app.handle_command_bar_key(&db, Key::End).expect("end");
    app.handle_command_bar_key(&db, Key::Char('s')).expect("append");
    assert_eq!(app.command_input, "ote locks");

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn today_command_opens_the_daily_note_with_cursor_at_the_end() {
    let (db, mut app, path) = app_with_note("alpha");
    app.execute_terminal_command(&db, "today");
    assert!(
        app.active_note.id.starts_with("daily-"),
        "active note: {}",
        app.active_note.id
    );
    assert!(app.editor.lines[0].starts_with("# "), "template heading: {:?}", app.editor.lines);
    assert_eq!(app.editor.cursor_line, app.editor.lines.len() - 1);

    // Running it again returns to the same note instead of creating another.
    let first_id = app.active_note.id.clone();
    app.execute_terminal_command(&db, "daily");
    assert_eq!(app.active_note.id, first_id);

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}
