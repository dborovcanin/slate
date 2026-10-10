use super::*;

fn lines(text: &[&str]) -> Vec<String> {
    text.iter().map(|s| s.to_string()).collect()
}

fn cursor(line: usize, column: usize) -> BufferCursor {
    BufferCursor { line, column }
}

#[test]
fn unicode_underscores_punctuation_and_whitespace_keep_distinct_motion_classes() {
    let doc = lines(&["αβ_γ !!  δ"]);
    for (from, to) in [(0, 5), (4, 5), (5, 9), (7, 9), (9, 10)] {
        assert_eq!(
            move_cursor_right_word(&doc, cursor(0, from), false, std::iter::empty()),
            cursor(0, to)
        );
    }
    for (from, to) in [(10, 9), (9, 5), (7, 5), (5, 0), (4, 0)] {
        assert_eq!(
            move_cursor_left_word(&doc, cursor(0, from), false, std::iter::empty()),
            cursor(0, to)
        );
    }
}

#[test]
fn visible_neighbors_control_line_crossings_and_are_consumed_only_when_needed() {
    let doc = lines(&["one", "hidden", "界"]);
    assert_eq!(
        move_cursor_right_word(&doc, cursor(0, 3), false, [2].into_iter()),
        cursor(2, 0)
    );
    assert_eq!(
        move_cursor_left_word(&doc, cursor(2, 0), false, [0].into_iter()),
        cursor(0, 3)
    );
    assert_eq!(
        move_cursor_left_word(&doc, cursor(0, 0), false, std::iter::empty()),
        cursor(0, 0)
    );
    assert_eq!(
        move_cursor_right_word(&doc, cursor(2, 9), false, std::iter::empty()),
        cursor(2, 9)
    );
    assert_eq!(
        move_cursor_right_word(
            &doc,
            cursor(0, 0),
            false,
            std::iter::from_fn(|| panic!("same-line motion must not request a neighbor"))
        ),
        cursor(0, 3)
    );
}

#[test]
fn empty_lines_and_out_of_range_character_columns_keep_existing_clamping() {
    let doc = lines(&["", "β"]);
    assert_eq!(
        move_cursor_left_word(&doc, cursor(0, 4), false, std::iter::empty()),
        cursor(0, 0)
    );
    assert_eq!(
        move_cursor_right_word(&doc, cursor(0, 4), false, [1].into_iter()),
        cursor(1, 0)
    );
    assert_eq!(
        move_cursor_left_word(&doc, cursor(1, 8), false, std::iter::empty()),
        cursor(1, 0)
    );
}

#[test]
fn table_words_skip_delimiters_land_on_empty_rows_and_exit_into_prose() {
    let doc = lines(&[
        "before",
        "| αβ | b_c |",
        "| -- | --- |",
        "| δ  | !!  |",
        "|    |     |",
        "after",
    ]);
    assert_eq!(
        move_cursor_right_word(&doc, cursor(1, 2), true, std::iter::empty()),
        cursor(1, 7)
    );
    assert_eq!(
        move_cursor_right_word(&doc, cursor(1, 12), true, [2, 3].into_iter()),
        cursor(3, 2)
    );
    assert_eq!(
        move_cursor_left_word(&doc, cursor(3, 2), true, [2, 1].into_iter()),
        cursor(1, 7)
    );
    assert_eq!(
        move_cursor_right_word(&doc, cursor(3, 12), true, [4, 5].into_iter()),
        cursor(4, 0)
    );
    assert_eq!(
        move_cursor_right_word(&doc, cursor(4, 0), true, [5].into_iter()),
        cursor(5, 0)
    );
    assert_eq!(
        move_cursor_left_word(&doc, cursor(1, 2), true, [0].into_iter()),
        cursor(0, 6)
    );
    // The host can omit a folded table row without the core visiting it.
    assert_eq!(
        move_cursor_right_word(&doc, cursor(1, 12), true, [5].into_iter()),
        cursor(5, 0)
    );
}

