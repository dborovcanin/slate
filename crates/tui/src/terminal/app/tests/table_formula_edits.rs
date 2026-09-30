//! Formula tables across structural edits, undo and redo: what the app shows
//! must match a freshly opened note with the same text. The large stress run
//! is ignored by default because its timings need a release build:
//!
//! ```sh
//! cargo test --release -p slate --lib table_formula_edits -- --ignored --nocapture
//! ```

use super::*;
use crate::terminal::input::{set_terminal_size, Key};

const DATA_ROWS: usize = 250;

fn stress_note() -> String {
    let mut lines = vec![
        "# Stress".to_string(),
        String::new(),
        "rate := 1.5".to_string(),
        "bonus := 10".to_string(),
        String::new(),
        "| id | a | b | c | d | e | ab | c_rate | row_sum | mix |".to_string(),
        "| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |".to_string(),
    ];
    for r in 1..=DATA_ROWS {
        let (a, b, c, d, e) = (r, r * 2 % 97, r * 3 % 89, r % 13, r * 7 % 31);
        lines.push(format!(
            "| r{r} | {a} | {b} | {c} | {d} | {e} | :=({r},2)+({r},3) | :=({r},4)*rate \
             | :=sum_row() | :=({r},7)+({r},8)*bonus-sum_col()/100 |"
        ));
    }
    lines.push(
        "| total | :=sum_col() | :=sum_col() | :=sum_col() | :=sum_col() | :=sum_col() \
         | :=sum_col() | :=sum_col()*rate | :=sum_col() | :=sum_col()+bonus |"
            .to_string(),
    );
    lines.push(String::new());
    lines.push("after the table".to_string());
    lines.join("\n")
}

/// Keys for `text`; `<esc>`, `<tab>` and `<cr>` name those keys.
fn keys(text: &str) -> Vec<Key> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(ch) = rest.chars().next() {
        let named = [
            ("<esc>", Key::Esc),
            ("<tab>", Key::Tab),
            ("<cr>", Key::Enter),
        ]
        .into_iter()
        .find(|(name, _)| rest.starts_with(name));
        if let Some((name, key)) = named {
            out.push(key);
            rest = &rest[name.len()..];
        } else {
            out.push(Key::Char(ch));
            rest = &rest[ch.len_utf8()..];
        }
    }
    out
}

/// Compares what `app` shows (calc values and every rendered line) with a
/// freshly opened note holding the same text and cursor. Returns mismatches.
fn diff_against_fresh(app: &mut TerminalApp) -> Vec<String> {
    let (_db, mut fresh, path) = app_with_note(&app.editor.lines.join("\n"));
    fresh.mode = app.mode;
    fresh.run_calc_recompute();
    let mut problems = Vec::new();
    if fresh.editor.lines != app.editor.lines {
        problems.push("fresh note text differs (normalized on open)".to_string());
    }
    fresh.editor.cursor_line = app.editor.cursor_line;
    fresh.editor.cursor_col = app.editor.cursor_col;
    let mut mismatched = 0usize;
    for idx in 0..app.editor.lines.len() {
        let ours = app.calc.cell_results.get(idx).cloned().unwrap_or_default();
        let theirs = fresh
            .calc
            .cell_results
            .get(idx)
            .cloned()
            .unwrap_or_default();
        let a = app.prepare_display_line(idx, 0).text;
        let b = fresh.prepare_display_line(idx, 0).text;
        if ours == theirs && a == b {
            continue;
        }
        mismatched += 1;
        if mismatched <= 2 {
            problems.push(format!(
                "line {idx}:\n  ours  {a}\n  fresh {b}\n  calc ours={:?} fresh={:?}",
                ours.iter().map(|c| &c.value).collect::<Vec<_>>(),
                theirs.iter().map(|c| &c.value).collect::<Vec<_>>()
            ));
        }
    }
    if mismatched > 2 {
        problems.push(format!("... {mismatched} mismatched lines in total"));
    }
    drop(fresh);
    cleanup_db_files(&path);
    problems
}

/// Presses `seq` and paints one frame; returns milliseconds taken.
fn press(app: &mut TerminalApp, db: &Db, seq: &str) -> f64 {
    let started = Instant::now();
    for key in keys(seq) {
        app.handle_key(db, key).expect("key");
    }
    render_screen(app);
    started.elapsed().as_secs_f64() * 1e3
}

