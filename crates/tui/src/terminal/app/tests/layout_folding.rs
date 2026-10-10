use super::*;

#[test]
fn load_note_reminder_ghosts_reconciles_shift_without_dropping_adjacent_reminders() {
    let path = temp_db_path();
    let db = Db::open(path.clone()).expect("db opens");
    let note_id = "n1";
    let note = db.save_note(note_id, "a\nb\nc").expect("note saved");
    db.upsert_reminder(note_id, 1, 1_900_000_000_000, "2030-03-10 09:00", "a")
        .expect("reminder a");
    db.upsert_reminder(note_id, 2, 1_900_000_100_000, "2030-03-10 09:05", "b")
        .expect("reminder b");

    let lines = vec![
        "x".to_string(),
        "a".to_string(),
        "b".to_string(),
        "c".to_string(),
    ];
    let ghosts =
        crate::terminal::app::load_note_reminder_ghosts(&db, &note, &lines).expect("load ghosts");
    assert!(ghosts.contains_key(&1));
    assert!(ghosts.contains_key(&2));

    let persisted = db.list_reminders(note_id).expect("list reminders");
    let persisted_lines = persisted
        .iter()
        .map(|entry| entry.line_number)
        .collect::<Vec<_>>();
    assert_eq!(persisted_lines, vec![2, 3]);

    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn load_note_reminder_ghosts_updates_line_text_when_line_changes_in_place() {
    let path = temp_db_path();
    let db = Db::open(path.clone()).expect("db opens");
    let note_id = "n1";
    let note = db.save_note(note_id, "alpha").expect("note saved");
    db.upsert_reminder(note_id, 1, 1_900_000_000_000, "2030-03-10 09:00", "alpha")
        .expect("reminder");

    let lines = vec!["alpha updated".to_string()];
    let ghosts =
        crate::terminal::app::load_note_reminder_ghosts(&db, &note, &lines).expect("load ghosts");
    let ghost = ghosts.get(&0).expect("ghost on first line");
    assert_eq!(ghost.line_text, "alpha updated");

    let persisted = db.list_reminders(note_id).expect("list reminders");
    assert_eq!(persisted[0].line_number, 1);
    assert_eq!(persisted[0].line_text, "alpha updated");

    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn load_note_reminder_ghosts_is_empty_when_note_has_no_reminders() {
    let path = temp_db_path();
    let db = Db::open(path.clone()).expect("db opens");
    let note_id = "n1";
    let note = db
        .save_note(note_id, "alpha\nbeta\ngamma")
        .expect("note saved");

    let lines = vec!["alpha".to_string(), "beta".to_string(), "gamma".to_string()];
    let ghosts =
        crate::terminal::app::load_note_reminder_ghosts(&db, &note, &lines).expect("load ghosts");
    assert!(ghosts.is_empty());

    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn load_note_reminder_ghosts_leaves_a_locked_notes_reminders_alone() {
    let path = temp_db_path();
    let db = Db::open(path.clone()).expect("db opens");
    let note_id = "n1";
    let mut note = db.save_note(note_id, "Plan\nbuy milk").expect("note saved");
    db.upsert_reminder(
        note_id,
        2,
        1_900_000_000_000,
        "2030-03-10 09:00",
        "buy milk",
    )
    .expect("reminder");
    note.access_mode = app_core::storage::NoteAccessMode::Encrypted;
    note.is_unlocked = false;

    // The locked buffer shows a placeholder, not the note's text.
    let ghosts = crate::terminal::app::load_note_reminder_ghosts(&db, &note, &["".to_string()])
        .expect("load ghosts");
    assert!(ghosts.is_empty());
    let persisted = db.list_reminders(note_id).expect("list reminders");
    assert_eq!(persisted[0].line_number, 2);
    assert_eq!(persisted[0].line_text, "buy milk");

    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn load_note_reminder_ghosts_does_not_move_a_deleted_lines_reminder() {
    let path = temp_db_path();
    let db = Db::open(path.clone()).expect("db opens");
    let note_id = "n1";
    let note = db
        .save_note(note_id, "buy milk\ncall Ana")
        .expect("note saved");
    db.upsert_reminder(
        note_id,
        1,
        1_900_000_000_000,
        "2030-03-10 09:00",
        "buy milk",
    )
    .expect("reminder");

    let lines = vec!["call Ana".to_string()];
    let ghosts =
        crate::terminal::app::load_note_reminder_ghosts(&db, &note, &lines).expect("load ghosts");
    assert!(ghosts.is_empty(), "call Ana gets no reminder");

    // Undoing the deletion brings the reminder back.
    let lines = vec!["buy milk".to_string(), "call Ana".to_string()];
    let ghosts =
        crate::terminal::app::load_note_reminder_ghosts(&db, &note, &lines).expect("load ghosts");
    assert_eq!(
        ghosts.get(&0).map(|g| g.line_text.as_str()),
        Some("buy milk")
    );

    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn display_cols_for_prefix_expands_tabs_without_clamping() {
    assert_eq!(display_cols_for_prefix("\tabc", 1), 4);
    assert_eq!(display_cols_for_prefix("\tabc", 4), 7);
}

#[test]
fn display_cols_for_prefix_counts_wide_chars_as_two_columns() {
    // Each emoji occupies 2 terminal columns.
    assert_eq!(display_cols_for_prefix("😀", 1), 2);
    assert_eq!(display_cols_for_prefix("😀a", 1), 2);
    assert_eq!(display_cols_for_prefix("😀a", 2), 3);
    assert_eq!(display_cols_for_prefix("a😀b", 2), 3);
    assert_eq!(display_cols_for_prefix("a😀b", 3), 4);
}

#[test]
fn gutter_width_expands_after_four_digit_line_numbers() {
    assert_eq!(crate::terminal::app::gutter_width_for_visible_lines(1), 6);
    assert_eq!(
        crate::terminal::app::gutter_width_for_visible_lines(9_999),
        6
    );
    assert_eq!(
        crate::terminal::app::gutter_width_for_visible_lines(10_000),
        7
    );
    assert_eq!(
        crate::terminal::app::gutter_width_for_visible_lines(100_000),
        8
    );
}

#[test]
fn cursor_position_respects_expanded_gutter_width() {
    let body = vec!["x"; 10_000].join("\n");
    let (_db, mut app, path) = app_with_note(&body);
    app.editor.cursor_line = 9_999;
    app.editor.cursor_col = 0;
    app.adjust_scroll();

    let (rows, cols) = crate::terminal::input::terminal_size();
    let (_row, cursor_col) = app.cursor_position(rows, cols);
    let expected =
        crate::terminal::app::gutter_width_for_visible_lines(app.visible_line_count()) + 1;

    assert_eq!(expected, 8);
    assert_eq!(cursor_col, expected);

    drop(app);
    cleanup_db_files(&path);
}

#[test]
fn insert_newline_invalidates_fence_checkpoints_from_original_line() {
    let interval = crate::terminal::app::FENCE_CHECKPOINT_INTERVAL;
    let fence_line = interval - 1;
    let mut lines = vec!["plain".to_string(); interval + 40];
    lines[fence_line] = "```".to_string();
    lines[fence_line + 2] = "```".to_string();
    let body = lines.join("\n");
    let (db, mut app, path) = app_with_note(&body);

    let _ = app.fence_state_before_line(interval + 20);
    assert!(app.render_state.fence_checkpoints_valid_through >= 1);

    app.editor.cursor_line = fence_line;
    app.editor.cursor_col = 0;
    app.insert_newline();

    assert_eq!(app.editor.cursor_line, interval);
    assert_eq!(app.render_state.fence_checkpoints_valid_through, 0);

    let in_code_block = app.fence_state_before_line(interval).in_code_block;
    assert!(!in_code_block);

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn insert_paste_multiline_invalidates_fence_checkpoints_from_original_line() {
    let interval = crate::terminal::app::FENCE_CHECKPOINT_INTERVAL;
    let fence_line = interval - 1;
    let mut lines = vec!["plain".to_string(); interval + 40];
    lines[fence_line] = "```".to_string();
    lines[fence_line + 2] = "```".to_string();
    let body = lines.join("\n");
    let (db, mut app, path) = app_with_note(&body);

    let _ = app.fence_state_before_line(interval + 20);
    assert!(app.render_state.fence_checkpoints_valid_through >= 1);

    app.editor.cursor_line = fence_line;
    app.editor.cursor_col = 0;
    app.insert_paste("\n");

    assert_eq!(app.editor.cursor_line, interval);
    assert_eq!(app.render_state.fence_checkpoints_valid_through, 0);

    let in_code_block = app.fence_state_before_line(interval).in_code_block;
    assert!(!in_code_block);

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
#[ignore = "Temporarily disabled heavy large-file load tests"]
fn large_doc_structural_edits_near_eof_keep_fold_maps_and_scroll_stable() {
    let body = vec!["alpha"; 100_000].join("\n");
    let (db, mut app, path) = app_with_note(&body);

    app.mode = UiMode::Editor;
    app.editor.cursor_line = app.editor.lines().len().saturating_sub(1);
    app.editor.cursor_col = line_char_len(app.current_line());
    app.adjust_scroll();
    let insert_scroll_before = app.view.scroll_line;

    run_keys(&mut app, &db, &[Key::Enter]);

    assert_eq!(app.editor.lines().len(), 100_001);
    assert_eq!(app.folds.real_to_visible.len(), app.editor.lines().len());
    assert_eq!(app.folds.hidden_owner.len(), app.editor.lines().len());
    assert_eq!(
        app.folds.placeholder_hidden_lines.len(),
        app.editor.lines().len()
    );
    assert_eq!(app.folds.range_by_start.len(), app.editor.lines().len());
    assert!(app.view.scroll_line > 0);
    assert!(app.view.scroll_line >= insert_scroll_before.saturating_sub(1));

    app.mode = UiMode::Normal;
    app.editor.cursor_line = app.editor.lines().len().saturating_sub(2);
    app.editor.cursor_col = 0;
    app.adjust_scroll();
    let delete_scroll_before = app.view.scroll_line;

    run_keys(&mut app, &db, &[Key::Char('d'), Key::Char('d')]);

    assert_eq!(app.editor.lines().len(), 100_000);
    assert_eq!(app.folds.real_to_visible.len(), app.editor.lines().len());
    assert_eq!(app.folds.hidden_owner.len(), app.editor.lines().len());
    assert_eq!(
        app.folds.placeholder_hidden_lines.len(),
        app.editor.lines().len()
    );
    assert_eq!(app.folds.range_by_start.len(), app.editor.lines().len());
    assert!(app.view.scroll_line > 0);
    assert!(app.view.scroll_line >= delete_scroll_before.saturating_sub(2));

    render_screen(&mut app);

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
#[ignore = "Temporarily disabled heavy large-file load tests"]
fn large_doc_random_tail_edit_stress_keeps_state_consistent() {
    let body = vec!["tail"; 100_000].join("\n");
    let (db, mut app, path) = app_with_note(&body);

    let mut seed = 0xA11CE5EED_u64;
    let mut next_u64 = || {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        seed
    };

    for step_idx in 0..24usize {
        let len = app.editor.lines().len().max(1);
        let tail_window = 500usize.min(len.saturating_sub(1)).max(1);
        let tail_start = len.saturating_sub(tail_window);
        let span = len.saturating_sub(tail_start).max(1);
        let target = tail_start + (next_u64() as usize % span);

        app.editor.cursor_line = target.min(app.editor.lines().len().saturating_sub(1));
        app.editor.cursor_col = 0;
        app.mode = UiMode::Normal;
        app.vim_state = crate::editor_core::vim::VimState::default();
        app.adjust_cursor();
        app.adjust_scroll();

        let op = (next_u64() % 8) as usize;
        match op {
            0 => run_keys(&mut app, &db, &[Key::Char('d'), Key::Char('d')]),
            1 => run_keys(&mut app, &db, &[Key::Char('o'), Key::Char('x'), Key::Esc]),
            2 => run_keys(&mut app, &db, &[Key::Char('O'), Key::Char('x'), Key::Esc]),
            3 => run_keys(&mut app, &db, &[Key::Char('A'), Key::Char('z'), Key::Esc]),
            4 => run_keys(&mut app, &db, &[Key::Char('x')]),
            5 => run_keys(&mut app, &db, &[Key::Char('i'), Key::Enter, Key::Esc]),
            6 => run_keys(&mut app, &db, &[Key::Char('j')]),
            _ => run_keys(&mut app, &db, &[Key::Char('k')]),
        }

        assert!(
            !app.editor.lines().is_empty(),
            "step {step_idx}: lines unexpectedly empty after op {op}"
        );
        assert!(
            app.editor.cursor_line < app.editor.lines().len(),
            "step {step_idx}: cursor_line {} out of bounds {} after op {op}",
            app.editor.cursor_line,
            app.editor.lines().len()
        );
        assert_eq!(
            app.folds.real_to_visible.len(),
            app.editor.lines().len(),
            "step {step_idx}: fold_real_to_visible size mismatch after op {op}"
        );
        assert_eq!(
            app.folds.hidden_owner.len(),
            app.editor.lines().len(),
            "step {step_idx}: fold_hidden_owner size mismatch after op {op}"
        );
        assert_eq!(
            app.folds.placeholder_hidden_lines.len(),
            app.editor.lines().len(),
            "step {step_idx}: fold_placeholder_hidden_lines size mismatch after op {op}"
        );
        assert_eq!(
            app.folds.range_by_start.len(),
            app.editor.lines().len(),
            "step {step_idx}: fold_range_by_start size mismatch after op {op}"
        );
        assert!(
            app.view.scroll_line < app.visible_line_count(),
            "step {step_idx}: scroll_line {} out of visible range {} after op {op}",
            app.view.scroll_line,
            app.visible_line_count()
        );

        if step_idx % 4 == 0 {
            render_screen(&mut app);
        }
    }

    render_screen(&mut app);

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
#[ignore = "Temporarily disabled heavy large-file load tests"]
fn large_doc_deferred_calc_reactivates_when_assignment_is_typed() {
    let body = vec!["plain"; 25_000].join("\n");
    let (db, mut app, path) = app_with_note(&body);

    assert!(app.calc.stale);
    assert!(!app.calc.cached_has_builtin_formula);
    assert!(!app.calc.cached_has_variable_assignment);

    app.mode = UiMode::Editor;
    app.editor.cursor_line = app.editor.lines().len().saturating_sub(1);
    app.editor.cursor_col = 0;
    run_keys(
        &mut app,
        &db,
        &[
            Key::Char('t'),
            Key::Char('o'),
            Key::Char('t'),
            Key::Char('a'),
            Key::Char('l'),
            Key::Char(' '),
            Key::Char(':'),
            Key::Char('='),
            Key::Char(' '),
            Key::Char('2'),
        ],
    );

    assert!(app.calc.cached_has_variable_assignment);
    assert!(!app.calc.stale);
    assert_eq!(app.calc.prev_line_metadata.len(), app.editor.lines().len());
    assert!(app
        .calc
        .prev_line_metadata
        .iter()
        .any(|entry| entry.has_assignment));
    assert!(app.calc.variable_names.iter().any(|name| name == "total"));

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn rendered_line_display_cols_accounts_for_calc_ghost() {
    assert_eq!(rendered_line_display_cols("2 + 2", Some("4")), 9);
    assert_eq!(rendered_line_display_cols("x := 1", Some("2")), 10);
}

#[test]
fn fold_range_builder_detects_heading_fence_list_table_and_paragraph_blocks() {
    let ranges = describe_fold_ranges(&[
        "# top",
        "para one",
        "para two",
        "## sub",
        "```rs",
        "let total = 1;",
        "```",
        "- first",
        "- second",
        "| a |",
        "| - |",
        "| b |",
        "plain one",
        "plain two",
        "",
    ]);

    assert!(ranges.contains(&(0, 13, "heading")));
    assert!(ranges.contains(&(3, 13, "heading")));
    assert!(ranges.contains(&(4, 6, "fence")));
    assert!(ranges.contains(&(7, 8, "list")));
    assert!(ranges.contains(&(9, 11, "table")));
    assert!(ranges.contains(&(1, 2, "paragraph")));
    assert!(ranges.contains(&(12, 13, "paragraph")));
}

#[test]
fn heading_folds_stop_at_same_level_only() {
    let ranges = describe_fold_ranges(&["## parent", "### child", "details", "## sibling", "tail"]);

    assert!(ranges.contains(&(0, 2, "heading")));
    assert!(ranges.contains(&(1, 4, "heading")));
    assert!(ranges.contains(&(3, 4, "heading")));
}

#[test]
fn normal_mode_za_toggles_fold_and_vertical_navigation_uses_virtual_lines() {
    let (db, mut app, path) = app_with_note("# h1\none\ntwo\n# h2\nthree");
    app.mode = UiMode::Normal;
    app.editor.cursor_line = 0;

    run_keys(&mut app, &db, &[Key::Char('z'), Key::Char('a')]);

    assert!(app.folds.collapsed_starts.contains(&0));
    assert_eq!(app.folds.visible_to_real, vec![0, 3, 4]);
    assert_eq!(app.folds.placeholder_hidden_lines[0], Some(2));

    run_keys(&mut app, &db, &[Key::Char('j')]);
    assert_eq!(app.editor.cursor_line, 3);
    assert_eq!(app.current_virtual_line(), 1);

    app.editor.cursor_line = 1;
    app.adjust_cursor();
    assert_eq!(app.editor.cursor_line, 0);

    run_keys(&mut app, &db, &[Key::Char('z'), Key::Char('a')]);
    assert!(!app.folds.collapsed_starts.contains(&0));

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn line_deletes_defer_the_fold_rescan_until_a_fold_command_needs_it() {
    let (db, mut app, path) = app_with_note("# h1\none\ntwo\nthree\n# h2\nfour");
    app.mode = UiMode::Normal;
    app.editor.cursor_line = 1;

    run_keys(&mut app, &db, &[Key::Char('d'), Key::Char('d')]);
    assert!(app.folds.rescan_pending, "rescan waits for idle time");
    assert_eq!(app.folds.visible_to_real.len(), app.editor.lines().len());

    app.editor.cursor_line = 0;
    run_keys(&mut app, &db, &[Key::Char('z'), Key::Char('a')]);
    assert!(!app.folds.rescan_pending);
    assert!(app.folds.collapsed_starts.contains(&0));
    assert_eq!(app.folds.visible_to_real, vec![0, 3, 4]);

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn undo_keeps_collapsed_folds_exact_and_defers_the_rescan_otherwise() {
    let (db, mut app, path) = app_with_note("# h1\none\ntwo\n# h2\nthree\nfour");
    app.mode = UiMode::Normal;

    app.editor.cursor_line = 5;
    run_keys(
        &mut app,
        &db,
        &[Key::Char('d'), Key::Char('d'), Key::Char('u')],
    );
    assert!(app.folds.rescan_pending, "nothing collapsed: rescan waits");

    app.editor.cursor_line = 0;
    run_keys(&mut app, &db, &[Key::Char('z'), Key::Char('a')]);
    app.editor.cursor_line = 4;
    run_keys(
        &mut app,
        &db,
        &[Key::Char('d'), Key::Char('d'), Key::Char('u')],
    );
    assert!(
        !app.folds.rescan_pending,
        "a collapsed fold forces the rescan"
    );
    assert!(app.folds.collapsed_starts.contains(&0));
    assert_eq!(app.folds.visible_to_real, vec![0, 3, 4, 5]);

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn heading_fold_keeps_title_and_preserves_separator_blank_line() {
    let (db, mut app, path) = app_with_note("## 13.05.2026.\nentry\n\n## 14.05.2026.\nnext line");
    app.mode = UiMode::Normal;
    app.editor.cursor_line = 0;

    run_keys(&mut app, &db, &[Key::Char('z'), Key::Char('a')]);

    assert!(app.folds.collapsed_starts.contains(&0));
    assert_eq!(app.folds.placeholder_hidden_lines[0], Some(1));
    assert_eq!(app.folds.visible_to_real, vec![0, 2, 3, 4]);

    let rendered = screen_text(&mut app);
    assert!(rendered.contains("## 13.05.2026."));
    assert!(rendered.contains("1 line folded"));

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn horizontal_scroll_clamps_to_last_visible_window_at_line_end_in_normal_mode() {
    let long = "a".repeat(200);
    let (_db, mut app, path) = app_with_note(&long);
    app.mode = UiMode::Normal;
    app.editor.cursor_line = 0;
    app.editor.cursor_col = line_char_len(app.current_line());
    app.adjust_scroll();

    let (_rows, cols) = crate::terminal::input::terminal_size();
    let available = cols.saturating_sub(crate::terminal::app::GUTTER_WIDTH);
    let expected = crate::terminal::text_utils::line_display_cols(app.current_line())
        .saturating_sub(available);

    assert_eq!(app.view.scroll_col, expected);

    drop(app);
    cleanup_db_files(&path);
}

#[test]
fn horizontal_scroll_allows_insert_end_slot_on_overflow_line_end() {
    let long = "a".repeat(200);
    let (_db, mut app, path) = app_with_note(&long);
    app.mode = UiMode::Editor;
    app.editor.cursor_line = 0;
    app.editor.cursor_col = line_char_len(app.current_line());
    app.adjust_scroll();

    let (_rows, cols) = crate::terminal::input::terminal_size();
    let available = cols.saturating_sub(crate::terminal::app::GUTTER_WIDTH);
    let expected = crate::terminal::text_utils::line_display_cols(app.current_line())
        .saturating_sub(available)
        .saturating_add(1);

    assert_eq!(app.view.scroll_col, expected);

    let line_width = crate::terminal::text_utils::line_display_cols(app.current_line());
    let viewport = crate::terminal::text_utils::compute_line_viewport(
        line_width,
        app.view.scroll_col,
        available,
    );
    assert!(!viewport.has_right_overflow);
    assert_eq!(
        viewport.text_window_col + viewport.text_width,
        line_width + 1
    );

    drop(app);
    cleanup_db_files(&path);
}

#[test]
fn sample_overflow_line_places_cursor_on_last_screen_cell_in_insert_and_normal() {
    let sample = "- [ ] Automatic link handling in the form of [link](link) with optional [link] text update. Show only [link] by default. dfsasjf hsjabshga sfhdghksaghkdfgbghb ahgbsadfhgkbfa ghbf";
    let (_db, mut app, path) = app_with_note(sample);
    let (rows, cols) = crate::terminal::input::terminal_size();
    let available = cols.saturating_sub(crate::terminal::app::GUTTER_WIDTH);
    let line_width = crate::terminal::text_utils::line_display_cols(app.current_line());

    app.editor.cursor_line = 0;
    app.editor.cursor_col = line_char_len(app.current_line());
    app.mode = UiMode::Editor;
    app.adjust_scroll();
    let (_row, col_insert) = app.cursor_position(rows, cols);
    assert_eq!(col_insert, cols);
    assert!(line_width <= app.view.scroll_col.saturating_add(available));

    app.mode = UiMode::Normal;
    app.adjust_scroll();
    let (_row, col_normal) = app.cursor_position(rows, cols);
    assert_eq!(col_normal, cols);
    assert!(line_width <= app.view.scroll_col.saturating_add(available));

    drop(app);
    cleanup_db_files(&path);
}

#[test]
fn normal_mode_cursor_at_logical_line_end_renders_on_last_character_cell() {
    let (_db, mut app, path) = app_with_note("UI settings page");
    let (rows, cols) = crate::terminal::input::terminal_size();
    app.mode = UiMode::Normal;
    app.editor.cursor_line = 0;
    app.editor.cursor_col = line_char_len(app.current_line());
    app.adjust_scroll();

    let (_row, cursor_col) = app.cursor_position(rows, cols);
    let expected = crate::terminal::app::GUTTER_WIDTH + line_char_len(app.current_line());
    assert_eq!(cursor_col, expected);

    drop(app);
    cleanup_db_files(&path);
}

#[test]
fn append_line_end_on_overflow_keeps_last_character_visible_and_cursor_at_screen_edge() {
    let sample = "- [ ] Automatic link handling in the form of [link](link) with optional [link] text update. Show only [link] by default. dfsasjf hsjabshga sfhdghksaghkdfgbghb ahgbsadfhgkbfa ghbf";
    let (db, mut app, path) = app_with_note(sample);
    app.mode = UiMode::Normal;
    app.vim_state = crate::editor_core::vim::VimState::default();
    app.editor.cursor_line = 0;
    app.editor.cursor_col = 0;
    app.view.scroll_col = 0;

    run_keys(&mut app, &db, &[Key::Char('A')]);

    let (rows, cols) = crate::terminal::input::terminal_size();
    let available = cols.saturating_sub(crate::terminal::app::GUTTER_WIDTH);
    let line_width = crate::terminal::text_utils::line_display_cols(app.current_line());
    let viewport = crate::terminal::text_utils::compute_line_viewport(
        line_width,
        app.view.scroll_col,
        available,
    );
    let (_row, cursor_col) = app.cursor_position(rows, cols);

    assert_eq!(app.mode, UiMode::Editor);
    assert_eq!(app.editor.cursor_col, line_char_len(app.current_line()));
    assert_eq!(cursor_col, cols);
    assert_eq!(
        viewport.text_window_col + viewport.text_width,
        line_width + 1,
        "insert mode should reserve one visual end-slot past final character",
    );

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn normal_mode_dollar_then_a_stays_on_same_line_and_enters_insert_at_line_end() {
    let (db, mut app, path) = app_with_note("alpha\nbeta");
    app.mode = UiMode::Normal;
    app.editor.cursor_line = 0;
    app.editor.cursor_col = 0;

    run_keys(&mut app, &db, &[Key::Char('$'), Key::Char('a')]);

    assert_eq!(app.mode, UiMode::Editor);
    assert_eq!(app.editor.cursor_line, 0);
    assert_eq!(app.editor.cursor_col, line_char_len("alpha"));

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn enter_from_overflowing_checklist_repositions_cursor_and_resets_horizontal_scroll() {
    let long = format!("- [ ] {}", "a".repeat(200));
    let (db, mut app, path) = app_with_note(&long);
    app.editor.cursor_line = 0;
    app.editor.cursor_col = line_char_len(app.current_line());
    app.adjust_scroll();
    assert!(app.view.scroll_col > 0);

    app.handle_editor_key(&db, Key::Enter)
        .expect("enter applies");

    assert_eq!(app.editor.cursor_line, 1);
    assert_eq!(app.editor.lines()[1], "- [ ] ");
    assert_eq!(app.editor.cursor_col, line_char_len("- [ ] "));
    assert_eq!(app.view.scroll_col, 0);

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn enter_from_overflowing_unordered_list_repositions_cursor_and_resets_horizontal_scroll() {
    let long = format!("- {}", "a".repeat(200));
    let (db, mut app, path) = app_with_note(&long);
    app.editor.cursor_line = 0;
    app.editor.cursor_col = line_char_len(app.current_line());
    app.adjust_scroll();
    assert!(app.view.scroll_col > 0);

    app.handle_editor_key(&db, Key::Enter)
        .expect("enter applies");

    assert_eq!(app.editor.cursor_line, 1);
    assert_eq!(app.editor.lines()[1], "- ");
    assert_eq!(app.editor.cursor_col, line_char_len("- "));
    assert_eq!(app.view.scroll_col, 0);

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn enter_from_overflowing_ordered_list_repositions_cursor_and_resets_horizontal_scroll() {
    let long = format!("9. {}", "a".repeat(200));
    let (db, mut app, path) = app_with_note(&long);
    app.editor.cursor_line = 0;
    app.editor.cursor_col = line_char_len(app.current_line());
    app.adjust_scroll();
    assert!(app.view.scroll_col > 0);

    app.handle_editor_key(&db, Key::Enter)
        .expect("enter applies");

    assert_eq!(app.editor.cursor_line, 1);
    assert_eq!(app.editor.lines()[1], "10. ");
    assert_eq!(app.editor.cursor_col, line_char_len("10. "));
    assert_eq!(app.view.scroll_col, 0);

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn enter_from_overflowing_table_row_repositions_cursor_and_resets_horizontal_scroll() {
    let long_cell = "a".repeat(180);
    let row = format!("| col |\n| --- |\n| {} |", long_cell);
    let (db, mut app, path) = app_with_note(&row);
    app.editor.cursor_line = 2;
    app.editor.cursor_col = line_char_len(app.current_line());
    app.adjust_scroll();
    assert!(app.view.scroll_col > 0);

    app.handle_editor_key(&db, Key::Enter)
        .expect("enter applies");

    assert_eq!(app.editor.cursor_line, 3);
    assert!(app.editor.lines()[3].starts_with("| "));
    assert!(app.editor.lines()[3].ends_with(" |"));
    assert_eq!(app.editor.cursor_col, 2);
    assert_eq!(app.view.scroll_col, 0);

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn status_bar_shows_mode_pill_position_and_fresh_messages() {
    let (db, mut app, path) = app_with_note("alpha\nbeta");
    app.vim_enabled = true;
    app.mode = UiMode::Normal;
    app.editor.cursor_line = 1;
    app.editor.cursor_col = 2;
    app.status = "saved n1".to_string();

    let rows = screen_rows(&mut app);
    let status = rows.last().expect("status row");
    assert!(status.starts_with(" NORMAL "), "status: {status:?}");
    assert!(status.contains("saved n1"));
    assert!(
        status.trim_end().ends_with("Ln 2, Col 3"),
        "status: {status:?}"
    );

    // Messages expire; the idle loop is told to redraw once.
    let shown = app.render_state.status_shown.clone().expect("tracked");
    app.render_state.status_shown =
        Some((shown.0, Instant::now() - std::time::Duration::from_secs(6)));
    assert!(app.status_message_expired());
    let rows = screen_rows(&mut app);
    assert!(!rows.last().expect("status row").contains("saved n1"));
    assert!(!app.status_message_expired());

    // Mode banners are not repeated next to the pill.
    app.status = "-- NORMAL --".to_string();
    let rows = screen_rows(&mut app);
    assert!(!rows.last().expect("status row").contains("-- NORMAL --"));

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn title_bar_shows_title_first_and_dirty_marker() {
    let (db, mut app, path) = app_with_note("# Weekly review\nbody");
    app.session.dirty = true;
    let rows = screen_rows(&mut app);
    assert!(
        rows[0].starts_with(" Weekly review •  n1"),
        "title: {:?}",
        rows[0]
    );

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn typing_a_fence_into_place_rebuilds_the_fold_ranges() {
    let (db, mut app, path) = app_with_note("~~\ninside\n~~~\nafter");
    app.recompute_folding();
    app.mode = UiMode::Normal;
    app.editor.cursor_line = 0;
    app.editor.cursor_col = 0;
    run_keys(&mut app, &db, &[Key::Char('i'), Key::Char('~'), Key::Esc]);
    assert_eq!(app.editor.lines()[0], "~~~");
    if app.folds.rescan_pending {
        app.recompute_folding();
    }
    assert_eq!(
        app.folds.ranges,
        crate::editor_core::folding::build_fold_ranges(app.editor.lines())
    );

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}