#[test]
fn escaped_pipes_remain_punctuation_and_disabled_tables_use_prose_rules() {
    let doc = lines(&["| a\\|b | c |"]);
    assert_eq!(
        move_cursor_right_word(&doc, cursor(0, 2), true, std::iter::empty()),
        cursor(0, 3)
    );
    assert_eq!(
        move_cursor_right_word(&doc, cursor(0, 3), true, std::iter::empty()),
        cursor(0, 5)
    );
    let doc = lines(&["| a | b |"]);
    assert_eq!(
        move_cursor_right_word(&doc, cursor(0, 2), false, std::iter::empty()),
        cursor(0, 4)
    );
    assert_eq!(
        move_cursor_right_word(&doc, cursor(0, 2), true, std::iter::empty()),
        cursor(0, 6)
    );
}

#[test]
fn backward_deletion_keeps_its_distinct_underscore_and_unicode_rules() {
    for (text, col, expected, expected_col) in [
        ("αβ_γ !!", 7, "αβ_", 3),
        ("abc_def", 7, "abc_", 4),
        ("one  !", 6, "", 0),
        ("!!  ", 4, "", 0),
        ("α", 8, "α", 8),
    ] {
        let doc = lines(&[text]);
        let Some(BackwardWordDelete::WithinLine(range)) =
            prepare_backward_word_delete(&doc, cursor(0, col), false)
        else {
            panic!("same-line deletion");
        };
        assert_eq!(range.edit.from, (0, byte_index(text, expected_col)));
        assert_eq!(range.edit.to, (0, byte_index(text, col)));
        assert_eq!(range.edit.from_line_len, text.len());
        assert_eq!(range.edit.to_line_len, text.len());
        assert_eq!(range.edit.inserted_breaks, 0);
        assert_eq!(
            range.delta,
            EditDelta {
                start_line: 0,
                old_span: 1,
                new_span: 1
            }
        );
        let mut line = doc[0].clone();
        assert_eq!(apply_word_delete(&mut line, range), expected_col);
        assert_eq!(line, expected);
    }
}

#[test]
fn backward_deletion_joins_physical_lines_and_protects_table_cell_edges() {
    let doc = lines(&["previous", "current"]);
    assert!(matches!(
        prepare_backward_word_delete(&doc, cursor(1, 0), false),
        Some(BackwardWordDelete::JoinPreviousLine)
    ));
    assert!(prepare_backward_word_delete(&doc, cursor(0, 0), false).is_none());
    let doc = lines(&["| abc | def |"]);
    assert!(prepare_backward_word_delete(&doc, cursor(0, 2), true).is_none());
    let Some(BackwardWordDelete::WithinLine(range)) =
        prepare_backward_word_delete(&doc, cursor(0, 5), true)
    else {
        panic!("table-cell deletion");
    };
    let mut line = doc[0].clone();
    assert_eq!(apply_word_delete(&mut line, range), 2);
    assert_eq!(line, "|  | def |");
    let Some(BackwardWordDelete::WithinLine(range)) =
        prepare_backward_word_delete(&doc, cursor(0, 2), false)
    else {
        panic!("plain deletion with tables disabled");
    };
    let mut line = doc[0].clone();
    assert_eq!(apply_word_delete(&mut line, range), 0);
    assert_eq!(line, "abc | def |");
    let doc = lines(&["| αβ γ | end |"]);
    let Some(BackwardWordDelete::WithinLine(range)) =
        prepare_backward_word_delete(&doc, cursor(0, 6), true)
    else {
        panic!("Unicode table-cell deletion");
    };
    let mut line = doc[0].clone();
    assert_eq!(apply_word_delete(&mut line, range), 5);
    assert_eq!(line, "| αβ  | end |");
}

#[test]
fn backward_deletion_in_later_cells_uses_character_bounds_after_unicode() {
    let doc = lines(&["| é🙂 | alpha beta | tail |"]);
    let start = doc[0].chars().position(|c| c == 'a').unwrap();
    let Some(BackwardWordDelete::WithinLine(range)) =
        prepare_backward_word_delete(&doc, cursor(0, start + 5), true)
    else {
        panic!("word delete");
    };
    let mut line = doc[0].clone();
    let result = apply_word_delete(&mut line, range);
    assert_eq!(line, "| é🙂 |  beta | tail |");
    assert_eq!(result, start);
}
