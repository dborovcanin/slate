use super::*;

#[test]
fn find_calc_segment_range_detects_single_table_expression_cell() {
    let line = "| name | 4+2 |";
    let Some((from, to)) = find_calc_segment_range(line) else {
        panic!("expected table segment");
    };
    assert_eq!(&line[from..to], "4+2");
}

#[test]
fn find_calc_segment_range_detects_builtin_formula_cell() {
    let line = "| name | =avg_col() | 1.91 |";
    let Some((from, to)) = find_calc_segment_range(line) else {
        panic!("expected formula segment");
    };
    assert_eq!(&line[from..to], "=avg_col()");
}

#[test]
fn builtin_formula_label_normalizes_aliases() {
    assert_eq!(
        builtin_formula_label("=avg_col()").as_deref(),
        Some("avg_col()")
    );
    assert_eq!(
        builtin_formula_label(" sum_column ( ) ").as_deref(),
        Some("sum_col()")
    );
    assert_eq!(builtin_formula_label("2+2"), None);
}

#[test]
fn format_formula_display_value_rounds_and_strips_approximation_text() {
    assert_eq!(format_formula_display_value("6.666666"), "6.67");
    assert_eq!(format_formula_display_value("≈ 6.666666"), "6.67");
    assert_eq!(format_formula_display_value("approximately 12.000"), "12");
    assert_eq!(format_formula_display_value("5.555 m"), "5.56 m");
}

#[test]
fn find_table_formula_segment_extracts_cell_bounds_and_label() {
    let line = "| a | =sum_column() | 9 |";
    let Some(seg) = find_table_formula_segment(line) else {
        panic!("expected table formula segment");
    };
    assert_eq!(&line[seg.from_byte..seg.to_byte], "=sum_column()");
    assert_eq!(seg.labels, vec!["sum_col()"]);
    assert!(seg.from_char < seg.to_char);
}

#[test]
fn find_table_formula_segment_extracts_chained_formula_labels_in_order() {
    let line = "| sum_col() * a + avg_col() |";
    let Some(seg) = find_table_formula_segment(line) else {
        panic!("expected table formula segment");
    };
    assert_eq!(
        &line[seg.from_byte..seg.to_byte],
        "sum_col() * a + avg_col()"
    );
    assert_eq!(seg.labels, vec!["sum_col()", "avg_col()"]);
}

#[test]
fn find_table_formula_segment_tracks_full_cell_bounds() {
    let line = "| a | =sum_col() | 9 |";
    let Some(seg) = find_table_formula_segment(line) else {
        panic!("expected table formula segment");
    };
    assert!(seg.cell_from_char < seg.from_char);
    assert!(seg.to_char < seg.cell_to_char);
}

#[test]
fn should_mask_formula_cell_reveals_when_cursor_is_anywhere_in_formula_cell() {
    let line = "| a | =sum_col() |";
    let seg = find_table_formula_segment(line).expect("formula segment");
    assert!(should_mask_formula_cell(false, seg.from_char, &seg));
    assert!(!should_mask_formula_cell(true, seg.cell_from_char, &seg));
    assert!(!should_mask_formula_cell(
        true,
        seg.cell_to_char.saturating_sub(1),
        &seg
    ));
    assert!(should_mask_formula_cell(true, seg.cell_to_char, &seg));
}

#[test]
fn table_cell_navigation_anchor_uses_padding_for_empty_and_word_end_for_non_empty() {
    let line = "| aaa |     | bb  |";
    let first_cell = table_cell_info_at_char(line, 2).expect("first cell");
    assert!(!table_cell_is_empty(&first_cell));
    assert_eq!(table_cell_navigation_anchor(line, &first_cell), 5);

    let empty_cell = table_cell_info_at_char(line, 8).expect("empty cell");
    assert!(table_cell_is_empty(&empty_cell));
    assert_eq!(table_cell_navigation_anchor(line, &empty_cell), 8);

    let third_cell = table_cell_info_at_char(line, 14).expect("third cell");
    assert!(!table_cell_is_empty(&third_cell));
    assert_eq!(table_cell_navigation_anchor(line, &third_cell), 16);
}

