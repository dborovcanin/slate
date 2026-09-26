use super::*;
use app_core::calc::{CalcEngine, TableCellEvaluation};
use serde::Deserialize;

#[test]
fn vim_replay_cases_match_expected() {
    let suite: VimReplaySuite =
        serde_json::from_str(include_str!("../../tests/golden/vim_replay.json"))
            .expect("parse vim replay fixture");
    for case in suite.cases {
        let expected = case
            .expected
            .as_ref()
            .unwrap_or_else(|| panic!("vim replay case '{}' has no expected", case.name));
        let actual = run_vim_replay_case(&case);
        assert_eq!(actual.lines, expected.lines, "lines in {}", case.name);
        assert_eq!(
            (actual.cursor_line, actual.cursor_col),
            (expected.cursor_line, expected.cursor_col),
            "cursor in {}",
            case.name
        );
        assert_eq!(
            format!("{:?}", actual.mode),
            expected.mode,
            "mode in {}",
            case.name
        );
        assert_eq!(
            actual.vim_state, expected.vim_state,
            "vim state in {}",
            case.name
        );
        assert_eq!(
            actual.selection_anchor, expected.selection_anchor,
            "selection anchor in {}",
            case.name
        );
        assert_eq!(
            actual.clipboard_text, expected.clipboard_text,
            "clipboard text in {}",
            case.name
        );
        assert_eq!(
            format!("{:?}", actual.clipboard_mode),
            expected.clipboard_mode,
            "clipboard mode in {}",
            case.name
        );
    }
}

#[test]
fn markdown_replay_cases_match_expected() {
    let suite: MarkdownReplaySuite =
        serde_json::from_str(include_str!("../../tests/golden/markdown_replay.json"))
            .expect("parse markdown replay fixture");
    for case in suite.cases {
        let actual = run_markdown_replay_case(&case);
        assert_eq!(
            actual, case.expected,
            "markdown replay mismatch in {}",
            case.name
        );
    }
}

#[derive(Debug, Deserialize)]
struct CalcReplaySuite {
    cases: Vec<CalcReplayCase>,
}

#[derive(Debug, Deserialize)]
struct CalcReplayCase {
    name: String,
    initial_text: String,
    modules: CalcReplayModules,
    expected: CalcReplaySnapshot,
}

#[derive(Debug, Deserialize)]
struct CalcReplayModules {
    math: bool,
    table: bool,
    variables: bool,
}

#[derive(Debug, PartialEq, Eq, Deserialize)]
struct CalcReplaySnapshot {
    line_results: Vec<Option<String>>,
    table_cell_results: Vec<Vec<TableCellEvaluation>>,
}

fn calc_snapshot(lines: &[String], modules: &CalcReplayModules) -> CalcReplaySnapshot {
    if !modules.math {
        return CalcReplaySnapshot {
            line_results: vec![None; lines.len()],
            table_cell_results: vec![Vec::new(); lines.len()],
        };
    }
    let engine = CalcEngine::new();
    let data = super::super::calc_helpers::compute_calc_data(
        &engine,
        lines,
        modules.variables,
        true,
        modules.table,
        None,
        Vec::new(),
    );
    CalcReplaySnapshot {
        line_results: data.line_results,
        table_cell_results: data.cell_results,
    }
}

#[test]
fn calc_replay_cases_match_expected() {
    let suite: CalcReplaySuite =
        serde_json::from_str(include_str!("../../tests/golden/calc_replay.json"))
            .expect("parse calc replay fixture");
    for case in suite.cases {
        let lines = case
            .initial_text
            .split('\n')
            .map(|line| line.to_string())
            .collect::<Vec<_>>();
        let actual = calc_snapshot(&lines, &case.modules);
        assert_eq!(
            actual, case.expected,
            "calc replay mismatch in {}",
            case.name
        );
    }
}

fn change_inside_tilde_case(
    name: String,
    text: &str,
    cursor_col: usize,
    keys: &[&str],
) -> VimReplayCase {
    VimReplayCase {
        name,
        initial_text: text.to_string(),
        initial_state: crate::editor_core::vim::VimState::default(),
        initial_cursor_line: 0,
        initial_cursor_col: cursor_col,
        keys: keys.iter().map(|key| key.to_string()).collect(),
        expected: None,
    }
}

#[test]
fn vim_ci_tilde_preserves_double_tilde_boundaries() {
    let case = change_inside_tilde_case(
        "ci-tilde-double-marker".to_string(),
        "~~test~~",
        3,
        &["char:c", "char:i", "char:~"],
    );
    let actual = run_vim_replay_case(&case);
    assert_eq!(actual.lines, vec!["~~~~".to_string()]);
    assert_eq!(actual.mode, UiMode::Editor);
}

#[test]
fn vim_ci_tilde_on_mixed_line_never_deletes_tilde_boundaries() {
    let text = "~~test~~ dsd";
    for cursor_col in 0..=text.chars().count() {
        let case = change_inside_tilde_case(
            format!("ci-tilde-mixed-col-{cursor_col}"),
            text,
            cursor_col,
            &["char:c", "char:i", "char:~"],
        );
        let expected = if cursor_col <= 7 {
            "~~~~ dsd".to_string()
        } else {
            text.to_string()
        };
        let actual = run_vim_replay_case(&case);
        assert_eq!(actual.lines, vec![expected], "cursor_col={cursor_col}");
    }
}

#[test]
fn vim_left_then_ci_tilde_on_mixed_line_keeps_boundaries() {
    let text = "~~test~~ dsd";
    for cursor_col in 0..=text.chars().count() {
        let case = change_inside_tilde_case(
            format!("left-ci-tilde-mixed-col-{cursor_col}"),
            text,
            cursor_col,
            &["arrow_left", "char:c", "char:i", "char:~"],
        );
        let moved_col = cursor_col.saturating_sub(1);
        let expected = if moved_col <= 7 {
            "~~~~ dsd".to_string()
        } else {
            text.to_string()
        };
        let actual = run_vim_replay_case(&case);
        assert_eq!(actual.lines, vec![expected], "cursor_col={cursor_col}");
    }
}
