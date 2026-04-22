use editor_core::engine::{EditorEngine, ModuleCommandPlan, ModuleState};
use editor_core::types::CommandMode;
use editor_core::vim::{parse_key_token, VimAction, VimContext, VimMode, VimState};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct VimReplaySuite {
    cases: Vec<VimReplayCase>,
}

#[derive(Debug, Deserialize)]
struct VimReplayCase {
    name: String,
    initial_state: VimState,
    context: VimContext,
    keys: Vec<String>,
    expected: VimReplayExpected,
}

#[derive(Debug, Deserialize)]
struct VimReplayExpected {
    mode: VimMode,
    handled: bool,
    actions: Vec<VimAction>,
}

#[derive(Debug, Deserialize)]
struct ModuleReplaySuite {
    cases: Vec<ModuleReplayCase>,
}

#[derive(Debug, Deserialize)]
struct ModuleReplayCase {
    name: String,
    mode: CommandMode,
    raw_input: String,
    current: ModuleState,
    expected: ModuleCommandPlan,
}

#[derive(Debug, Deserialize)]
struct CommandReplaySuite {
    cases: Vec<CommandReplayCase>,
}

#[derive(Debug, Deserialize)]
struct CommandReplayCase {
    name: String,
    raw_input: String,
    expected_normalized: String,
    mode: CommandMode,
    expected_resolved: Option<String>,
    expected_note_security: Option<ExpectedNoteSecurity>,
}

#[derive(Debug, Deserialize)]
struct ExpectedNoteSecurity {
    action: String,
    password: String,
    used_note_prefix: bool,
}

#[test]
fn vim_replay_golden_cases() {
    let suite: VimReplaySuite = serde_json::from_str(include_str!("golden/vim_replay.json"))
        .expect("parse vim replay fixture");

    for case in suite.cases {
        let mut state = case.initial_state.clone();
        let mut last_mode = state.mode;
        let mut last_handled = false;
        let mut last_actions: Vec<VimAction> = Vec::new();

        for token in &case.keys {
            let key = parse_key_token(token).unwrap_or_else(|| {
                panic!("case {} has invalid key token: {token}", case.name);
            });
            let step = EditorEngine::step_vim(&state, key, &case.context);
            last_mode = step.state.mode;
            last_handled = step.handled;
            last_actions = step.actions;
            state = step.state;
        }

        assert_eq!(
            last_mode, case.expected.mode,
            "mode mismatch in {}",
            case.name
        );
        assert_eq!(
            last_handled, case.expected.handled,
            "handled mismatch in {}",
            case.name
        );
        assert_eq!(
            last_actions, case.expected.actions,
            "actions mismatch in {}",
            case.name
        );
    }
}

#[test]
fn module_command_replay_golden_cases() {
    let suite: ModuleReplaySuite =
        serde_json::from_str(include_str!("golden/module_command_replay.json"))
            .expect("parse module replay fixture");

    for case in suite.cases {
        let command =
            EditorEngine::resolve_command(case.mode, &case.raw_input).unwrap_or_else(|| {
                panic!(
                    "case {} failed to resolve command '{}'",
                    case.name, case.raw_input
                )
            });
        let plan =
            EditorEngine::plan_module_command(command.id, case.current).unwrap_or_else(|| {
                panic!(
                    "case {} did not produce module plan for '{}'",
                    case.name, case.raw_input
                )
            });
        assert_eq!(plan, case.expected, "module plan mismatch in {}", case.name);
    }
}

#[test]
fn command_replay_golden_cases() {
    let suite: CommandReplaySuite =
        serde_json::from_str(include_str!("golden/command_replay.json"))
            .expect("parse command replay fixture");

    for case in suite.cases {
        let normalized = EditorEngine::normalize_command(&case.raw_input);
        assert_eq!(
            normalized, case.expected_normalized,
            "normalized mismatch in {}",
            case.name
        );

        let resolved = EditorEngine::resolve_command(case.mode, &case.raw_input)
            .map(|command| command.value.to_string());
        assert_eq!(
            resolved, case.expected_resolved,
            "resolved mismatch in {}",
            case.name
        );

        let parsed = EditorEngine::parse_note_security_command(&case.raw_input);
        match (&parsed, &case.expected_note_security) {
            (None, None) => {}
            (Some(_), None) => panic!("expected no note security parse in {}", case.name),
            (None, Some(_)) => panic!("expected note security parse in {}", case.name),
            (Some(actual), Some(expected)) => {
                assert_eq!(
                    actual.action.as_str(),
                    expected.action,
                    "note action mismatch in {}",
                    case.name
                );
                assert_eq!(
                    actual.password, expected.password,
                    "note password mismatch in {}",
                    case.name
                );
                assert_eq!(
                    actual.used_note_prefix, expected.used_note_prefix,
                    "note prefix mismatch in {}",
                    case.name
                );
            }
        }
    }
}
