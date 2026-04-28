use super::*;
use app_core::calc::{CalcEngine, NoteEvaluationOptions};
use serde::Deserialize;

#[test]
fn vim_cross_frontend_parity_replay_cases() {
    let suite: VimParityReplaySuite =
        serde_json::from_str(include_str!("../../tests/golden/vim_parity_replay.json"))
            .expect("parse vim parity replay fixture");
    for case in suite.cases {
        let tui = run_tui_parity_case(&case);
        let gui = run_gui_parity_case(&case);
        assert_eq!(tui, gui, "cross-frontend parity mismatch in {}", case.name);
    }
}

#[test]
fn markdown_cross_frontend_parity_replay_cases() {
    let suite: MarkdownParityReplaySuite = serde_json::from_str(include_str!(
        "../../tests/golden/markdown_parity_replay.json"
    ))
    .expect("parse markdown parity replay fixture");
    for case in suite.cases {
        let tui = run_tui_markdown_parity_case(&case);
        let gui = run_gui_markdown_parity_case(&case);
        assert_eq!(tui, gui, "markdown parity mismatch in {}", case.name);
    }
}

#[derive(Debug, Deserialize)]
struct CalcParityReplaySuite {
    cases: Vec<CalcParityReplayCase>,
}

#[derive(Debug, Deserialize)]
struct CalcParityReplayCase {
    name: String,
    initial_text: String,
    modules: CalcParityModules,
}

#[derive(Debug, Deserialize)]
struct CalcParityModules {
    math: bool,
    table: bool,
    variables: bool,
}

#[derive(Debug, PartialEq, Eq)]
struct CalcParitySnapshot {
    line_results: Vec<Option<String>>,
    table_cell_results: Vec<Vec<(usize, String)>>,
}

fn calc_snapshot_for_modules(lines: &[String], modules: &CalcParityModules) -> CalcParitySnapshot {
    if !modules.math {
        return CalcParitySnapshot {
            line_results: vec![None; lines.len()],
            table_cell_results: vec![Vec::new(); lines.len()],
        };
    }

    let engine = CalcEngine::new();
    let result = engine.evaluate_note_context(
        lines,
        NoteEvaluationOptions {
            variables_enabled: modules.variables,
            table_enabled: modules.table,
            eval_range: None,
        },
    );
    let table_cell_results = result
        .table_cell_results
        .into_iter()
        .map(|row| {
            row.into_iter()
                .map(|entry| (entry.cell_index, entry.value))
                .collect()
        })
        .collect();
    CalcParitySnapshot {
        line_results: result.line_results,
        table_cell_results,
    }
}

fn calc_snapshot_via_tui_path(lines: &[String], modules: &CalcParityModules) -> CalcParitySnapshot {
    if !modules.math {
        return CalcParitySnapshot {
            line_results: vec![None; lines.len()],
            table_cell_results: vec![Vec::new(); lines.len()],
        };
    }
    let engine = CalcEngine::new();
    let data = super::super::calc_helpers::compute_calc_data(
        &engine,
        lines,
        modules.variables,
        modules.table,
        None,
    );
    CalcParitySnapshot {
        line_results: data.line_results,
        table_cell_results: data.cell_results,
    }
}

#[test]
fn calc_cross_module_parity_replay_cases() {
    let suite: CalcParityReplaySuite =
        serde_json::from_str(include_str!("../../tests/golden/calc_parity_replay.json"))
            .expect("parse calc parity replay fixture");
    for case in suite.cases {
        let lines = case
            .initial_text
            .split('\n')
            .map(|line| line.to_string())
            .collect::<Vec<_>>();
        let tui = calc_snapshot_via_tui_path(&lines, &case.modules);
        let gui = calc_snapshot_for_modules(&lines, &case.modules);
        assert_eq!(tui, gui, "calc parity mismatch in {}", case.name);
    }
}