/// A real pause: past the calc and autosave debounces, then the idle ticks
/// the event loop would run. Returns milliseconds spent in the ticks.
fn idle(app: &mut TerminalApp, db: &Db) -> f64 {
    std::thread::sleep(Duration::from_millis(400));
    let started = Instant::now();
    for _ in 0..4 {
        app.maybe_autosave(db).expect("idle tick");
    }
    let spent = started.elapsed().as_secs_f64() * 1e3;
    render_screen(app);
    spent
}

fn check_text(problems: &mut Vec<String>, tag: String, ours: &[String], expected: &[String]) {
    if ours != expected {
        problems.push(format!("{tag} text wrong: {}", first_diff(ours, expected)));
    }
}

fn check_fresh(problems: &mut Vec<String>, tag: &str, app: &mut TerminalApp) {
    for p in diff_against_fresh(app) {
        problems.push(format!("{tag} {p}"));
    }
}

#[test]
#[ignore]
fn table_formula_edits_stress() {
    set_terminal_size(50, 200);
    let (db, mut app, path) = app_with_note(&formatted(&stress_note()));
    app.mode = UiMode::Normal;
    app.run_calc_recompute();
    render_screen(&mut app);
    let mut problems = Vec::new();
    check_fresh(&mut problems, "[initial]", &mut app);

    // `G` lines are 1-based; data row r is line r + 7.
    let edits: &[(&str, &str)] = &[
        ("dd mid row", "40Gdd"),
        ("p below", "5jp"),
        ("dd first data row", "8Gdd"),
        ("P above", "20GP"),
        ("3dd", "100G3dd"),
        ("p 3 rows", "10jp"),
        ("yy p", "60Gyyp"),
        ("ciw value", "45G0f|;wciw999<esc>"),
        ("type in cell", "50G0f|;la12<esc>"),
        ("edit formula", "19G0f|;;;;;wf)a*2<esc>"),
        (
            "new row with o",
            "30Go| new | 1 | 2 | 3 | 4 | 5 | :=(1,2)*bonus | | | |<esc>",
        ),
        (
            "cc row",
            "70Gcc| x | 9 | 9 | 9 | 9 | 9 | :=sum_row() | | | |<esc>",
        ),
        ("tab typing", "80G0a<tab><tab>7<esc>"),
        ("Vjjd", "90GVjjd"),
        ("x in cell", "30G0wx"),
        ("dd total row", "Gkkdd"),
        ("p total back", "kp"),
        ("dd header", "6Gdd"),
        ("P header back", "P"),
    ];

    let mut times: Vec<(String, f64, f64, f64, f64)> = Vec::new();
    let mut history = vec![app.editor.lines.clone()];
    for (label, seq) in edits {
        let before = app.editor.lines.clone();
        let edit_ms = press(&mut app, &db, seq);
        let after = app.editor.lines.clone();
        if after == before {
            problems.push(format!("[{label}] edit changed nothing"));
        }
        let idle_ms = idle(&mut app, &db);
        check_fresh(&mut problems, &format!("[after {label}]"), &mut app);

        let undo_ms = press(&mut app, &db, "u");
        check_text(
            &mut problems,
            format!("[undo {label}]"),
            &app.editor.lines,
            &before,
        );
        idle(&mut app, &db);
        check_fresh(&mut problems, &format!("[undo {label}]"), &mut app);

        let started = Instant::now();
        app.handle_key(&db, Key::Ctrl('r')).expect("redo");
        render_screen(&mut app);
        let redo_ms = started.elapsed().as_secs_f64() * 1e3;
        check_text(
            &mut problems,
            format!("[redo {label}]"),
            &app.editor.lines,
            &after,
        );
        idle(&mut app, &db);
        check_fresh(&mut problems, &format!("[redo {label}]"), &mut app);

        history.push(app.editor.lines.clone());
        times.push((label.to_string(), edit_ms, idle_ms, undo_ms, redo_ms));
    }

    // Walk the whole history back and forward again.
    for step in (0..edits.len()).rev() {
        press(&mut app, &db, "u");
        check_text(
            &mut problems,
            format!("[chain undo {}]", edits[step].0),
            &app.editor.lines,
            &history[step],
        );
    }
    idle(&mut app, &db);
    check_fresh(&mut problems, "[chain undo end]", &mut app);
    for step in 1..=edits.len() {
        app.handle_key(&db, Key::Ctrl('r')).expect("redo");
        check_text(
            &mut problems,
            format!("[chain redo {}]", edits[step - 1].0),
            &app.editor.lines,
            &history[step],
        );
    }
    idle(&mut app, &db);
    check_fresh(&mut problems, "[chain redo end]", &mut app);

    eprintln!(
        "{:<18} {:>8} {:>8} {:>8} {:>8}  (ms; keys + one frame)",
        "edit", "edit", "idle", "undo", "redo"
    );
    for (label, e, i, u, r) in &times {
        eprintln!("{label:<18} {e:>8.2} {i:>8.2} {u:>8.2} {r:>8.2}");
    }
    eprintln!("problems: {}", problems.len());
    for p in &problems {
        eprintln!("{p}");
    }
    drop(app);
    drop(db);
    cleanup_db_files(&path);
    assert!(problems.is_empty(), "{} problems", problems.len());
}

