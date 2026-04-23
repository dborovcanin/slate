use super::*;

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
