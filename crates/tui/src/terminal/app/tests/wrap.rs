use super::*;

fn wrapped_app(body: &str) -> (Db, TerminalApp, PathBuf) {
    let (db, mut app, path) = app_with_note(body);
    app.render_state.wrap_lines = true;
    (db, app, path)
}

/// A line of `words` distinct words, long enough to wrap in an 80-col test screen.
fn long_line(tag: &str, words: usize) -> String {
    (0..words)
        .map(|i| format!("{tag}{i:02}"))
        .collect::<Vec<_>>()
        .join(" ")
}

fn editor_rows(rows: &[String]) -> &[String] {
    // Row 0 is the title bar; the last row is the status bar.
    &rows[1..rows.len() - 1]
}

#[test]
fn long_line_wraps_onto_continuation_rows_with_blank_gutter() {
    let body = format!("{}\nnext line", long_line("word", 20));
    let (db, mut app, path) = wrapped_app(&body);

    let rows = screen_rows(&mut app);
    let editor = editor_rows(&rows);
    assert!(editor[0].trim_start().starts_with("1  word00"));
    assert!(
        editor[1].starts_with("   ") && editor[1].contains("word"),
        "continuation row should have a blank gutter and more words: {:?}",
        editor[1]
    );
    let next_idx = editor
        .iter()
        .position(|row| row.contains("next line"))
        .expect("second line rendered");
    assert!(next_idx >= 2, "second line should follow the wrapped rows");
    assert!(editor[next_idx].trim_start().starts_with("2  next line"));
    assert!(
        !rows.iter().any(|row| row.contains('>')),
        "no overflow markers when wrapping"
    );

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn cursor_on_continuation_row_is_placed_on_that_row() {
    let body = long_line("word", 20);
    let (db, mut app, path) = wrapped_app(&body);
    app.mode = UiMode::Editor;
    app.editor.cursor_col = line_char_len(&app.editor.lines[0]);
    app.adjust_scroll();

    let (rows, cursor) = render_screen(&mut app);
    assert_eq!(
        app.view.scroll_col, 0,
        "wrapped lines never scroll horizontally"
    );
    let cursor_row = usize::from(cursor.row);
    // Buffer row 1 is the first editor row; the line end is on a later row.
    assert!(cursor_row >= 2, "cursor should be below the first line row");
    assert!(
        rows[cursor_row].contains("word19"),
        "cursor row should hold the end of the line: {:?}",
        rows[cursor_row]
    );

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn scrolls_until_wrapped_rows_above_leave_room_for_the_cursor_line() {
    // 30 lines that each wrap to at least two rows, cursor on the last one.
    let body = (0..30)
        .map(|i| long_line(&format!("l{i:02}w"), 18))
        .collect::<Vec<_>>()
        .join("\n");
    let (db, mut app, path) = wrapped_app(&body);
    app.editor.cursor_line = 20;
    app.editor.cursor_col = 0;
    app.adjust_scroll();
    let logical_scroll = app.view.scroll_line;

    let (rows, cursor) = render_screen(&mut app);
    assert!(
        app.view.scroll_line > logical_scroll,
        "rendering should scroll further to fit wrapped rows"
    );
    let cursor_row = usize::from(cursor.row);
    assert!(cursor_row >= 1 && cursor_row < rows.len() - 1);
    assert!(
        rows[cursor_row].contains("l20w00"),
        "cursor row: {:?}",
        rows[cursor_row]
    );

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn tables_stay_unwrapped_with_overflow_marker() {
    let cells = (0..12)
        .map(|i| format!("column{i:02}"))
        .collect::<Vec<_>>()
        .join(" | ");
    let body = format!("| {cells} |\n| --- |\nafter");
    let (db, mut app, path) = wrapped_app(&body);
    app.editor.cursor_line = 2;

    let rows = screen_rows(&mut app);
    let editor = editor_rows(&rows);
    assert!(
        editor[0].trim_end().ends_with('>'),
        "table row: {:?}",
        editor[0]
    );
    assert!(
        editor[1].trim_start().starts_with("2  |"),
        "table stays one row per line"
    );

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn nowrap_mode_keeps_horizontal_scrolling() {
    let body = long_line("word", 20);
    let (db, mut app, path) = app_with_note(&body);
    app.mode = UiMode::Editor;
    app.editor.cursor_col = line_char_len(&app.editor.lines[0]);
    app.adjust_scroll();
    assert!(app.view.scroll_col > 0);
    let rows = screen_rows(&mut app);
    assert!(editor_rows(&rows)[0].contains('<'));

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

// In the 80-column test screen the text area is 74 columns, so a line of
// "wordNN" tokens puts word00..word09 on row 0 and word10.. on row 1.
const ROW1_START: usize = 70;

#[test]
fn gj_and_gk_move_between_screen_rows_keeping_the_column() {
    let body = format!("{}\nnext", long_line("word", 20));
    let (db, mut app, path) = wrapped_app(&body);
    app.mode = UiMode::Normal;
    app.editor.cursor_col = 7;

    app.move_cursor_screen(false, 1);
    assert_eq!(app.editor.cursor_line, 0);
    assert_eq!(app.editor.cursor_col, ROW1_START + 7);

    app.move_cursor_screen(true, 1);
    assert_eq!(app.editor.cursor_col, 7);

    app.move_cursor_screen(false, 2);
    assert_eq!(
        app.editor.cursor_line, 1,
        "gj past the last row enters the next line"
    );

    app.move_cursor_screen(true, 1);
    assert_eq!(
        app.editor.cursor_line, 0,
        "gk from the next line lands on the last row"
    );
    assert!(app.editor.cursor_col >= ROW1_START);

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn gj_keys_route_to_screen_motion_in_vim_mode() {
    let body = long_line("word", 20);
    let (db, mut app, path) = wrapped_app(&body);
    app.vim_enabled = true;
    app.mode = UiMode::Normal;
    app.editor.cursor_col = 3;
    run_keys(&mut app, &db, &[Key::Char('g'), Key::Char('j')]);
    assert_eq!(app.editor.cursor_col, ROW1_START + 3);

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn arrow_keys_move_by_screen_row_on_wrapped_lines_in_editor_mode() {
    let body = format!("{}\nnext", long_line("word", 20));
    let (db, mut app, path) = wrapped_app(&body);
    app.mode = UiMode::Editor;
    app.editor.cursor_col = 3;

    app.handle_editor_key(&db, Key::ArrowDown)
        .expect("arrow down");
    assert_eq!(
        (app.editor.cursor_line, app.editor.cursor_col),
        (0, ROW1_START + 3)
    );
    app.handle_editor_key(&db, Key::ArrowUp).expect("arrow up");
    assert_eq!((app.editor.cursor_line, app.editor.cursor_col), (0, 3));

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn line_taller_than_the_screen_scrolls_by_rows_to_the_cursor() {
    // ~250 words wrap to more rows than the 22-row editor area.
    let body = format!("intro\n{}", long_line("w", 250).replace("w", "word"));
    let (db, mut app, path) = wrapped_app(&body);
    app.mode = UiMode::Editor;
    app.editor.cursor_line = 1;
    app.editor.cursor_col = line_char_len(&app.editor.lines[1]);
    app.adjust_scroll();

    let (rows, cursor) = render_screen(&mut app);
    assert_eq!(
        app.view.scroll_line, 1,
        "the tall cursor line becomes the top line"
    );
    assert!(
        app.view.scroll_row_offset > 0,
        "rows above the cursor are skipped"
    );
    let cursor_row = usize::from(cursor.row);
    assert!(cursor_row >= 1 && cursor_row < rows.len() - 1);
    assert!(
        rows[cursor_row].contains("word249"),
        "cursor row: {:?}",
        rows[cursor_row]
    );

    // Moving back to the start scrolls the offset back to zero.
    app.editor.cursor_col = 0;
    app.adjust_scroll();
    render_screen(&mut app);
    assert_eq!(app.view.scroll_row_offset, 0);

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}
