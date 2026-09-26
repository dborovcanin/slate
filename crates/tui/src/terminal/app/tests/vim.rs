use super::*;

#[test]
fn vim_colon_substitute_replaces_current_line_only() {
    let (db, mut app, path) = app_with_note("alpha alpha\nalpha alpha");
    app.mode = UiMode::Normal;
    app.editor.cursor_line = 1;
    app.editor.cursor_col = 0;

    run_keys(
        &mut app,
        &db,
        &[
            Key::Char(':'),
            Key::Char('s'),
            Key::Char('/'),
            Key::Char('a'),
            Key::Char('l'),
            Key::Char('p'),
            Key::Char('h'),
            Key::Char('a'),
            Key::Char('/'),
            Key::Char('o'),
            Key::Char('m'),
            Key::Char('e'),
            Key::Char('g'),
            Key::Char('a'),
            Key::Char('/'),
            Key::Enter,
        ],
    );

    assert_eq!(
        app.editor.lines,
        vec!["alpha alpha".to_string(), "omega alpha".to_string()]
    );
    assert_eq!(app.mode, UiMode::Normal);
    assert_eq!(app.status, "1 substitution on 1 line");

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn vim_colon_percent_substitute_global_replaces_whole_document() {
    let (db, mut app, path) = app_with_note("alpha alpha\nalpha alpha");
    app.mode = UiMode::Normal;

    run_keys(
        &mut app,
        &db,
        &[
            Key::Char(':'),
            Key::Char('%'),
            Key::Char('s'),
            Key::Char('/'),
            Key::Char('a'),
            Key::Char('l'),
            Key::Char('p'),
            Key::Char('h'),
            Key::Char('a'),
            Key::Char('/'),
            Key::Char('o'),
            Key::Char('m'),
            Key::Char('e'),
            Key::Char('g'),
            Key::Char('a'),
            Key::Char('/'),
            Key::Char('g'),
            Key::Enter,
        ],
    );

    assert_eq!(
        app.editor.lines,
        vec!["omega omega".to_string(), "omega omega".to_string()]
    );
    assert_eq!(app.mode, UiMode::Normal);
    assert_eq!(app.status, "4 substitutions on 2 lines");

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn visual_colon_runs_command_on_preserved_selection() {
    let (db, mut app, path) = app_with_note("alpha\nbeta\ngamma");
    app.mode = UiMode::Normal;

    run_keys(
        &mut app,
        &db,
        &[
            Key::Char('v'),
            Key::Char('j'),
            Key::Char(':'),
            Key::Char('c'),
            Key::Char('l'),
            Key::Char('i'),
            Key::Char('s'),
            Key::Char('t'),
            Key::Enter,
        ],
    );

    assert_eq!(app.mode, UiMode::Normal);
    assert_eq!(app.editor.lines[0], "- [ ] alpha");
    assert_eq!(app.editor.lines[1], "- [ ] beta");
    assert_eq!(app.editor.lines[2], "gamma");

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn visual_colon_keeps_selection_while_command_bar_is_open() {
    let (db, mut app, path) = app_with_note("alpha\nbeta\ngamma");
    app.mode = UiMode::Normal;

    run_keys(
        &mut app,
        &db,
        &[Key::Char('v'), Key::Char('j'), Key::Char(':')],
    );

    assert_eq!(app.mode, UiMode::CommandBar);
    assert!(app.editor.selection_anchor.is_some());
    assert!(app.command_selection.is_some());
    assert!(!app.command_selection_linewise);

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn visual_line_colon_runs_command_on_preserved_selection() {
    let (db, mut app, path) = app_with_note("alpha\nbeta\ngamma");
    app.mode = UiMode::Normal;

    run_keys(
        &mut app,
        &db,
        &[
            Key::Char('V'),
            Key::Char('j'),
            Key::Char(':'),
            Key::Char('o'),
            Key::Char('l'),
            Key::Char('i'),
            Key::Char('s'),
            Key::Char('t'),
            Key::Enter,
        ],
    );

    assert_eq!(app.mode, UiMode::Normal);
    assert_eq!(app.editor.lines[0], "1. alpha");
    assert_eq!(app.editor.lines[1], "2. beta");
    assert_eq!(app.editor.lines[2], "gamma");

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn visual_line_colon_keeps_linewise_selection_while_command_bar_is_open() {
    let (db, mut app, path) = app_with_note("alpha\nbeta\ngamma");
    app.mode = UiMode::Normal;

    run_keys(
        &mut app,
        &db,
        &[Key::Char('V'), Key::Char('j'), Key::Char(':')],
    );

    assert_eq!(app.mode, UiMode::CommandBar);
    assert!(app.editor.selection_anchor.is_some());
    assert!(app.command_selection.is_some());
    assert!(app.command_selection_linewise);

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn visual_mode_supports_counted_navigation_and_doc_motions() {
    let body = (1..=40)
        .map(|i| format!("line {i}"))
        .collect::<Vec<_>>()
        .join("\n");
    let (db, mut app, path) = app_with_note(&body);
    app.mode = UiMode::Normal;

    run_keys(
        &mut app,
        &db,
        &[
            Key::Char('v'),
            Key::Char('1'),
            Key::Char('0'),
            Key::Char('j'),
        ],
    );
    assert_eq!(app.mode, UiMode::Visual);
    assert_eq!(app.editor.cursor_line, 10);

    run_keys(
        &mut app,
        &db,
        &[Key::Char('3'), Key::Char('0'), Key::Char('k')],
    );
    assert_eq!(app.editor.cursor_line, 0);

    run_keys(&mut app, &db, &[Key::Char('$')]);
    assert_eq!(
        app.editor.cursor_col,
        line_char_len(app.current_line()).saturating_sub(1)
    );

    run_keys(&mut app, &db, &[Key::Char('G')]);
    assert_eq!(
        app.editor.cursor_line,
        app.editor.lines.len().saturating_sub(1)
    );

    run_keys(&mut app, &db, &[Key::Char('g'), Key::Char('g')]);
    assert_eq!(app.editor.cursor_line, 0);

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn visual_line_mode_supports_counted_gg_and_g() {
    let body = (1..=40)
        .map(|i| format!("line {i}"))
        .collect::<Vec<_>>()
        .join("\n");
    let (db, mut app, path) = app_with_note(&body);
    app.mode = UiMode::Normal;

    run_keys(
        &mut app,
        &db,
        &[
            Key::Char('V'),
            Key::Char('3'),
            Key::Char('g'),
            Key::Char('g'),
        ],
    );
    assert_eq!(app.mode, UiMode::VisualLine);
    assert_eq!(app.editor.cursor_line, 2);

    run_keys(
        &mut app,
        &db,
        &[Key::Char('3'), Key::Char('0'), Key::Char('G')],
    );
    assert_eq!(app.editor.cursor_line, 29);

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn visual_render_ranges_fill_linewise_and_middle_rows() {
    let (db, mut app, path) = app_with_note("alpha\n\nomega");
    app.mode = UiMode::Visual;
    app.editor.selection_anchor = Some((0, 1));
    app.editor.cursor_line = 2;
    app.editor.cursor_col = 2;

    let mut middle_ranges = Vec::new();
    app.append_visual_highlights(1, &mut middle_ranges);
    assert_eq!(middle_ranges, vec![(0, usize::MAX)]);

    app.mode = UiMode::VisualLine;
    let mut linewise_ranges = Vec::new();
    app.append_visual_highlights(0, &mut linewise_ranges);
    assert_eq!(linewise_ranges, vec![(0, usize::MAX)]);

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn vim_di_pipe_deletes_cell_contents() {
    let (db, mut app, path) = app_with_note("| one | two |");
    app.mode = UiMode::Normal;
    app.editor.cursor_col = 3;

    run_keys(
        &mut app,
        &db,
        &[Key::Char('d'), Key::Char('i'), Key::Char('|')],
    );

    assert_eq!(app.editor.lines, vec!["|  | two |".to_string()]);
    assert_eq!(app.editor.cursor_col, 2);

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn vim_daw_deletes_word_with_padding() {
    let (db, mut app, path) = app_with_note("foo bar baz");
    app.mode = UiMode::Normal;
    app.editor.cursor_col = 5;

    run_keys(
        &mut app,
        &db,
        &[Key::Char('d'), Key::Char('a'), Key::Char('w')],
    );

    assert_eq!(app.editor.lines, vec!["foo baz".to_string()]);
    assert_eq!(app.editor.cursor_col, 4);

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn vim_yaw_yanks_word_with_padding() {
    let (db, mut app, path) = app_with_note("foo bar baz");
    app.mode = UiMode::Normal;
    app.editor.cursor_col = 5;

    run_keys(
        &mut app,
        &db,
        &[Key::Char('y'), Key::Char('a'), Key::Char('w')],
    );

    assert_eq!(app.editor.lines, vec!["foo bar baz".to_string()]);
    assert_eq!(app.clipboard.mode, VimRegisterMode::Charwise);
    assert_eq!(app.clipboard.text, "bar ");

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn vim_dollar_and_d0_delete_line_ranges() {
    let (db, mut app, path) = app_with_note("alpha beta");
    app.mode = UiMode::Normal;
    app.editor.cursor_col = 6;

    run_keys(&mut app, &db, &[Key::Char('d'), Key::Char('$')]);
    assert_eq!(app.editor.lines, vec!["alpha ".to_string()]);

    run_keys(&mut app, &db, &[Key::Char('u')]);
    assert_eq!(app.editor.lines, vec!["alpha beta".to_string()]);

    app.editor.cursor_col = 6;
    run_keys(&mut app, &db, &[Key::Char('d'), Key::Char('0')]);
    assert_eq!(app.editor.lines, vec!["beta".to_string()]);

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn vim_delete_updates_register_without_syncing_clipboard_watch_text() {
    let (db, mut app, path) = app_with_note("alpha beta");
    app.mode = UiMode::Normal;
    app.clipboard_watch.last_text = Some("external clipboard".to_string());
    app.editor.cursor_col = 0;

    run_keys(&mut app, &db, &[Key::Char('d'), Key::Char('w')]);

    assert_eq!(app.editor.lines, vec!["beta".to_string()]);
    assert_eq!(app.clipboard.mode, VimRegisterMode::Charwise);
    assert_eq!(app.clipboard.text, "alpha ");
    assert_eq!(
        app.clipboard_watch.last_text.as_deref(),
        Some("external clipboard")
    );
    assert_eq!(app.last_clipboard_backend, None);

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn visual_delete_updates_register_without_syncing_clipboard_watch_text() {
    let (db, mut app, path) = app_with_note("alpha beta");
    app.mode = UiMode::Visual;
    app.clipboard_watch.last_text = Some("external clipboard".to_string());
    app.editor.selection_anchor = Some((0, 0));
    app.editor.cursor_col = 4;

    assert!(app.apply_visual_selection_action(true));

    assert_eq!(app.editor.lines, vec![" beta".to_string()]);
    assert_eq!(app.clipboard.mode, VimRegisterMode::Charwise);
    assert_eq!(app.clipboard.text, "alpha");
    assert_eq!(
        app.clipboard_watch.last_text.as_deref(),
        Some("external clipboard")
    );
    assert_eq!(app.last_clipboard_backend, None);

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn move_cursor_left_word_clamps_empty_line_cursor_without_underflow() {
    let (db, mut app, path) = app_with_note("alpha\n\nbeta");
    app.mode = UiMode::Normal;
    app.editor.cursor_line = 1;
    app.editor.cursor_col = 4;

    app.move_cursor_left_word();

    assert_eq!(app.editor.cursor_line, 1);
    assert_eq!(app.editor.cursor_col, 0);

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn counted_yank_word_forward_advances_across_words() {
    let (db, mut app, path) = app_with_note("one two three");
    app.mode = UiMode::Normal;
    app.editor.cursor_col = 0;

    run_keys(
        &mut app,
        &db,
        &[Key::Char('2'), Key::Char('y'), Key::Char('w')],
    );

    assert_eq!(app.clipboard.mode, VimRegisterMode::Charwise);
    assert_eq!(app.clipboard.text, "one two ");
    assert_eq!(app.editor.cursor_col, 0);

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn vim_dw_deletes_across_newline_when_motion_crosses_lines() {
    let (db, mut app, path) = app_with_note("alpha\nbeta");
    app.mode = UiMode::Normal;
    app.editor.cursor_line = 0;
    app.editor.cursor_col = line_char_len(app.current_line());

    run_keys(&mut app, &db, &[Key::Char('d'), Key::Char('w')]);

    assert_eq!(app.editor.lines, vec!["alphabeta".to_string()]);
    assert_eq!(app.editor.cursor_line, 0);
    assert_eq!(app.editor.cursor_col, 5);

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn vim_d2w_deletes_two_words_forward() {
    let (db, mut app, path) = app_with_note("foo bar baz qux");
    app.mode = UiMode::Normal;
    app.editor.cursor_col = 0;

    run_keys(
        &mut app,
        &db,
        &[Key::Char('d'), Key::Char('2'), Key::Char('w')],
    );

    assert_eq!(app.editor.lines, vec!["baz qux".to_string()]);
    assert_eq!(app.clipboard.mode, VimRegisterMode::Charwise);
    assert_eq!(app.clipboard.text, "foo bar ");
    assert_eq!(app.editor.cursor_col, 0);

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn vim_d2b_deletes_two_words_backward() {
    let (db, mut app, path) = app_with_note("foo bar baz");
    app.mode = UiMode::Normal;
    app.editor.cursor_col = app.current_line().find("baz").expect("baz");

    run_keys(
        &mut app,
        &db,
        &[Key::Char('d'), Key::Char('2'), Key::Char('b')],
    );

    assert_eq!(app.editor.lines, vec!["baz".to_string()]);
    assert_eq!(app.clipboard.mode, VimRegisterMode::Charwise);
    assert_eq!(app.clipboard.text, "foo bar ");
    assert_eq!(app.editor.cursor_col, 0);

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn vim_de_uses_word_end_semantics_distinct_from_dw() {
    let (db, mut app, path) = app_with_note("foo bar baz");
    app.mode = UiMode::Normal;
    app.editor.cursor_col = 0;

    run_keys(&mut app, &db, &[Key::Char('d'), Key::Char('e')]);
    assert_eq!(app.editor.lines, vec![" bar baz".to_string()]);
    assert_eq!(app.clipboard.mode, VimRegisterMode::Charwise);
    assert_eq!(app.clipboard.text, "foo");
    assert_eq!(app.editor.cursor_col, 0);

    run_keys(&mut app, &db, &[Key::Char('u')]);
    assert_eq!(app.editor.lines, vec!["foo bar baz".to_string()]);

    app.editor.cursor_col = 2;
    run_keys(&mut app, &db, &[Key::Char('d'), Key::Char('e')]);
    assert_eq!(app.editor.lines, vec!["fo baz".to_string()]);
    assert_eq!(app.clipboard.mode, VimRegisterMode::Charwise);
    assert_eq!(app.clipboard.text, "o bar");
    assert_eq!(app.editor.cursor_col, 2);

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn vim_dt_deletes_until_before_target_char() {
    let (db, mut app, path) = app_with_note("alpha beta gamma");
    app.mode = UiMode::Normal;
    app.editor.cursor_col = 0;

    run_keys(
        &mut app,
        &db,
        &[Key::Char('d'), Key::Char('t'), Key::Char('b')],
    );

    assert_eq!(app.editor.lines, vec!["beta gamma".to_string()]);
    assert_eq!(app.clipboard.mode, VimRegisterMode::Charwise);
    assert_eq!(app.clipboard.text, "alpha ");
    assert_eq!(app.editor.cursor_col, 0);

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn vim_d2tx_targets_second_match_of_char() {
    let (db, mut app, path) = app_with_note("a x b x c");
    app.mode = UiMode::Normal;
    app.editor.cursor_col = 0;

    run_keys(
        &mut app,
        &db,
        &[
            Key::Char('d'),
            Key::Char('2'),
            Key::Char('t'),
            Key::Char('x'),
        ],
    );

    assert_eq!(app.editor.lines, vec!["x c".to_string()]);
    assert_eq!(app.clipboard.mode, VimRegisterMode::Charwise);
    assert_eq!(app.clipboard.text, "a x b ");
    assert_eq!(app.editor.cursor_col, 0);

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn vim_yw_yanks_across_newline_when_motion_crosses_lines() {
    let (db, mut app, path) = app_with_note("alpha\nbeta");
    app.mode = UiMode::Normal;
    app.editor.cursor_line = 0;
    app.editor.cursor_col = line_char_len(app.current_line());

    run_keys(&mut app, &db, &[Key::Char('y'), Key::Char('w')]);

    assert_eq!(
        app.editor.lines,
        vec!["alpha".to_string(), "beta".to_string()]
    );
    assert_eq!(app.clipboard.mode, VimRegisterMode::Charwise);
    assert_eq!(app.clipboard.text, "\n");
    assert_eq!(app.editor.cursor_line, 0);
    assert_eq!(app.editor.cursor_col, 5);

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn vim_p_after_yy_pastes_linewise_below_cursor_line() {
    let (db, mut app, path) = app_with_note("one\ntwo");
    app.mode = UiMode::Normal;
    app.editor.cursor_line = 0;
    app.editor.cursor_col = 0;

    run_keys(&mut app, &db, &[Key::Char('y'), Key::Char('y')]);
    assert_eq!(app.clipboard.mode, VimRegisterMode::Linewise);
    assert_eq!(app.clipboard.text, "one");

    run_keys(&mut app, &db, &[Key::Char('p')]);

    assert_eq!(
        app.editor.lines,
        vec!["one".to_string(), "one".to_string(), "two".to_string()]
    );
    assert_eq!(app.editor.cursor_line, 1);
    assert_eq!(app.editor.cursor_col, 0);

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn vim_counted_p_repeats_linewise_register_in_original_order() {
    let (db, mut app, path) = app_with_note("one\ntwo\nthree");
    app.mode = UiMode::Normal;
    app.editor.cursor_line = 0;
    app.editor.cursor_col = 0;

    run_keys(
        &mut app,
        &db,
        &[Key::Char('2'), Key::Char('y'), Key::Char('y')],
    );
    assert_eq!(app.clipboard.mode, VimRegisterMode::Linewise);
    assert_eq!(app.clipboard.text, "one\ntwo");

    run_keys(&mut app, &db, &[Key::Char('2'), Key::Char('p')]);
    assert_eq!(
        app.editor.lines,
        vec![
            "one".to_string(),
            "one".to_string(),
            "two".to_string(),
            "one".to_string(),
            "two".to_string(),
            "two".to_string(),
            "three".to_string(),
        ]
    );
    assert_eq!(app.editor.cursor_line, 1);
    assert_eq!(app.editor.cursor_col, 0);

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn vim_p_after_dollar_uses_charwise_register() {
    let (db, mut app, path) = app_with_note("alpha beta");
    app.mode = UiMode::Normal;
    app.editor.cursor_col = 6;

    run_keys(&mut app, &db, &[Key::Char('d'), Key::Char('$')]);
    assert_eq!(app.clipboard.mode, VimRegisterMode::Charwise);
    assert_eq!(app.clipboard.text, "beta");
    assert_eq!(app.editor.lines, vec!["alpha ".to_string()]);

    run_keys(&mut app, &db, &[Key::Char('p')]);

    assert_eq!(app.editor.lines, vec!["alpha beta".to_string()]);
    assert_eq!(app.editor.cursor_line, 0);
    assert_eq!(app.editor.cursor_col, 10);

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn vim_p_charwise_newline_splits_line_when_register_contains_newline() {
    let (db, mut app, path) = app_with_note("alpha\nbeta");
    app.mode = UiMode::Normal;
    app.editor.cursor_line = 0;
    app.editor.cursor_col = line_char_len(app.current_line());

    run_keys(&mut app, &db, &[Key::Char('y'), Key::Char('w')]);
    assert_eq!(app.clipboard.mode, VimRegisterMode::Charwise);
    assert_eq!(app.clipboard.text, "\n");

    run_keys(&mut app, &db, &[Key::Char('p')]);
    assert_eq!(
        app.editor.lines,
        vec!["alpha".to_string(), "".to_string(), "beta".to_string()]
    );
    assert_eq!(app.editor.cursor_line, 1);
    assert_eq!(app.editor.cursor_col, 0);

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn vim_normal_mode_undo_redo_roundtrip() {
    let (db, mut app, path) = app_with_note("one\ntwo\nthree");
    app.mode = UiMode::Normal;

    run_keys(
        &mut app,
        &db,
        &[Key::Char('j'), Key::Char('d'), Key::Char('d')],
    );
    assert_eq!(
        app.editor.lines,
        vec!["one".to_string(), "three".to_string()]
    );

    run_keys(&mut app, &db, &[Key::Char('u')]);
    assert_eq!(
        app.editor.lines,
        vec!["one".to_string(), "two".to_string(), "three".to_string()]
    );

    run_keys(&mut app, &db, &[Key::Ctrl('r')]);
    assert_eq!(
        app.editor.lines,
        vec!["one".to_string(), "three".to_string()]
    );

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn vim_redo_is_cleared_after_new_edit() {
    let (db, mut app, path) = app_with_note("one\ntwo\nthree");
    app.mode = UiMode::Normal;

    run_keys(&mut app, &db, &[Key::Char('d'), Key::Char('d')]);
    assert_eq!(
        app.editor.lines,
        vec!["two".to_string(), "three".to_string()]
    );

    run_keys(&mut app, &db, &[Key::Char('u')]);
    assert_eq!(
        app.editor.lines,
        vec!["one".to_string(), "two".to_string(), "three".to_string()]
    );

    run_keys(
        &mut app,
        &db,
        &[Key::Char('j'), Key::Char('d'), Key::Char('d')],
    );
    assert_eq!(
        app.editor.lines,
        vec!["one".to_string(), "three".to_string()]
    );

    // New edit after undo should invalidate redo history.
    run_keys(&mut app, &db, &[Key::Ctrl('r')]);
    assert_eq!(
        app.editor.lines,
        vec!["one".to_string(), "three".to_string()]
    );

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn vim_undo_still_works_after_save() {
    let (db, mut app, path) = app_with_note("one\ntwo");
    app.mode = UiMode::Normal;

    run_keys(&mut app, &db, &[Key::Char('d'), Key::Char('d')]);
    assert_eq!(app.editor.lines, vec!["two".to_string()]);

    app.save(&db).expect("save succeeds");
    run_keys(&mut app, &db, &[Key::Char('u')]);
    assert_eq!(app.editor.lines, vec!["one".to_string(), "two".to_string()]);

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn undo_exhaustion_keeps_latest_cursor_location() {
    let (db, mut app, path) = app_with_note("one\ntwo");
    app.mode = UiMode::Editor;
    app.editor.cursor_line = 1;
    app.editor.cursor_col = 1;

    app.handle_editor_key(&db, Key::Char('x'))
        .expect("insert char");
    assert_eq!(app.editor.lines[1], "txwo");
    assert_eq!(app.editor.cursor_line, 1);
    assert_eq!(app.editor.cursor_col, 2);

    app.undo(&db);

    assert_eq!(app.editor.lines, vec!["one".to_string(), "two".to_string()]);
    assert_eq!(app.editor.cursor_line, 1);
    assert_eq!(app.editor.cursor_col, 2);

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn vim_remind_set_is_undoable_and_redoable() {
    let (db, mut app, path) = app_with_note("task");
    app.mode = UiMode::Normal;

    run_keys(
        &mut app,
        &db,
        &[
            Key::Char(':'),
            Key::Char('r'),
            Key::Char('e'),
            Key::Char('m'),
            Key::Char('i'),
            Key::Char('n'),
            Key::Char('d'),
            Key::Enter,
        ],
    );
    assert_eq!(app.mode, UiMode::DatePicker);

    run_keys(&mut app, &db, &[Key::Enter]);
    assert!(!app.reminder_ghosts.is_empty());
    assert_eq!(
        db.list_reminders(&app.active_note.id)
            .expect("list reminders after remind set")
            .len(),
        1
    );

    run_keys(&mut app, &db, &[Key::Char('u')]);
    assert!(app.reminder_ghosts.is_empty());
    assert!(db
        .list_reminders(&app.active_note.id)
        .expect("list reminders after undo")
        .is_empty());

    run_keys(&mut app, &db, &[Key::Ctrl('r')]);
    assert_eq!(
        db.list_reminders(&app.active_note.id)
            .expect("list reminders after redo")
            .len(),
        1
    );

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn vim_n_and_shift_n_cycle_last_search_matches() {
    let (db, mut app, path) = app_with_note("alpha\nbeta alpha\nalpha");
    app.mode = UiMode::Normal;

    run_keys(
        &mut app,
        &db,
        &[
            Key::Char('/'),
            Key::Char('a'),
            Key::Char('l'),
            Key::Char('p'),
            Key::Char('h'),
            Key::Char('a'),
            Key::Enter,
        ],
    );
    assert_eq!(app.search.query, "alpha");
    assert!(!app.search.matches.is_empty());
    assert_eq!((app.editor.cursor_line, app.editor.cursor_col), (0, 0));

    run_keys(&mut app, &db, &[Key::Char('n')]);
    assert_eq!((app.editor.cursor_line, app.editor.cursor_col), (1, 5));

    run_keys(&mut app, &db, &[Key::Char('N')]);
    assert_eq!((app.editor.cursor_line, app.editor.cursor_col), (0, 0));

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn vim_macros_record_and_replay_normal_mode_actions() {
    let (db, mut app, path) = app_with_note("one\ntwo\nthree");
    app.mode = UiMode::Normal;
    app.editor.cursor_line = 0;
    app.editor.cursor_col = 0;

    run_keys(
        &mut app,
        &db,
        &[
            Key::Char('q'),
            Key::Char('a'),
            Key::Char('j'),
            Key::Char('d'),
            Key::Char('d'),
            Key::Char('q'),
        ],
    );
    assert_eq!(
        app.editor.lines,
        vec!["one".to_string(), "three".to_string()]
    );

    run_keys(&mut app, &db, &[Key::Char('@'), Key::Char('a')]);
    assert_eq!(app.editor.lines, vec!["one".to_string()]);

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn vim_macros_support_counted_playback() {
    let (db, mut app, path) = app_with_note("abcdef");
    app.mode = UiMode::Normal;
    app.editor.cursor_col = 0;

    run_keys(
        &mut app,
        &db,
        &[
            Key::Char('q'),
            Key::Char('a'),
            Key::Char('x'),
            Key::Char('q'),
        ],
    );
    assert_eq!(app.editor.lines, vec!["bcdef".to_string()]);

    run_keys(
        &mut app,
        &db,
        &[Key::Char('2'), Key::Char('@'), Key::Char('a')],
    );
    assert_eq!(app.editor.lines, vec!["def".to_string()]);

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn vim_macros_capture_and_replay_insert_mode_input() {
    let (db, mut app, path) = app_with_note("A");
    app.mode = UiMode::Normal;
    app.editor.cursor_col = 0;

    run_keys(
        &mut app,
        &db,
        &[
            Key::Char('q'),
            Key::Char('a'),
            Key::Char('i'),
            Key::Char('x'),
            Key::Esc,
            Key::Char('q'),
        ],
    );
    assert_eq!(app.editor.lines, vec!["xA".to_string()]);

    run_keys(&mut app, &db, &[Key::Char('@'), Key::Char('a')]);
    assert_eq!(app.editor.lines, vec!["xxA".to_string()]);

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn w_and_b_step_through_table_cells_and_rows() {
    let body = "intro\n| sas   | sasa    | x   |\n| ----- | ------- | --- |\n| dusan | as dasd | y   |\n| ab    | cd      | ef  |\noutro";
    let (db, mut app, path) = app_with_note(body);
    let lines = app.editor.lines.clone();
    let word = |line: usize, text: &str| (line, lines[line].find(text).unwrap());
    app.mode = UiMode::Normal;
    app.editor.cursor_line = 3;
    app.editor.cursor_col = 2;
    let forward = vec![
        word(3, "as"),
        word(3, "dasd"),
        word(3, "y"),
        word(4, "ab"),
        word(4, "cd"),
        word(4, "ef"),
        (5, 0),
    ];
    let mut trace = Vec::new();
    for _ in 0..forward.len() {
        app.handle_key(&db, Key::Char('w')).expect("w");
        trace.push((app.editor.cursor_line, app.editor.cursor_col));
    }
    assert_eq!(trace, forward);

    app.editor.cursor_line = 4;
    app.editor.cursor_col = lines[4].find("ef").unwrap();
    let backward = vec![
        word(4, "cd"),
        word(4, "ab"),
        word(3, "y"),
        word(3, "dasd"),
        word(3, "as"),
        word(3, "dusan"),
        word(1, "x"),
    ];
    let mut trace = Vec::new();
    for _ in 0..backward.len() {
        app.handle_key(&db, Key::Char('b')).expect("b");
        trace.push((app.editor.cursor_line, app.editor.cursor_col));
    }
    assert_eq!(trace, backward, "b skips the delimiter row");

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}