#[test]
fn find_calc_segment_range_detects_list_body() {
    let line = "- [ ] subtotal + tax";
    let Some((from, to)) = find_calc_segment_range(line) else {
        panic!("expected list segment");
    };
    assert_eq!(&line[from..to], "subtotal + tax");
}

#[test]
fn compute_calc_results_resolves_reactive_variables() {
    let lines = vec!["x := 4".to_string(), "x + 2".to_string()];
    let results = compute_calc_results(&lines, true);
    assert_eq!(results.len(), 2);
    assert_eq!(results[0].as_deref(), None);
    assert_eq!(results[1].as_deref(), Some("6"));
}

#[test]
fn compute_calc_results_resolves_variables_when_assignment_has_trailer_literal() {
    let lines = vec![
        "total := 12 = 12".to_string(),
        "value := total + 3 = 15".to_string(),
        "value + 1".to_string(),
    ];
    let results = compute_calc_results(&lines, true);
    assert_eq!(
        results,
        vec![None, Some("15".to_string()), Some("16".to_string())]
    );
}

#[test]
fn variable_completion_prefix_resolves_current_query_span() {
    let prefix = extract_variable_completion_prefix("total cost + tax", 10).expect("prefix exists");
    assert_eq!(prefix.from_col, 0);
    assert_eq!(prefix.to_col, 10);
    assert_eq!(prefix.query, "total cost");
}

#[test]
fn variable_completion_prefix_skips_leading_spaces_and_rejects_trailing_space() {
    let prefix = extract_variable_completion_prefix("   total", 8).expect("prefix exists");
    assert_eq!(prefix.from_col, 3);
    assert_eq!(prefix.to_col, 8);
    assert_eq!(prefix.query, "total");

    assert!(extract_variable_completion_prefix("total ", 6).is_none());
}

#[test]
fn variable_suggestions_require_min_chars_and_exclude_exact_match() {
    let variables = vec![
        "total cost".to_string(),
        "tax".to_string(),
        "total revenue".to_string(),
    ];

    assert!(build_variable_suggestions(&variables, "to", 3, 8).is_empty());

    let picks = build_variable_suggestions(&variables, "tot", 3, 8);
    assert_eq!(
        picks,
        vec!["total cost".to_string(), "total revenue".to_string()]
    );

    assert!(build_variable_suggestions(&variables, "total cost", 3, 8).is_empty());
}