fn first_diff(ours: &[String], expected: &[String]) -> String {
    if ours.len() != expected.len() {
        return format!("{} lines vs {} expected", ours.len(), expected.len());
    }
    let idx = ours
        .iter()
        .zip(expected)
        .position(|(a, b)| a != b)
        .unwrap_or(0);
    format!("line {idx}: {:?} vs {:?}", ours[idx], expected[idx])
}

/// `text` with each table block formatted, like a note saved after editing.
fn formatted(text: &str) -> String {
    use crate::editor_core::table::{format_table_lines, is_table_line};
    let lines: Vec<String> = text.lines().map(str::to_string).collect();
    let mut out = Vec::with_capacity(lines.len());
    let mut idx = 0;
    while idx < lines.len() {
        if !is_table_line(&lines[idx]) {
            out.push(lines[idx].clone());
            idx += 1;
            continue;
        }
        let end = (idx..lines.len())
            .find(|&i| !is_table_line(&lines[i]))
            .unwrap_or(lines.len());
        out.extend(format_table_lines(&lines[idx..end]));
        idx = end;
    }
    out.join("\n")
}

/// Presses each step and checks the result against a fresh open.
fn assert_steps_match_fresh(note: &str, steps: &[&str]) {
    let (db, mut app, path) = app_with_note(&formatted(note));
    app.mode = UiMode::Normal;
    app.run_calc_recompute();
    let mut problems = Vec::new();
    for step in steps {
        if *step == "<ctrl-r>" {
            app.handle_key(&db, Key::Ctrl('r')).expect("redo");
            render_screen(&mut app);
        } else {
            press(&mut app, &db, step);
        }
        check_fresh(&mut problems, &format!("[{step}]"), &mut app);
    }
    drop(app);
    drop(db);
    cleanup_db_files(&path);
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

#[test]
fn formula_values_follow_deleted_and_pasted_rows_through_undo_and_redo() {
    assert_steps_match_fresh(
        "x := 2\n\n| n | v | d |\n| --- | --- | --- |\n| a | 1 | :=(1,2)*x |\n\
         | b | 2 | :=(2,2)*x |\n| c | 3 | :=(3,2)*x |\n| t | :=sum_col() | :=sum_col() |",
        &[
            "5Gdd", "u", "<ctrl-r>", "jp", "u", "u", "<ctrl-r>", "<ctrl-r>", "5G2dd", "u", "5GddP",
            "u", "<ctrl-r>", "6GyykP",
        ],
    );
}

#[test]
fn sum_col_counts_formula_cells_above_an_edited_row() {
    assert_steps_match_fresh(
        "| n | a | b | ab |\n| --- | --- | --- | --- |\n| r1 | 1 | 2 | :=(1,2)+(1,3) |\n\
         | r2 | 3 | 4 | :=(2,2)+(2,3) |\n| r3 | 5 | 6 | :=(3,2)+(3,3) |\n\
         | t | :=sum_col() | :=sum_col() | :=sum_col() |",
        &["4G0f|;wciw9<esc>", "u", "<ctrl-r>", "4G0f|;wa0<esc>"],
    );
}
