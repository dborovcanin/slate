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
    let line = "| name | :=avg_col() | 1.91 |";
    let Some((from, to)) = find_calc_segment_range(line) else {
        panic!("expected formula segment");
    };
    assert_eq!(&line[from..to], ":=avg_col()");
}

#[test]
fn builtin_formula_label_normalizes_aliases() {
    assert_eq!(
        builtin_formula_label(":=avg_col()").as_deref(),
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
    let line = "| a | :=sum_column() | 9 |";
    let Some(seg) = find_table_formula_segment(line) else {
        panic!("expected table formula segment");
    };
    assert_eq!(&line[seg.from_byte..seg.to_byte], ":=sum_column()");
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
    let line = "| a | :=sum_col() | 9 |";
    let Some(seg) = find_table_formula_segment(line) else {
        panic!("expected table formula segment");
    };
    assert!(seg.cell_from_char < seg.from_char);
    assert!(seg.to_char < seg.cell_to_char);
}

#[test]
fn should_mask_formula_cell_reveals_when_cursor_is_anywhere_in_formula_cell() {
    let line = "| a | :=sum_col() |";
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
fn formula_cell_stays_masked_and_updates_after_dependent_cell_edit() {
    let (db, mut app, path) =
        app_with_note("| value | calc |\n| --- | --- |\n| 2 | :=(1,1) * 10 |\n| 3 | plain |");

    app.editor.cursor_line = 0;
    app.editor.cursor_col = 0;
    app.run_calc_recompute();

    let rendered_before = screen_text(&mut app);
    assert!(
        rendered_before.contains("20*"),
        "expected masked computed value before edit, got:\n{rendered_before}"
    );
    assert!(
        !rendered_before.contains("| :=(1,1) * 10 |"),
        "formula source leaked into table cell before edit:\n{rendered_before}"
    );

    app.editor.cursor_line = 2;
    let old_value_col = app.editor.lines[2].find('2').expect("numeric source value") + 1;
    app.editor.cursor_col = old_value_col + 1;
    app.handle_editor_key(&db, Key::Backspace)
        .expect("backspace source value");
    app.handle_editor_key(&db, Key::Char('4'))
        .expect("type source value");

    app.run_calc_recompute();

    let rendered_after = screen_text(&mut app);
    assert!(
        rendered_after.contains("40*"),
        "expected masked computed value after edit, got:\n{rendered_after}"
    );
    assert!(
        !rendered_after.contains("| :=(1,1) * 10 |"),
        "formula source leaked into table cell after edit:\n{rendered_after}"
    );

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn table_cell_navigation_anchor_uses_padding_for_empty_and_word_end_for_non_empty() {
    let line = "| aaa |     | bb  |";
    let lines = vec![line.to_string()];
    let first_cell = table_cell_info_at_char(&lines, 0, 2).expect("first cell");
    assert!(!table_cell_is_empty(&first_cell));
    assert_eq!(table_cell_navigation_anchor(line, &first_cell), 5);

    let empty_cell = table_cell_info_at_char(&lines, 0, 8).expect("empty cell");
    assert!(table_cell_is_empty(&empty_cell));
    assert_eq!(table_cell_navigation_anchor(line, &empty_cell), 8);

    let third_cell = table_cell_info_at_char(&lines, 0, 14).expect("third cell");
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
fn variable_completion_candidates_try_word_boundary_suffixes_longest_first() {
    let run = extract_variable_completion_prefix("then pri", 8).expect("prefix exists");
    let queries: Vec<(usize, String)> =
        super::super::calc_helpers::variable_completion_candidates(&run)
            .into_iter()
            .map(|candidate| (candidate.from_col, candidate.query))
            .collect();
    assert_eq!(
        queries,
        vec![(0, "then pri".to_string()), (5, "pri".to_string())]
    );
}

#[test]
fn variable_autocomplete_suggests_from_the_last_word_of_a_prose_line() {
    let (_db, mut app, path) = app_with_note("price := 5\nthen pri");
    app.mode = UiMode::Editor;
    app.run_calc_recompute();
    app.editor.cursor_line = 1;
    app.editor.cursor_col = 8;
    let state = app.variable_autocomplete_state().expect("suggestions");
    assert_eq!(state.suggestions, vec!["price".to_string()]);
    assert_eq!(state.from_col, 5);

    drop(app);
    cleanup_db_files(&path);
}

#[test]
fn variable_autocomplete_still_prefers_multi_word_names() {
    let (_db, mut app, path) = app_with_note("tax rate := 0.2\ntax ra");
    app.mode = UiMode::Editor;
    app.run_calc_recompute();
    app.editor.cursor_line = 1;
    app.editor.cursor_col = 6;
    let state = app.variable_autocomplete_state().expect("suggestions");
    assert_eq!(state.suggestions, vec!["tax rate".to_string()]);
    assert_eq!(state.from_col, 0);

    drop(app);
    cleanup_db_files(&path);
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

fn build_repeated_note(line: &str, line_count: usize) -> String {
    if line_count == 0 {
        return String::new();
    }
    let mut body = String::with_capacity((line.len() + 1).saturating_mul(line_count));
    body.push_str(line);
    for _ in 1..line_count {
        body.push('\n');
        body.push_str(line);
    }
    body
}

fn max_expected_viewport_eval_span(editor_height: usize) -> usize {
    // viewport + prefetch above + prefetch below, with a tiny boundary buffer.
    editor_height.saturating_mul(5).saturating_add(8)
}

#[test]
fn huge_plain_text_note_100k_edits_skip_calc_recompute() {
    let body = build_repeated_note("plain text", 100_000);
    let (db, mut app, path) = app_with_note(&body);

    assert!(!app.calc.cached_has_builtin_formula);
    assert!(!app.calc.cached_has_variable_assignment);
    assert!(!app.calc_runtime.viewport_only);
    assert!(app.calc.prev_line_metadata.is_empty());

    app.editor.cursor_line = 50_000;
    app.editor.cursor_col = line_char_len(app.current_line());
    app.insert_text(" updated");

    assert!(!app.calc_runtime.recompute_pending);
    assert!(!app.calc.stale);
    assert_eq!(
        app.calc
            .results
            .get(50_000)
            .and_then(|value| value.as_deref()),
        None
    );

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn huge_variable_calc_note_100k_uses_viewport_or_minimal_eval_windows() {
    let mut lines = Vec::with_capacity(100_000);
    lines.push("base := 1".to_string());
    lines.extend((0..99_999).map(|_| "base + 2".to_string()));
    let body = lines.join("\n");
    let (db, mut app, path) = app_with_note(&body);

    assert!(app.calc_runtime.viewport_only);
    let initial_range = app
        .calc_runtime
        .last_view_eval_range
        .expect("initial viewport eval range");
    let span_budget = max_expected_viewport_eval_span(app.editor_height());
    assert!(
        initial_range.1.saturating_sub(initial_range.0) <= span_budget,
        "initial viewport span {} exceeds budget {}",
        initial_range.1.saturating_sub(initial_range.0),
        span_budget
    );

    let last_idx = app.editor.lines.len().saturating_sub(1);
    assert_eq!(
        app.calc
            .results
            .get(last_idx)
            .and_then(|value| value.as_deref()),
        None
    );

    app.editor.cursor_line = last_idx;
    app.editor.cursor_col = line_char_len(app.current_line());
    app.adjust_cursor();
    app.adjust_scroll();
    render_screen(&mut app);

    let end_range = app
        .calc_runtime
        .last_view_eval_range
        .expect("end viewport eval range");
    assert!(
        end_range.1.saturating_sub(end_range.0) <= span_budget,
        "end viewport span {} exceeds budget {}",
        end_range.1.saturating_sub(end_range.0),
        span_budget
    );
    assert!(end_range.0 <= last_idx && last_idx < end_range.1);
    assert_eq!(
        app.calc
            .results
            .get(last_idx)
            .and_then(|value| value.as_deref()),
        Some("3")
    );
    assert_eq!(
        app.calc
            .results
            .get(50_000)
            .and_then(|value| value.as_deref()),
        None
    );

    // Same-line change in a huge note should stay on viewport-only refresh.
    let changed_idx = 60_000;
    app.editor.lines[changed_idx] = "base + 20".to_string();
    app.editor.cursor_line = changed_idx;
    app.editor.cursor_col = line_char_len(app.current_line());
    app.adjust_scroll();
    app.mark_edited_from_line(changed_idx);
    assert!(app.calc_runtime.recompute_pending);
    app.last_edit = std::time::Instant::now() - std::time::Duration::from_millis(200);
    app.maybe_recompute_calc_after_idle();
    assert!(!app.calc_runtime.recompute_pending);
    let changed_range = app
        .calc_runtime
        .last_view_eval_range
        .expect("viewport range after edit");
    assert!(
        changed_range.1.saturating_sub(changed_range.0) <= span_budget,
        "post-edit viewport span {} exceeds budget {}",
        changed_range.1.saturating_sub(changed_range.0),
        span_budget
    );
    assert!(changed_range.0 <= changed_idx && changed_idx < changed_range.1);
    assert_eq!(
        app.calc
            .results
            .get(changed_idx)
            .and_then(|value| value.as_deref()),
        Some("21")
    );

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn huge_table_formula_note_100k_uses_minimal_incremental_eval_window() {
    let mut lines = vec!["plain".to_string(); 100_000];
    let table_start = 50_000usize;
    lines[table_start] = "| value |".to_string();
    lines[table_start + 1] = "| --- |".to_string();
    lines[table_start + 2] = "| 2 |".to_string();
    lines[table_start + 3] = "| 3 |".to_string();
    lines[table_start + 4] = "| :=sum_col() |".to_string();
    let body = lines.join("\n");
    let (db, mut app, path) =
        app_with_note_and_modules(&body, note_modules(true, true, false, true));

    assert!(!app.calc_runtime.viewport_only);
    assert_eq!(
        app.calc
            .results
            .get(table_start + 4)
            .and_then(|value| value.as_deref()),
        Some("5")
    );

    let changed_idx = table_start + 2;
    let formula_idx = table_start + 4;
    app.editor.lines[changed_idx] = "| 20 |".to_string();
    app.editor.cursor_line = changed_idx;
    app.editor.cursor_col = line_char_len(app.current_line());
    app.mark_edited_from_line(changed_idx);
    assert!(app.calc_runtime.recompute_pending);

    let plan = crate::editor_core::calc_plan::plan_incremental_calc_from_line_metadata(
        &app.calc.prev_line_metadata,
        &app.calc.results,
        &app.editor.lines,
        &app.calc.line_metadata,
    );
    let suffix_len = app.editor.lines.len().saturating_sub(plan.eval_to);
    let prev_changed_from = plan.eval_from.min(app.calc.prev_line_metadata.len());
    let prev_changed_to = app
        .calc
        .prev_line_metadata
        .len()
        .saturating_sub(suffix_len)
        .max(prev_changed_from);
    let prev_changed_slice = app
        .calc
        .prev_line_metadata
        .get(prev_changed_from..prev_changed_to)
        .unwrap_or(&[]);
    let prev_changed_assignment_names = prev_changed_slice
        .iter()
        .filter_map(|meta| meta.assignment_name.clone())
        .collect::<Vec<_>>();
    let prev_changed_had_assignment = prev_changed_slice.iter().any(|meta| meta.has_assignment);
    let prev_changed_had_builtin_formula = prev_changed_slice
        .iter()
        .any(|meta| meta.has_builtin_formula);
    let eval_window = crate::editor_core::calc_plan::decide_eval_window(
        &crate::editor_core::calc_plan::DecideEvalWindowParams {
            lines: &app.editor.lines,
            changed_from: plan.eval_from,
            changed_to: plan.eval_to,
            has_prev: !app.calc.prev_line_metadata.is_empty(),
            mask: app.calc_feature_mask(),
            prev_changed_assignment_names: &prev_changed_assignment_names,
            prev_changed_had_assignment,
            prev_changed_had_builtin_formula,
            variable_graph: None,
            table_formula_index: None,
        },
    );
    assert!(
        eval_window.eval_to.saturating_sub(eval_window.eval_from) <= 12,
        "unexpectedly wide table eval window: {:?}",
        eval_window
    );

    app.run_calc_recompute();
    assert!(!app.calc_runtime.recompute_pending);
    assert_eq!(
        app.calc
            .results
            .get(formula_idx)
            .and_then(|value| value.as_deref()),
        Some("23")
    );

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn large_mixed_calc_note_defers_full_recompute_until_idle() {
    let mut lines = vec!["plain".to_string(); 2_500];
    lines[0] = "base := 2".to_string();
    let table_start = 10usize;
    lines[table_start] = "| value |".to_string();
    lines[table_start + 1] = "| --- |".to_string();
    lines[table_start + 2] = "| 3 |".to_string();
    lines[table_start + 3] = "| :=sum_col() + base |".to_string();
    let formula_idx = table_start + 3;
    let body = lines.join("\n");
    let (db, mut app, path) = app_with_note(&body);

    assert!(!app.calc_runtime.viewport_only);
    assert!(app.calc.stale);
    assert!(app.calc_runtime.recompute_pending);
    assert_eq!(
        app.calc
            .results
            .get(formula_idx)
            .and_then(|value| value.as_deref()),
        None
    );

    app.last_edit = std::time::Instant::now() - std::time::Duration::from_millis(200);
    app.run_calc_recompute();

    assert!(!app.calc.stale);
    assert!(!app.calc_runtime.recompute_pending);
    assert_eq!(
        app.calc
            .results
            .get(formula_idx)
            .and_then(|value| value.as_deref()),
        Some("5")
    );

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn initial_open_without_calc_syntax_keeps_calc_cache_lightweight() {
    let (db, app, path) = app_with_note("plain line\nanother plain line");

    assert!(!app.calc.cached_has_builtin_formula);
    assert!(!app.calc.cached_has_variable_assignment);
    assert!(!app.calc.stale);
    assert_eq!(app.calc.results.len(), app.editor.lines.len());
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

    assert!(app.calc_runtime.viewport_only);
    assert!(app.calc_runtime.last_view_eval_range.is_some());
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

    let last_idx = app.editor.lines.len().saturating_sub(1);
    assert_eq!(
        app.calc
            .results
            .get(last_idx)
            .and_then(|entry| entry.as_deref()),
        None
    );

    app.editor.cursor_line = last_idx;
    app.adjust_cursor();
    app.adjust_scroll();
    render_screen(&mut app);

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
fn non_cursor_image_line_stays_collapsed_when_cursor_line_has_override_mapping() {
    let body = [
        "| a | :=sum_col() |",
        "| --- | --- |",
        "| 2 | 3 |",
        "![slate full](./assets/slate-full.png)",
    ]
    .join("\n");
    let (db, mut app, path) = app_with_note(&body);

    // Keep cursor on the formula line so cursor-line mapping is populated.
    app.editor.cursor_line = 0;
    app.editor.cursor_col = app.editor.lines[0].find("sum_col").expect("formula label");
    app.adjust_cursor();
    app.adjust_scroll();

    let rendered = screen_text(&mut app);

    assert!(
        rendered.contains("[image: slate full]"),
        "non-cursor image line should render collapsed placeholder, got:\n{rendered}"
    );

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn large_note_structural_edit_recomputes_calc_without_idle_delay() {
    let mut lines = Vec::new();
    lines.push("title".to_string());
    lines.push("base := 1".to_string());
    lines.extend((0..2_500).map(|_| "base + 2".to_string()));
    let body = lines.join("\n");
    let (db, mut app, path) = app_with_note(&body);

    assert!(app.editor.lines.len() >= 2_000);
    assert!(app.calc_runtime.viewport_only);
    assert_eq!(
        app.calc.results.get(2).and_then(|entry| entry.as_deref()),
        Some("3")
    );

    app.editor.cursor_line = 0;
    app.editor.cursor_col = line_char_len(&app.editor.lines[0]);
    app.insert_newline();

    assert!(!app.calc_runtime.recompute_pending);
    assert_eq!(app.calc.results.len(), app.editor.lines.len());
    assert_eq!(
        app.calc.results.get(3).and_then(|entry| entry.as_deref()),
        Some("3")
    );

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn large_note_structural_edit_with_calc_change_recomputes_immediately() {
    let mut lines = Vec::new();
    lines.push("title".to_string());
    lines.push("base := 1".to_string());
    lines.push("4+2".to_string());
    lines.extend((0..2_500).map(|_| "base + 2".to_string()));
    let body = lines.join("\n");
    let (db, mut app, path) = app_with_note(&body);

    assert!(app.editor.lines.len() >= 2_000);
    assert!(app.calc_runtime.viewport_only);
    assert_eq!(
        app.calc.results.get(2).and_then(|entry| entry.as_deref()),
        Some("6")
    );

    app.editor.cursor_line = 2;
    app.editor.cursor_col = 1;
    app.insert_newline();

    assert!(!app.calc_runtime.recompute_pending);
    let changed_left = app.calc.results.get(2).and_then(|entry| entry.as_deref());
    let changed_right = app.calc.results.get(3).and_then(|entry| entry.as_deref());
    assert!(
        changed_left.is_some() || changed_right.is_some(),
        "changed calc lines should be recomputed immediately"
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
    assert_eq!(app.calc.results.len(), app.editor.lines.len());
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
    app.editor.cursor_line = 1;
    app.editor.cursor_col = line_char_len(app.current_line());

    app.handle_editor_key(&db, Key::Tab).expect("tab applies");

    assert_eq!(app.editor.lines[1], "| value | 6 |");
    let expected_byte = app.editor.lines[1].find("6").expect("result exists") + "6".len();
    let expected_col = app.editor.lines[1][..expected_byte].chars().count();
    assert_eq!(app.editor.cursor_col, expected_col);

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn adjust_cursor_snaps_empty_table_cells_to_padding_start() {
    let (db, mut app, path) = app_with_note("| aaa |     | bb  |");
    app.editor.cursor_col = 9;
    app.adjust_cursor();
    assert_eq!(app.editor.cursor_col, 8);

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn arrow_navigation_does_not_jump_across_empty_table_cells() {
    let (db, mut app, path) = app_with_note("| aaa |     | bb  |");
    app.editor.cursor_col = 9;
    app.handle_editor_key(&db, Key::ArrowRight)
        .expect("move to empty anchor");
    assert_eq!(app.editor.cursor_col, 8);

    app.handle_editor_key(&db, Key::ArrowRight)
        .expect("regular right stays in current cell");
    assert_eq!(app.editor.cursor_col, 8);

    app.editor.cursor_col = 8;
    app.handle_editor_key(&db, Key::ArrowLeft)
        .expect("regular left stays in current cell");
    assert_eq!(app.editor.cursor_col, 8);

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn arrow_right_from_content_end_moves_to_next_cell_edit_start() {
    let (db, mut app, path) = app_with_note("| aaa | bb  |");
    app.editor.cursor_col = 5; // end of first cell content
    app.handle_editor_key(&db, Key::ArrowRight)
        .expect("jump to next cell edit start");
    assert_eq!(app.editor.cursor_col, 8);

    app.handle_editor_key(&db, Key::ArrowLeft)
        .expect("jump back to previous cell content end");
    assert_eq!(app.editor.cursor_col, 5);

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn ctrl_arrow_moves_between_table_cells() {
    let (db, mut app, path) = app_with_note("| aaa | bb  |");
    app.editor.cursor_col = 5; // first cell end
    app.handle_editor_key(&db, Key::CtrlArrowRight)
        .expect("ctrl-right jumps to next cell");
    assert_eq!(app.editor.cursor_col, 10);

    app.handle_editor_key(&db, Key::CtrlArrowLeft)
        .expect("ctrl-left jumps to previous cell");
    assert_eq!(app.editor.cursor_col, 5);

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn ctrl_arrow_does_not_fallback_to_word_motion_inside_table() {
    let (db, mut app, path) = app_with_note("| aaa | bb  |");
    app.editor.cursor_col = 5; // first cell end
    app.handle_editor_key(&db, Key::CtrlArrowLeft)
        .expect("ctrl-left in first cell is constrained");
    assert_eq!(app.editor.cursor_col, 5);

    app.editor.cursor_col = 10; // last cell end
    app.handle_editor_key(&db, Key::CtrlArrowRight)
        .expect("ctrl-right in last cell is constrained");
    assert_eq!(app.editor.cursor_col, 10);

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn backspace_and_delete_are_isolated_within_table_cell() {
    let (db, mut app, path) = app_with_note("| aaa |     | bb  |");
    let original = app.editor.lines[0].clone();
    app.editor.cursor_col = 8; // empty middle cell anchor
    app.handle_editor_key(&db, Key::Backspace)
        .expect("backspace in empty cell");
    assert_eq!(app.editor.lines[0], original);
    assert_eq!(app.editor.cursor_col, 8);

    app.handle_editor_key(&db, Key::Delete)
        .expect("delete in empty cell");
    assert_eq!(app.editor.lines[0], original);
    assert_eq!(app.editor.cursor_col, 8);

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn ctrl_w_removes_table_column_when_header_cell_empty() {
    let text = "| a |  | c |\n| --- | --- | --- |\n| 1 | 2 | 3 |";
    let (db, mut app, path) = app_with_note(text);
    app.editor.cursor_line = 0;
    app.editor.cursor_col = app.editor.lines[0].find("|  |").expect("empty header cell") + 2;

    app.handle_editor_key(&db, Key::Ctrl('w'))
        .expect("ctrl-w removes empty header column");

    assert_eq!(app.editor.lines.len(), 3);
    for line in &app.editor.lines {
        assert_eq!(line.matches('|').count(), 3, "line: {:?}", line);
    }
    assert!(app.editor.lines[2].contains("1"));
    assert!(app.editor.lines[2].contains("3"));
    assert!(!app.editor.lines[2].contains("2"));
    assert_eq!(app.editor.cursor_line, 0);
    assert_eq!(app.editor.cursor_col, 3);

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn ctrl_w_deletes_word_outside_table() {
    let (db, mut app, path) = app_with_note("alpha beta");
    app.editor.cursor_col = app.editor.lines[0].len();

    app.handle_editor_key(&db, Key::Ctrl('w'))
        .expect("ctrl-w deletes previous word");

    assert_eq!(app.editor.lines[0], "alpha ");
    assert_eq!(app.editor.cursor_col, "alpha ".len());

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn ctrl_w_deletes_word_inside_table_cell_when_no_structural_merge_applies() {
    let text = "| h |\n| --- |\n| alpha beta |";
    let (db, mut app, path) = app_with_note(text);
    app.editor.cursor_line = 2;
    app.editor.cursor_col =
        app.editor.lines[2].find("beta").expect("beta") + "beta".chars().count();

    app.handle_editor_key(&db, Key::Ctrl('w'))
        .expect("ctrl-w deletes previous word inside cell");

    let cells = crate::editor_core::table::split_table_cells(&app.editor.lines[2]);
    assert_eq!(cells[0], "alpha");

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn ctrl_backspace_and_ctrl_delete_merge_adjacent_table_cells() {
    let (db, mut app, path) = app_with_note("| aaa | bb |");

    app.editor.cursor_col = 8; // start of second cell content
    app.handle_editor_key(&db, Key::CtrlBackspace)
        .expect("ctrl-backspace merges with previous cell");
    assert_eq!(app.editor.lines[0], "| aaa bb |");

    app.editor.lines[0] = "| aaa | bb |".to_string();
    app.editor.cursor_col = 5; // end of first cell content
    app.handle_editor_key(&db, Key::CtrlDelete)
        .expect("ctrl-delete merges with next cell");
    assert_eq!(app.editor.lines[0], "| aaa bb |");

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
    app.editor.cursor_line = 0;
    app.editor.cursor_col = app.editor.lines[0].find("|  |").expect("empty header cell") + 2;
    app.handle_editor_key(&db, Key::CtrlBackspace)
        .expect("ctrl-backspace removes empty header column");
    assert_middle_column_removed(&app.editor.lines);
    assert_eq!(app.editor.cursor_line, 0);
    assert_eq!(app.editor.cursor_col, 3);
    drop(app);
    drop(db);
    cleanup_db_files(&path);

    let (db, mut app, path) = app_with_note(text);
    app.editor.cursor_line = 0;
    app.editor.cursor_col = app.editor.lines[0].find("|  |").expect("empty header cell") + 2;
    app.handle_editor_key(&db, Key::CtrlDelete)
        .expect("ctrl-delete removes empty header column");
    assert_middle_column_removed(&app.editor.lines);
    assert_eq!(app.editor.cursor_line, 0);
    assert_eq!(app.editor.cursor_col, 3);
    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn vertical_arrow_movement_into_table_preserves_text_column() {
    let (db, mut app, path) = app_with_note("plain\n| aaa | bb  |");
    app.editor.cursor_line = 0;
    app.editor.cursor_col = 1;
    app.handle_editor_key(&db, Key::ArrowDown)
        .expect("move into table row");
    assert_eq!(app.editor.cursor_line, 1);
    assert_eq!(app.editor.cursor_col, 1);

    app.handle_editor_key(&db, Key::ArrowUp)
        .expect("move out of table row");
    assert_eq!(app.editor.cursor_line, 0);
    assert_eq!(app.editor.cursor_col, 1);

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn vertical_arrow_movement_inside_table_uses_shared_cell_planner() {
    let (db, mut app, path) =
        app_with_note("before\n| name | qty |\n| ---- | --- |\n| pen  | 2   |\nafter");
    app.editor.cursor_line = 1;
    app.editor.cursor_col = 12; // end of "qty"

    app.handle_editor_key(&db, Key::ArrowDown)
        .expect("down skips delimiter and preserves table cell index");
    assert_eq!(app.editor.cursor_line, 3);
    assert_eq!(app.editor.cursor_col, 10);

    app.handle_editor_key(&db, Key::ArrowUp)
        .expect("up skips delimiter and returns to header cell");
    assert_eq!(app.editor.cursor_line, 1);
    assert_eq!(app.editor.cursor_col, 12);

    app.handle_editor_key(&db, Key::ArrowUp)
        .expect("up exits before table block");
    assert_eq!(app.editor.cursor_line, 0);
    assert_eq!(app.editor.cursor_col, "before".chars().count());

    app.editor.cursor_line = 3;
    app.editor.cursor_col = 10;
    app.handle_editor_key(&db, Key::ArrowDown)
        .expect("down exits after table block");
    assert_eq!(app.editor.cursor_line, 4);
    assert_eq!(app.editor.cursor_col, "after".chars().count());

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn normal_jk_movement_through_table_preserves_text_column() {
    let (db, mut app, path) = app_with_note("plain\n| aaa | bb  |\nafter");
    app.mode = UiMode::Normal;
    app.editor.cursor_line = 0;
    app.editor.cursor_col = 1;

    run_keys(&mut app, &db, &[Key::Char('j')]);
    assert_eq!(app.editor.cursor_line, 1);
    assert_eq!(app.editor.cursor_col, 1);

    run_keys(&mut app, &db, &[Key::Char('j')]);
    assert_eq!(app.editor.cursor_line, 2);
    assert_eq!(app.editor.cursor_col, 1);

    run_keys(&mut app, &db, &[Key::Char('k')]);
    assert_eq!(app.editor.cursor_line, 1);
    assert_eq!(app.editor.cursor_col, 1);

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn empty_table_row_draws_cursor_inside_editable_cell_slot() {
    let body = "| sasa | sasasa | sasasa |\n| ---- | ------ | ------ |\n|      |        |        |";
    let (db, mut app, path) = app_with_note(body);
    app.editor.cursor_line = 2;
    app.editor.cursor_col = 2;

    let (_, cursor) = render_screen(&mut app);

    let expected_screen_col = app.gutter_width() + 3;
    assert_eq!(
        usize::from(cursor.col) + 1,
        expected_screen_col,
        "cursor should render after the opening pipe and padding, not on the pipe"
    );

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn typing_space_in_table_cell_allows_followup_word_input() {
    let (db, mut app, path) = app_with_note("| aaa |");
    app.editor.cursor_col = 5; // end of content
    app.handle_editor_key(&db, Key::Char(' '))
        .expect("insert space");
    app.handle_editor_key(&db, Key::Char(' '))
        .expect("insert second space");
    app.handle_editor_key(&db, Key::Char('b'))
        .expect("insert next word char");
    assert_eq!(app.editor.lines[0], "| aaa  b |");

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn typing_space_in_longest_cell_middle_preserves_text_and_keeps_table_aligned() {
    let (db, mut app, path) =
        app_with_note("| h | v |\n| --- | --- |\n| abcd efgh | ok |\n| aa | bb |");
    app.editor.cursor_line = 2;
    app.editor.cursor_col = app.editor.lines[2].find("efgh").expect("efgh");

    app.handle_editor_key(&db, Key::Char(' '))
        .expect("space should insert in longest cell");

    app.handle_editor_key(&db, Key::Char('z'))
        .expect("follow-up typing should keep inserted space");

    let cells = crate::editor_core::table::split_table_cells(&app.editor.lines[2]);
    assert_eq!(cells[0], "abcd  zefgh");
    let expected_pipes = crate::editor_core::table::table_pipe_positions(&app.editor.lines[0]);
    for line in app
        .editor
        .lines
        .iter()
        .filter(|line| crate::editor_core::table::is_table_line(line))
    {
        assert_eq!(
            crate::editor_core::table::table_pipe_positions(line),
            expected_pipes
        );
    }

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn typing_spaces_at_right_edge_of_widest_cell_preserves_text_and_padding() {
    let (db, mut app, path) =
        app_with_note("| h | v |\n| --- | --- |\n| alpha beta gamma | ok |\n| aa | bb |");
    app.editor.cursor_line = 2;
    app.editor.cursor_col = app.editor.lines[2]
        .find("alpha beta gamma")
        .expect("widest cell")
        + "alpha beta gamma".chars().count();

    app.handle_editor_key(&db, Key::Char(' '))
        .expect("first space");
    app.handle_editor_key(&db, Key::Char(' '))
        .expect("second space");
    app.handle_editor_key(&db, Key::Char(' '))
        .expect("third space");
    app.handle_editor_key(&db, Key::Char('x'))
        .expect("follow-up char");

    let row_cells = crate::editor_core::table::split_table_cells(&app.editor.lines[2]);
    assert_eq!(row_cells[0], "alpha beta gamma   x");

    let expected_pipes = crate::editor_core::table::table_pipe_positions(&app.editor.lines[0]);
    for line in app
        .editor
        .lines
        .iter()
        .filter(|line| crate::editor_core::table::is_table_line(line))
    {
        assert_eq!(
            crate::editor_core::table::table_pipe_positions(line),
            expected_pipes
        );
    }

    let pipes = crate::editor_core::table::table_pipe_positions(&app.editor.lines[2]);
    assert!(pipes.len() >= 2);
    let right_pipe = pipes[1];
    assert_eq!(
        app.editor.lines[2]
            .chars()
            .nth(right_pipe.saturating_sub(1)),
        Some(' ')
    );
    assert_eq!(
        app.editor.lines[2]
            .chars()
            .nth(right_pipe.saturating_sub(2)),
        Some('x')
    );

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn typing_in_table_cell_reflows_column_when_cell_becomes_widest() {
    let (db, mut app, path) = app_with_note("| a | b |\n| --- | --- |\n| 1 | 2 |");
    app.editor.cursor_line = 2;
    app.editor.cursor_col = 3; // end of first cell content in row 3

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

    assert_eq!(app.editor.lines[0], "| a     | b   |");
    assert_eq!(app.editor.lines[1], "| ----- | --- |");
    assert_eq!(app.editor.lines[2], "| 12345 | 2   |");

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn shift_enter_in_table_cell_splits_into_next_row() {
    let (db, mut app, path) = app_with_note("| left | value |\n| --- | --- |\n| ok | data |");
    app.editor.cursor_line = 2;
    app.editor.cursor_col = app.editor.lines[2].find("data").expect("data") + 2;
    app.handle_editor_key(&db, Key::ShiftEnter)
        .expect("shift-enter splits row");
    assert_eq!(app.editor.lines.len(), 4);
    assert!(app.editor.lines.iter().all(|line| !line.contains("<br>")));
    assert!(app.editor.lines[2].contains("| ok"));
    assert!(app.editor.lines[2].contains("| da"));
    assert!(app.editor.lines[3].starts_with("|>"));
    assert!(app.editor.lines[3].contains("| ta"));

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn multiline_paste_inside_table_cell_creates_continuation_rows_in_same_cell() {
    let (db, mut app, path) = app_with_note("| h1 | h2 |\n| --- | --- |\n| left | right |");
    app.editor.cursor_line = 2;
    app.editor.cursor_col = app.editor.lines[2].find("right").expect("right") + 2; // ri|ght

    app.insert_paste("A\nB\nC");

    assert_eq!(app.editor.lines.len(), 5);
    assert_eq!(
        crate::editor_core::table::split_table_cells(&app.editor.lines[2])[1],
        "riA"
    );
    assert!(crate::editor_core::table::is_table_continuation_line(
        &app.editor.lines[3]
    ));
    assert_eq!(
        crate::editor_core::table::split_table_cells(&app.editor.lines[3])[1],
        "B"
    );
    assert!(crate::editor_core::table::is_table_continuation_line(
        &app.editor.lines[4]
    ));
    assert_eq!(
        crate::editor_core::table::split_table_cells(&app.editor.lines[4])[1],
        "Cght"
    );

    app.insert_char('X');
    assert_eq!(
        crate::editor_core::table::split_table_cells(&app.editor.lines[4])[1],
        "CXght"
    );

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn deleting_last_content_in_continuation_row_removes_that_row() {
    let (db, mut app, path) = app_with_note("| h |\n| --- |\n| base |\n|> tail |");
    app.editor.cursor_line = 3;
    app.editor.cursor_col =
        app.editor.lines[3].find("tail").expect("tail") + "tail".chars().count();

    run_keys(
        &mut app,
        &db,
        &[
            Key::Backspace,
            Key::Backspace,
            Key::Backspace,
            Key::Backspace,
        ],
    );

    assert_eq!(app.editor.lines, vec!["| h    |", "| ---- |", "| base |"]);
    assert!(app.editor.lines.iter().all(|line| !line.starts_with("|>")));
    assert_eq!(app.editor.cursor_line, 2);

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
    app.editor.cursor_line = 0;
    app.editor.cursor_col = 0;
    // Seed: a recompute now should leave the trailer alone (already in sync).
    app.run_calc_recompute();
    assert_eq!(app.editor.lines[1], "2 * rate = 20");

    // Change the variable definition.
    app.editor.lines[0] = "rate := 15".to_string();
    app.run_calc_recompute();

    // Trailer should have been refreshed from `= 20` to `= 30`.
    assert_eq!(app.editor.lines[1], "2 * rate = 30");
    assert_eq!(app.editor.lines[2], "other line");

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
    app.editor.cursor_line = 0;
    app.editor.cursor_col = 0;
    app.run_calc_recompute();
    assert_eq!(app.editor.lines[1], "2 * rate = FOO");

    app.editor.lines[0] = "rate := 15".to_string();
    app.run_calc_recompute();
    assert_eq!(app.editor.lines[1], "2 * rate = FOO");

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn recompute_calc_skips_refresh_when_cursor_in_trailer() {
    let (db, mut app, path) = app_with_note("rate := 10\n2 * rate = 20");
    app.editor.cursor_line = 0;
    app.editor.cursor_col = 0;
    app.run_calc_recompute();

    // Park the cursor inside the trailer on line 1.
    app.editor.cursor_line = 1;
    app.editor.cursor_col = app.editor.lines[1].chars().count(); // end of line, inside trailer
    app.editor.lines[0] = "rate := 15".to_string();
    app.run_calc_recompute();

    // Untouched because cursor is in the trailer region.
    assert_eq!(app.editor.lines[1], "2 * rate = 20");

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
    app.editor.cursor_line = 1;

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
    app.editor.cursor_line = 1;
    app.editor.cursor_col = line_char_len(app.current_line());

    app.handle_editor_key(&db, Key::Tab).expect("tab applies");

    assert_eq!(app.editor.lines[1], "- [ ] 15");
    assert_eq!(app.editor.cursor_col, app.editor.lines[1].chars().count());

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn tab_applies_table_error_result_and_reflows_table_alignment() {
    let (db, mut app, path) =
        app_with_note("| c1 | c2 |\n| --- | --- |\n| a | !ERROR#out_of_bounds |\n| b | 1 |");
    app.editor.cursor_line = 3;
    app.editor.cursor_col = app.editor.lines[3].find('1').expect("value");

    app.handle_editor_key(&db, Key::Char('2'))
        .expect("typing triggers table reflow");

    let expected_pipes = crate::editor_core::table::table_pipe_positions(&app.editor.lines[0]);
    for line in app.editor.lines.iter().take(4) {
        let pipes = crate::editor_core::table::table_pipe_positions(line);
        assert_eq!(pipes, expected_pipes, "misaligned line: {line}");
    }
    assert!(app.editor.lines[2].contains("!ERROR#out_of_bounds"));

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn variable_autocomplete_popup_appears_after_min_chars_and_supports_selection_keys() {
    let (db, mut app, path) = app_with_note("total cost := 10\ntotal revenue := 20\nto");
    app.editor.cursor_line = 2;
    app.editor.cursor_col = line_char_len(app.current_line());
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
    assert_eq!(app.editor.cursor_line, 2);
    assert_eq!(app.editor.cursor_col, 3);

    app.handle_editor_key(&db, Key::ArrowUp)
        .expect("up picks previous");
    assert_eq!(app.variable_autocomplete_popup.selected_index, 0);

    app.handle_editor_key(&db, Key::Enter)
        .expect("enter accepts selected suggestion");
    assert_eq!(app.editor.lines[2], "total cost");
    assert_eq!(app.editor.cursor_col, "total cost".chars().count());
    assert!(!app.variable_autocomplete_popup.visible);

    app.editor.lines[2] = "tot".to_string();
    app.editor.cursor_col = 3;
    app.refresh_variable_autocomplete_popup();
    app.handle_editor_key(&db, Key::ArrowDown)
        .expect("down selects second suggestion");
    app.handle_editor_key(&db, Key::Tab)
        .expect("tab accepts selected suggestion");
    assert_eq!(app.editor.lines[2], "total revenue");

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn autocomplete_popup_esc_dismisses_and_tab_fallback_still_accepts_variable() {
    let (db, mut app, path) = app_with_note("total cost := 10\ntotal revenue := 20\ntot");
    app.editor.cursor_line = 2;
    app.editor.cursor_col = line_char_len(app.current_line());
    app.refresh_variable_autocomplete_popup();
    assert!(app.variable_autocomplete_popup.visible);

    app.handle_editor_key(&db, Key::Esc)
        .expect("esc closes autocomplete popup");
    assert_eq!(app.mode, UiMode::Editor);
    assert!(!app.variable_autocomplete_popup.visible);

    app.handle_editor_key(&db, Key::Tab)
        .expect("tab fallback still applies variable autocomplete");
    assert_eq!(app.editor.lines[2], "total cost");
    assert_eq!(app.status, "autocomplete: total cost");

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn autocomplete_popup_closes_on_cursor_movement() {
    let (db, mut app, path) = app_with_note("total cost := 10\ntot");
    app.editor.cursor_line = 1;
    app.editor.cursor_col = line_char_len(app.current_line());
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
    app.editor.cursor_line = 1;
    app.editor.cursor_col = line_char_len(app.current_line());
    app.refresh_variable_autocomplete_popup();
    assert!(!app.variable_autocomplete_popup.visible);
    assert!(!app.calc_variables_enabled());

    app.handle_editor_key(&db, Key::Tab)
        .expect("tab falls back when variable module is off");
    assert_eq!(app.editor.lines[1], "tot  ");

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
    app.editor.cursor_line = 1;
    app.editor.cursor_col = line_char_len(app.current_line());
    app.refresh_variable_autocomplete_popup();
    assert!(!app.variable_autocomplete_popup.visible);

    db.save_note("n2", "total cost := 10\ntot")
        .expect("save n2");
    db.set_note_modules("n2", note_modules_with_variables(true))
        .expect("enable n2 variable module");
    let note2 = db.get_note("n2").expect("n2 lookup").expect("n2 exists");
    app.set_active_note(&db, note2).expect("activate n2");
    app.mode = UiMode::Editor;
    app.editor.cursor_line = 1;
    app.editor.cursor_col = line_char_len(app.current_line());
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
    app.editor.cursor_line = 1;
    app.editor.cursor_col = line_char_len(app.current_line());
    assert!(!app.variable_autocomplete_popup.visible);

    let hint = app
        .variable_autocomplete_status_hint()
        .expect("autocomplete hint");
    assert!(hint.contains("total cost"));

    app.handle_editor_key(&db, Key::Tab)
        .expect("tab accepts autocomplete");

    assert_eq!(app.editor.lines[1], "total cost");
    assert_eq!(app.editor.cursor_col, "total cost".chars().count());
    assert_eq!(app.status, "autocomplete: total cost");

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

fn type_text(app: &mut TerminalApp, db: &Db, text: &str) {
    for ch in text.chars() {
        app.handle_editor_key(db, Key::Char(ch)).expect("type");
    }
}

#[test]
fn typing_a_table_row_by_hand_keeps_its_cells() {
    let (db, mut app, path) = app_with_note("");
    app.mode = UiMode::Editor;
    type_text(&mut app, &db, "| a | b |");
    assert_eq!(app.editor.lines, vec!["| a | b |"]);
    assert_eq!(
        app.editor.cursor_col, 9,
        "cursor stays after the closing pipe"
    );

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn typing_a_whole_markdown_table_by_hand_builds_a_clean_table() {
    let (db, mut app, path) = app_with_note("");
    app.mode = UiMode::Editor;
    type_text(&mut app, &db, "| a | b |");
    app.handle_editor_key(&db, Key::Enter).expect("enter");
    // Enter already added the delimiter row; typing it again is absorbed.
    type_text(&mut app, &db, "| --- | --- |");
    app.handle_editor_key(&db, Key::Enter).expect("enter");
    type_text(&mut app, &db, "| 1 | 2 |");

    assert_eq!(
        app.editor.lines,
        vec!["| a   | b   |", "| --- | --- |", "| 1   | 2   |"]
    );

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn visual_line_selection_shows_number_stats_in_status_bar() {
    let (db, mut app, path) = app_with_note("1. rent 1200\n2. food 300\nlater 7");
    app.mode = UiMode::VisualLine;
    app.editor.selection_anchor = Some((0, 0));
    app.editor.cursor_line = 1;

    let rows = screen_rows(&mut app);
    let status = rows.last().expect("status row");
    assert!(status.contains("Σ 1500 · avg 750 · n 2"), "status: {status:?}");

    app.mode = UiMode::Normal;
    app.editor.selection_anchor = None;
    let rows = screen_rows(&mut app);
    assert!(!rows.last().expect("status row").contains('Σ'));

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn charwise_selection_counts_only_selected_numbers() {
    let (db, mut app, path) = app_with_note("a 10 b 20 c 30");
    app.mode = UiMode::Visual;
    // Select "10 b 20" (inclusive of the char under the cursor).
    app.editor.selection_anchor = Some((0, 2));
    app.editor.cursor_col = 8;

    let stats = app.selection_number_stats().expect("stats");
    assert_eq!((stats.count, stats.sum), (2, 30.0));

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn label_line_shows_leading_result_and_tab_appends_it() {
    let (db, mut app, path) = app_with_note("100 - 20 groceries");
    app.mode = UiMode::Editor;
    app.run_calc_recompute();
    assert_eq!(app.calc.results[0].as_deref(), Some("80"));

    app.editor.cursor_col = line_char_len(&app.editor.lines[0]);
    app.handle_editor_key(&db, Key::Tab).expect("tab");
    assert_eq!(app.editor.lines[0], "100 - 20 groceries = 80");
    app.run_calc_recompute();
    assert_eq!(app.calc.results[0], None, "applied result is not repeated");

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn plain_expressions_evaluate_when_opening_a_note_without_assignments() {
    let (_db, app, path) = app_with_note("2 + 2\n100 - 20 groceries\nplain text");
    assert_eq!(app.calc.results[0].as_deref(), Some("4"));
    assert_eq!(app.calc.results[1].as_deref(), Some("80"));
    assert_eq!(app.calc.results[2], None);

    drop(app);
    cleanup_db_files(&path);
}

#[test]
fn typing_a_plain_expression_evaluates_it_on_idle() {
    let (db, mut app, path) = app_with_note("");
    app.mode = UiMode::Editor;
    for ch in "12 * 3".chars() {
        app.handle_editor_key(&db, Key::Char(ch)).expect("type");
    }
    let deadline = Instant::now() + Duration::from_secs(2);
    while app.calc.results.first().cloned().flatten().is_none() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
        app.maybe_recompute_calc_after_idle();
    }
    assert_eq!(app.calc.results[0].as_deref(), Some("36"));

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn closing_a_hand_typed_cell_moves_cursor_into_the_next_cell() {
    for typed in ["| sasa |", "|sasa|"] {
        let (db, mut app, path) = app_with_note("");
        app.mode = UiMode::Editor;
        type_text(&mut app, &db, typed);
        let next_cell = app.gutter_width() + "| sasa | ".len();
        let (_, cursor) = render_screen(&mut app);
        assert_eq!(
            usize::from(cursor.col),
            next_cell,
            "{typed:?}: caret sits at the start of the next cell"
        );
        // A space fills the drawn pad instead of moving the caret.
        type_text(&mut app, &db, " ");
        let (_, cursor) = render_screen(&mut app);
        assert_eq!(usize::from(cursor.col), next_cell, "{typed:?}");

        drop(app);
        drop(db);
        cleanup_db_files(&path);
    }

    // Typing straight after the pipe gets the pad inserted.
    let (db, mut app, path) = app_with_note("");
    app.mode = UiMode::Editor;
    type_text(&mut app, &db, "|a|b|");
    assert_eq!(app.editor.lines, vec!["| a | b |"]);

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn typed_trailing_spaces_in_table_cell_render_before_next_word() {
    let (db, mut app, path) = app_with_note("| aa  | bb  |\n| --- | --- |\n| x   | y   |");
    app.editor.cursor_line = 2;
    app.editor.cursor_col = 3;
    let base = app.gutter_width() + "| x".len();
    for typed in 1..=2 {
        app.handle_editor_key(&db, Key::Char(' ')).expect("space");
        let (_, cursor) = render_screen(&mut app);
        assert_eq!(usize::from(cursor.col), base + typed, "caret advances per space");
    }

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn typed_trailing_spaces_widen_the_column_for_every_row() {
    let (db, mut app, path) = app_with_note(
        "| sas   | sasa    | x   |\n| ----- | ------- | --- |\n| dusan | as dasd | y   |",
    );
    app.editor.cursor_line = 2;
    app.editor.cursor_col = app.editor.lines[2].find("dasd").unwrap() + 4;
    for _ in 0..2 {
        app.handle_editor_key(&db, Key::Char(' ')).expect("space");
        let rows = screen_rows(&mut app);
        let pipes = |line_no: usize| -> Vec<usize> {
            first_editor_row_for(&rows, line_no)
                .char_indices()
                .filter(|(_, c)| *c == '|')
                .map(|(i, _)| i)
                .collect()
        };
        assert_eq!(pipes(1), pipes(3), "header and cursor row stay aligned");
        assert_eq!(pipes(2), pipes(3), "delimiter and cursor row stay aligned");
    }

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn enter_in_a_middle_table_cell_opens_a_row_below_without_splitting() {
    let (db, mut app, path) = app_with_note(
        "| sasa | sasa | sasas |\n| ---- | ---- | ----- |\n| sas  |      |       |",
    );
    app.editor.cursor_line = 2;
    app.editor.cursor_col = 5;
    app.handle_editor_key(&db, Key::Enter).expect("enter");
    assert_eq!(
        app.editor.lines,
        vec![
            "| sasa | sasa | sasas |",
            "| ---- | ---- | ----- |",
            "| sas  |      |       |",
            "|      |      |       |",
        ]
    );
    assert_eq!((app.editor.cursor_line, app.editor.cursor_col), (3, 2));

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn typing_a_compact_markdown_table_by_hand_builds_a_clean_table() {
    let (db, mut app, path) = app_with_note("");
    app.mode = UiMode::Editor;
    type_text(&mut app, &db, "|a|b|");
    app.handle_editor_key(&db, Key::Enter).expect("enter");
    type_text(&mut app, &db, "|-|-|");
    app.handle_editor_key(&db, Key::Enter).expect("enter");
    type_text(&mut app, &db, "|1|2|");

    assert_eq!(
        app.editor.lines,
        vec!["| a   | b   |", "| --- | --- |", "| 1   | 2   |"]
    );

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}