#[test]
fn initial_open_without_calc_syntax_keeps_calc_cache_lightweight() {
    let (db, app, path) = app_with_note("plain line\nanother plain line");

    assert!(!app.calc.cached_has_builtin_formula);
    assert!(!app.calc.cached_has_variable_assignment);
    assert!(!app.calc.stale);
    assert_eq!(app.calc.results.len(), app.lines.len());
    assert!(app.calc.results.iter().all(|entry| entry.is_none()));
    assert!(app.calc.cell_results.iter().all(|row| row.is_empty()));
    assert!(app.calc.prev_line_metadata.is_empty());

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn large_note_with_assignments_uses_viewport_calc_on_open() {
    let mut lines = Vec::new();
    lines.push("base := 1".to_string());
    lines.extend((0..2_500).map(|_| "base + 2".to_string()));
    let body = lines.join("\n");
    let (db, app, path) = app_with_note(&body);

    assert!(app.calc_viewport_only);
    assert!(app.calc_last_view_eval_range.is_some());
    assert_eq!(
        app.calc.results.get(1).and_then(|entry| entry.as_deref()),
        Some("3")
    );
    assert_eq!(
        app.calc.results.last().and_then(|entry| entry.as_deref()),
        None
    );

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn viewport_calc_evaluates_new_window_after_scroll() {
    let mut lines = Vec::new();
    lines.push("base := 1".to_string());
    lines.extend((0..2_500).map(|_| "base + 2".to_string()));
    let body = lines.join("\n");
    let (db, mut app, path) = app_with_note(&body);

    let last_idx = app.lines.len().saturating_sub(1);
    assert_eq!(
        app.calc
            .results
            .get(last_idx)
            .and_then(|entry| entry.as_deref()),
        None
    );

    app.cursor_line = last_idx;
    app.adjust_cursor();
    app.adjust_scroll();
    let mut out = Vec::new();
    app.draw(&mut out).expect("draw after scroll");

    assert_eq!(
        app.calc
            .results
            .get(last_idx)
            .and_then(|entry| entry.as_deref()),
        Some("3")
    );

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn set_active_note_without_calc_syntax_skips_full_calc_recompute() {
    let (db, mut app, path) = app_with_note("x := 4\nx + 2");
    assert!(app.calc.cached_has_variable_assignment);
    assert!(!app.calc.prev_line_metadata.is_empty());

    let plain_note = db
        .save_note("n2", "plain line\nstill plain")
        .expect("save plain note");
    app.set_active_note(&db, plain_note)
        .expect("switch to plain note");

    assert!(!app.calc.cached_has_builtin_formula);
    assert!(!app.calc.cached_has_variable_assignment);
    assert!(!app.calc.stale);
    assert_eq!(app.calc.results.len(), app.lines.len());
    assert!(app.calc.results.iter().all(|entry| entry.is_none()));
    assert!(app.calc.cell_results.iter().all(|row| row.is_empty()));
    assert!(app.calc.prev_line_metadata.is_empty());

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn tab_applies_table_calc_with_variables_and_positions_cursor_at_insert_end() {
    let (db, mut app, path) = app_with_note("x := 4\n| value | x + 2 |");
    app.cursor_line = 1;
    app.cursor_col = line_char_len(app.current_line());

    app.handle_editor_key(&db, Key::Tab).expect("tab applies");

    assert_eq!(app.lines[1], "| value | 6 |");
    let expected_byte = app.lines[1].find("6").expect("result exists") + "6".len();
    let expected_col = app.lines[1][..expected_byte].chars().count();
    assert_eq!(app.cursor_col, expected_col);

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn adjust_cursor_snaps_empty_table_cells_to_padding_start() {
    let (db, mut app, path) = app_with_note("| aaa |     | bb  |");
    app.cursor_col = 9;
    app.adjust_cursor();
    assert_eq!(app.cursor_col, 8);

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn arrow_navigation_does_not_jump_across_empty_table_cells() {
    let (db, mut app, path) = app_with_note("| aaa |     | bb  |");
    app.cursor_col = 9;
    app.handle_editor_key(&db, Key::ArrowRight)
        .expect("move to empty anchor");
    assert_eq!(app.cursor_col, 8);

    app.handle_editor_key(&db, Key::ArrowRight)
        .expect("regular right stays in current cell");
    assert_eq!(app.cursor_col, 8);

    app.cursor_col = 8;
    app.handle_editor_key(&db, Key::ArrowLeft)
        .expect("regular left stays in current cell");
    assert_eq!(app.cursor_col, 8);

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn arrow_right_from_content_end_does_not_jump_to_next_cell() {
    let (db, mut app, path) = app_with_note("| aaa | bb  |");
    app.cursor_col = 5; // end of first cell content
    app.handle_editor_key(&db, Key::ArrowRight)
        .expect("stay at current cell content end");
    assert_eq!(app.cursor_col, 5);

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn ctrl_arrow_moves_between_table_cells() {
    let (db, mut app, path) = app_with_note("| aaa | bb  |");
    app.cursor_col = 5; // first cell end
    app.handle_editor_key(&db, Key::CtrlArrowRight)
        .expect("ctrl-right jumps to next cell");
    assert_eq!(app.cursor_col, 10);

    app.handle_editor_key(&db, Key::CtrlArrowLeft)
        .expect("ctrl-left jumps to previous cell");
    assert_eq!(app.cursor_col, 5);

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn ctrl_arrow_does_not_fallback_to_word_motion_inside_table() {
    let (db, mut app, path) = app_with_note("| aaa | bb  |");
    app.cursor_col = 5; // first cell end
    app.handle_editor_key(&db, Key::CtrlArrowLeft)
        .expect("ctrl-left in first cell is constrained");
    assert_eq!(app.cursor_col, 5);

    app.cursor_col = 10; // last cell end
    app.handle_editor_key(&db, Key::CtrlArrowRight)
        .expect("ctrl-right in last cell is constrained");
    assert_eq!(app.cursor_col, 10);

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn backspace_and_delete_are_isolated_within_table_cell() {
    let (db, mut app, path) = app_with_note("| aaa |     | bb  |");
    let original = app.lines[0].clone();
    app.cursor_col = 8; // empty middle cell anchor
    app.handle_editor_key(&db, Key::Backspace)
        .expect("backspace in empty cell");
    assert_eq!(app.lines[0], original);
    assert_eq!(app.cursor_col, 8);

    app.handle_editor_key(&db, Key::Delete)
        .expect("delete in empty cell");
    assert_eq!(app.lines[0], original);
    assert_eq!(app.cursor_col, 8);

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn ctrl_w_removes_table_column_when_header_cell_empty() {
    let text = "| a |  | c |\n| --- | --- | --- |\n| 1 | 2 | 3 |";
    let (db, mut app, path) = app_with_note(text);
    app.cursor_line = 0;
    app.cursor_col = app.lines[0].find("|  |").expect("empty header cell") + 2;

    app.handle_editor_key(&db, Key::Ctrl('w'))
        .expect("ctrl-w removes empty header column");

    assert_eq!(app.lines.len(), 3);
    for line in &app.lines {
        assert_eq!(line.matches('|').count(), 3, "line: {:?}", line);
    }
    assert!(app.lines[2].contains("1"));
    assert!(app.lines[2].contains("3"));
    assert!(!app.lines[2].contains("2"));
    assert_eq!(app.cursor_line, 0);
    assert_eq!(app.cursor_col, 3);

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn ctrl_w_deletes_word_outside_table() {
    let (db, mut app, path) = app_with_note("alpha beta");
    app.cursor_col = app.lines[0].len();

    app.handle_editor_key(&db, Key::Ctrl('w'))
        .expect("ctrl-w deletes previous word");

    assert_eq!(app.lines[0], "alpha ");
    assert_eq!(app.cursor_col, "alpha ".len());

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn ctrl_backspace_and_ctrl_delete_merge_adjacent_table_cells() {
    let (db, mut app, path) = app_with_note("| aaa | bb |");

    app.cursor_col = 8; // start of second cell content
    app.handle_editor_key(&db, Key::CtrlBackspace)
        .expect("ctrl-backspace merges with previous cell");
    assert_eq!(app.lines[0], "| aaa bb |");

    app.lines[0] = "| aaa | bb |".to_string();
    app.cursor_col = 5; // end of first cell content
    app.handle_editor_key(&db, Key::CtrlDelete)
        .expect("ctrl-delete merges with next cell");
    assert_eq!(app.lines[0], "| aaa bb |");

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn ctrl_backspace_and_ctrl_delete_remove_table_column_when_header_cell_empty() {
    let text = "| a |  | c |\n| --- | --- | --- |\n| 1 | 2 | 3 |";
    let assert_middle_column_removed = |lines: &[String]| {
        assert_eq!(lines.len(), 3);
        for line in lines {
            assert_eq!(line.matches('|').count(), 3, "line: {:?}", line);
        }
        assert!(lines[2].contains("1"));
        assert!(lines[2].contains("3"));
        assert!(!lines[2].contains("2"));
    };

    let (db, mut app, path) = app_with_note(text);
    app.cursor_line = 0;
    app.cursor_col = app.lines[0].find("|  |").expect("empty header cell") + 2;
    app.handle_editor_key(&db, Key::CtrlBackspace)
        .expect("ctrl-backspace removes empty header column");
    assert_middle_column_removed(&app.lines);
    assert_eq!(app.cursor_line, 0);
    assert_eq!(app.cursor_col, 3);
    drop(app);
    drop(db);
    cleanup_db_files(&path);

    let (db, mut app, path) = app_with_note(text);
    app.cursor_line = 0;
    app.cursor_col = app.lines[0].find("|  |").expect("empty header cell") + 2;
    app.handle_editor_key(&db, Key::CtrlDelete)
        .expect("ctrl-delete removes empty header column");
    assert_middle_column_removed(&app.lines);
    assert_eq!(app.cursor_line, 0);
    assert_eq!(app.cursor_col, 3);
    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn vertical_movement_into_table_cell_snaps_to_cell_end() {
    let (db, mut app, path) = app_with_note("plain\n| aaa | bb  |");
    app.cursor_line = 0;
    app.cursor_col = 0;
    app.handle_editor_key(&db, Key::ArrowDown)
        .expect("move into table row");
    assert_eq!(app.cursor_line, 1);
    assert_eq!(app.cursor_col, 5);

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn typing_space_in_table_cell_allows_followup_word_input() {
    let (db, mut app, path) = app_with_note("| aaa |");
    app.cursor_col = 5; // end of content
    app.handle_editor_key(&db, Key::Char(' '))
        .expect("insert space");
    app.handle_editor_key(&db, Key::Char('b'))
        .expect("insert next word char");
    assert_eq!(app.lines[0], "| aaa b |");

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn typing_in_table_cell_reflows_column_when_cell_becomes_widest() {
    let (db, mut app, path) = app_with_note("| a | b |\n| --- | --- |\n| 1 | 2 |");
    app.cursor_line = 2;
    app.cursor_col = 3; // end of first cell content in row 3

    run_keys(
        &mut app,
        &db,
        &[
            Key::Char('2'),
            Key::Char('3'),
            Key::Char('4'),
            Key::Char('5'),
        ],
    );

    assert_eq!(app.lines[0], "| a     | b   |");
    assert_eq!(app.lines[1], "| ----- | --- |");
    assert_eq!(app.lines[2], "| 12345 | 2   |");

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn compute_calc_trailer_refresh_rewrites_stale_literal() {
    let out = compute_calc_trailer_refresh("1 + 1 = 2", "3", false, 0);
    let Some((eq_idx, tail)) = out else {
        panic!("expected refresh");
    };
    assert_eq!(eq_idx, 5);
    assert_eq!(tail, " = 3");
}

#[test]
fn compute_calc_trailer_refresh_skips_when_missing_trailer() {
    let out = compute_calc_trailer_refresh("1 + 1", "3", false, 0);
    assert_eq!(out, None);
}

#[test]
fn compute_calc_trailer_refresh_already_in_sync() {
    // Caller passed the eligibility gate but the line's trailer already
    // equals the new result — nothing to do.
    let out = compute_calc_trailer_refresh("1 + 1 = 3", "3", false, 0);
    assert_eq!(out, None);
}

#[test]
fn compute_calc_trailer_refresh_skips_when_cursor_inside_trailer() {
    // Cursor sits on the space before `=`; treat the whole trailer as
    // off-limits so we don't yank text out from under the caret.
    let out = compute_calc_trailer_refresh("1 + 1 = 2", "3", true, 5);
    assert_eq!(out, None);
}

#[test]
fn compute_calc_trailer_refresh_runs_when_cursor_is_before_trailer() {
    let out = compute_calc_trailer_refresh("1 + 1 = 2", "3", true, 0);
    assert!(out.is_some());
}

#[test]
fn recompute_calc_refreshes_stale_trailer_after_variable_change() {
    let (db, mut app, path) = app_with_note("rate := 10\n2 * rate = 20\nother line");
    // Move the cursor out of the trailer so the refresh is not guarded.
    app.cursor_line = 0;
    app.cursor_col = 0;
    // Seed: a recompute now should leave the trailer alone (already in sync).
    app.run_calc_recompute();
    assert_eq!(app.lines[1], "2 * rate = 20");

    // Change the variable definition.
    app.lines[0] = "rate := 15".to_string();
    app.run_calc_recompute();

    // Trailer should have been refreshed from `= 20` to `= 30`.
    assert_eq!(app.lines[1], "2 * rate = 30");
    assert_eq!(app.lines[2], "other line");

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn recompute_calc_does_not_refresh_hand_typed_trailer() {
    // The user typed `= FOO` by hand; it never matched a backend result,
    // so subsequent recomputes must not clobber it even when the left
    // side becomes reactively different.
    let (db, mut app, path) = app_with_note("rate := 10\n2 * rate = FOO");
    app.cursor_line = 0;
    app.cursor_col = 0;
    app.run_calc_recompute();
    assert_eq!(app.lines[1], "2 * rate = FOO");

    app.lines[0] = "rate := 15".to_string();
    app.run_calc_recompute();
    assert_eq!(app.lines[1], "2 * rate = FOO");

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn recompute_calc_skips_refresh_when_cursor_in_trailer() {
    let (db, mut app, path) = app_with_note("rate := 10\n2 * rate = 20");
    app.cursor_line = 0;
    app.cursor_col = 0;
    app.run_calc_recompute();

    // Park the cursor inside the trailer on line 1.
    app.cursor_line = 1;
    app.cursor_col = app.lines[1].chars().count(); // end of line, inside trailer
    app.lines[0] = "rate := 15".to_string();
    app.run_calc_recompute();

    // Untouched because cursor is in the trailer region.
    assert_eq!(app.lines[1], "2 * rate = 20");

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn defer_calc_state_after_edit_clears_full_cached_results_vector() {
    let (db, mut app, path) = app_with_note("1 + 1\n2 + 2\n3 + 3");
    app.calc.results = vec![
        Some("2".to_string()),
        Some("4".to_string()),
        Some("6".to_string()),
    ];
    app.calc.variable_names = vec!["total".to_string()];
    app.cursor_line = 1;

    app.defer_calc_state_after_edit();

    assert_eq!(app.calc.results, vec![None, None, None]);
    assert!(app.calc.variable_names.is_empty());
    assert!(app.calc.stale);

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn tab_applies_checklist_calc_with_variables_and_positions_cursor_at_insert_end() {
    let (db, mut app, path) = app_with_note("base := 10\n- [ ] base + 5");
    app.cursor_line = 1;
    app.cursor_col = line_char_len(app.current_line());

    app.handle_editor_key(&db, Key::Tab).expect("tab applies");

    assert_eq!(app.lines[1], "- [ ] 15");
    assert_eq!(app.cursor_col, app.lines[1].chars().count());

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn variable_autocomplete_popup_appears_after_min_chars_and_supports_selection_keys() {
    let (db, mut app, path) = app_with_note("total cost := 10\ntotal revenue := 20\nto");
    app.cursor_line = 2;
    app.cursor_col = line_char_len(app.current_line());
    app.refresh_variable_autocomplete_popup();
    assert!(!app.variable_autocomplete_popup.visible);

    app.handle_editor_key(&db, Key::Char('t'))
        .expect("typing triggers popup refresh");
    assert!(app.variable_autocomplete_popup.visible);
    assert_eq!(app.variable_autocomplete_popup.selected_index, 0);
    assert_eq!(app.variable_autocomplete_popup.suggestions.len(), 2);

    app.handle_editor_key(&db, Key::ArrowDown)
        .expect("down picks next");
    assert_eq!(app.variable_autocomplete_popup.selected_index, 1);
    assert_eq!(app.cursor_line, 2);
    assert_eq!(app.cursor_col, 3);

    app.handle_editor_key(&db, Key::ArrowUp)
        .expect("up picks previous");
    assert_eq!(app.variable_autocomplete_popup.selected_index, 0);

    app.handle_editor_key(&db, Key::Enter)
        .expect("enter accepts selected suggestion");
    assert_eq!(app.lines[2], "total cost");
    assert_eq!(app.cursor_col, "total cost".chars().count());
    assert!(!app.variable_autocomplete_popup.visible);

    app.lines[2] = "tot".to_string();
    app.cursor_col = 3;
    app.refresh_variable_autocomplete_popup();
    app.handle_editor_key(&db, Key::ArrowDown)
        .expect("down selects second suggestion");
    app.handle_editor_key(&db, Key::Tab)
        .expect("tab accepts selected suggestion");
    assert_eq!(app.lines[2], "total revenue");

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn autocomplete_popup_esc_dismisses_and_tab_fallback_still_accepts_variable() {
    let (db, mut app, path) = app_with_note("total cost := 10\ntotal revenue := 20\ntot");
    app.cursor_line = 2;
    app.cursor_col = line_char_len(app.current_line());
    app.refresh_variable_autocomplete_popup();
    assert!(app.variable_autocomplete_popup.visible);

    app.handle_editor_key(&db, Key::Esc)
        .expect("esc closes autocomplete popup");
    assert_eq!(app.mode, UiMode::Editor);
    assert!(!app.variable_autocomplete_popup.visible);

    app.handle_editor_key(&db, Key::Tab)
        .expect("tab fallback still applies variable autocomplete");
    assert_eq!(app.lines[2], "total cost");
    assert_eq!(app.status, "autocomplete: total cost");

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn autocomplete_popup_closes_on_cursor_movement() {
    let (db, mut app, path) = app_with_note("total cost := 10\ntot");
    app.cursor_line = 1;
    app.cursor_col = line_char_len(app.current_line());
    app.refresh_variable_autocomplete_popup();
    assert!(app.variable_autocomplete_popup.visible);

    app.handle_editor_key(&db, Key::ArrowLeft)
        .expect("left moves cursor");
    assert!(!app.variable_autocomplete_popup.visible);

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn variable_autocomplete_is_disabled_when_active_note_module_is_off() {
    let (db, mut app, path) = app_with_note("total cost := 10\ntot");
    db.set_note_modules("n1", note_modules_with_variables(false))
        .expect("disable variable module");
    let note = db
        .get_note("n1")
        .expect("note lookup")
        .expect("note exists");
    app.set_active_note(&db, note).expect("activate note");
    app.mode = UiMode::Editor;
    app.cursor_line = 1;
    app.cursor_col = line_char_len(app.current_line());
    app.refresh_variable_autocomplete_popup();
    assert!(!app.variable_autocomplete_popup.visible);
    assert!(!app.calc_variables_enabled());

    app.handle_editor_key(&db, Key::Tab)
        .expect("tab falls back when variable module is off");
    assert_eq!(app.lines[1], "tot  ");

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn switching_notes_refreshes_variable_module_gating_immediately() {
    let (db, mut app, path) = app_with_note("total cost := 10\ntot");
    db.set_note_modules("n1", note_modules_with_variables(false))
        .expect("disable n1 variable module");
    let note1 = db.get_note("n1").expect("n1 lookup").expect("n1 exists");
    app.set_active_note(&db, note1).expect("activate n1");
    app.mode = UiMode::Editor;
    app.cursor_line = 1;
    app.cursor_col = line_char_len(app.current_line());
    app.refresh_variable_autocomplete_popup();
    assert!(!app.variable_autocomplete_popup.visible);

    db.save_note("n2", "total cost := 10\ntot")
        .expect("save n2");
    db.set_note_modules("n2", note_modules_with_variables(true))
        .expect("enable n2 variable module");
    let note2 = db.get_note("n2").expect("n2 lookup").expect("n2 exists");
    app.set_active_note(&db, note2).expect("activate n2");
    app.mode = UiMode::Editor;
    app.cursor_line = 1;
    app.cursor_col = line_char_len(app.current_line());
    app.refresh_variable_autocomplete_popup();
    assert!(app.variable_autocomplete_popup.visible);
    assert!(app.calc_variables_enabled());

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn tab_accepts_variable_autocomplete_for_active_prefix() {
    let (db, mut app, path) = app_with_note("total cost := 10\ntot");
    app.cursor_line = 1;
    app.cursor_col = line_char_len(app.current_line());
    assert!(!app.variable_autocomplete_popup.visible);

    let hint = app
        .variable_autocomplete_status_hint()
        .expect("autocomplete hint");
    assert!(hint.contains("total cost"));

    app.handle_editor_key(&db, Key::Tab)
        .expect("tab accepts autocomplete");

    assert_eq!(app.lines[1], "total cost");
    assert_eq!(app.cursor_col, "total cost".chars().count());
    assert_eq!(app.status, "autocomplete: total cost");

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}
