use super::*;
use crate::editor_core::types::SelectionSnapshot;
use app_core::scripts::{ScriptConfig, ScriptDefinition, ScriptInput, ScriptOutput};

fn configure(app: &mut TerminalApp, code: &str, input: ScriptInput, output: ScriptOutput) {
    let mut config = ScriptConfig::default();
    config.scripts.insert(
        "example".into(),
        ScriptDefinition {
            argv: vec!["sh".into(), "-c".into(), code.into()],
            input,
            output,
            timeout_seconds: 3,
        },
    );
    app.scripts = super::super::scripts::ScriptState::new(Ok(config));
}
fn wait_for_result(app: &mut TerminalApp) {
    let start = Instant::now();
    while (app.status.starts_with("running script") || app.status.starts_with("cancelling script"))
        && start.elapsed() < Duration::from_secs(5)
    {
        app.poll_script_result();
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(start.elapsed() < Duration::from_secs(5), "{}", app.status);
}
#[test]
#[cfg(unix)]
fn script_replaces_unicode_selection_with_one_undo_step() {
    let (db, mut app, path) = app_with_note("one é日 tail");
    configure(
        &mut app,
        "cat >/dev/null; printf '%s' '{\"text\":\"two\\nlines\"}'",
        ScriptInput::Selection,
        ScriptOutput::ReplaceSelection,
    );
    app.command_selection = Some(SelectionSnapshot { anchor: 6, head: 4 });
    app.execute_terminal_command(&db, "run example 'literal argument'");
    app.command_selection = None;
    wait_for_result(&mut app);
    assert_eq!(app.editor.lines, vec!["one two", "lines tail"]);
    app.undo(&db);
    assert_eq!(app.editor.lines, vec!["one é日 tail"]);
    app.redo(&db);
    assert_eq!(app.editor.lines, vec!["one two", "lines tail"]);
    let _ = fs::remove_file(path);
}
#[test]
#[cfg(unix)]
fn stale_result_and_cancellation_preserve_buffer() {
    let (db, mut app, path) = app_with_note("original");
    configure(
        &mut app,
        "cat >/dev/null; sleep 0.1; printf '{\"text\":\"changed\"}'",
        ScriptInput::Note,
        ScriptOutput::Insert,
    );
    app.execute_terminal_command(&db, "run example");
    app.handle_key(&db, Key::Char('x')).unwrap();
    wait_for_result(&mut app);
    assert_eq!(app.editor.lines, vec!["xoriginal"]);
    assert!(app.status.contains("buffer changed"));
    app.execute_terminal_command(&db, "run example");
    app.execute_terminal_command(&db, "run-cancel");
    wait_for_result(&mut app);
    assert!(app.status.contains("cancelled"));
    assert_eq!(app.editor.lines, vec!["xoriginal"]);
    let _ = fs::remove_file(path);
}
#[test]
#[cfg(unix)]
fn switching_away_and_back_discards_result() {
    let (db, mut app, path) = app_with_note("original");
    configure(
        &mut app,
        "cat >/dev/null; sleep 0.1; printf '{\"text\":\"changed\"}'",
        ScriptInput::None,
        ScriptOutput::Insert,
    );
    app.execute_terminal_command(&db, "run example");
    let note = db.get_note("n1").unwrap().unwrap();
    app.set_active_note(&db, note).unwrap();
    // Even returning to the same note is a different buffer context.
    app.status = "running script example".into();
    wait_for_result(&mut app);
    assert_eq!(app.editor.lines, vec!["original"]);
    assert!(app.status.contains("cancelled"));
    let _ = fs::remove_file(path);
}
#[test]
#[cfg(unix)]
fn script_errors_and_missing_selection_do_not_edit() {
    let (db, mut app, path) = app_with_note("original");
    configure(
        &mut app,
        "exit 9",
        ScriptInput::Selection,
        ScriptOutput::ReplaceSelection,
    );
    app.execute_terminal_command(&db, "run example");
    assert!(app.status.contains("selection"));
    configure(
        &mut app,
        "cat >/dev/null; echo failure >&2; exit 9",
        ScriptInput::None,
        ScriptOutput::Insert,
    );
    app.execute_terminal_command(&db, "run example");
    wait_for_result(&mut app);
    assert!(app.status.contains("failure"));
    assert_eq!(app.editor.lines, vec!["original"]);
    let _ = fs::remove_file(path);
}
#[test]
fn keybinding_sequences_replay_mismatches_and_timeouts() {
    let (db, mut app, path) = app_with_note("original");
    let config =
        ScriptConfig::parse("[keybindings.editor]\n'<C-r>'='format bold'\n'xy'='format italic'")
            .unwrap();
    app.scripts = super::super::scripts::ScriptState::new(Ok(config));
    app.handle_key(&db, Key::Char('x')).unwrap();
    assert_eq!(app.editor.lines, vec!["original"]);
    app.handle_key(&db, Key::Char('z')).unwrap();
    assert_eq!(app.editor.lines, vec!["xzoriginal"]);
    app.handle_key(&db, Key::Char('x')).unwrap();
    std::thread::sleep(Duration::from_millis(950));
    app.poll_script_binding_timeout(&db).unwrap();
    assert_eq!(app.editor.lines, vec!["xzxoriginal"]);
    app.handle_key(&db, Key::Ctrl('r')).unwrap();
    assert!(app.editor.lines[0].contains("****"));
    let _ = fs::remove_file(path);
}
#[test]
#[cfg(unix)]
fn visual_shortcut_runs_script_and_message_output_does_not_edit() {
    let (db, mut app, path) = app_with_note("original");
    configure(
        &mut app,
        "cat >/dev/null; printf '{\"text\":\"REPLACED\"}'",
        ScriptInput::Selection,
        ScriptOutput::ReplaceSelection,
    );
    let mut config = app.scripts.config.as_ref().unwrap().clone();
    config
        .keybindings
        .entry("visual".into())
        .or_default()
        .insert("<C-r>".into(), "run example".into());
    app.scripts = super::super::scripts::ScriptState::new(Ok(config));
    app.mode = UiMode::Visual;
    app.editor.selection_anchor = Some((0, 0));
    app.editor.cursor_col = 8;
    app.handle_key(&db, Key::Ctrl('r')).unwrap();
    wait_for_result(&mut app);
    assert_eq!(app.editor.lines, vec!["REPLACED"]);
    configure(
        &mut app,
        "cat >/dev/null; printf '{\"text\":\"status only\"}'",
        ScriptInput::Note,
        ScriptOutput::Message,
    );
    app.execute_terminal_command(&db, "run example");
    wait_for_result(&mut app);
    assert_eq!(app.status, "status only");
    assert_eq!(app.editor.lines, vec!["REPLACED"]);
    let _ = fs::remove_file(path);
}

#[test]
#[cfg(unix)]
fn single_character_visual_command_uses_the_highlighted_character() {
    let (db, mut app, path) = app_with_note("é tail");
    configure(
        &mut app,
        "cat >/dev/null; printf '{\"text\":\"É\"}'",
        ScriptInput::Selection,
        ScriptOutput::ReplaceSelection,
    );
    app.mode = UiMode::Visual;
    app.editor.selection_anchor = Some((0, 0));
    app.open_command_bar_from_vim_action();
    app.command_input = "run example".into();
    app.handle_key(&db, Key::Enter).unwrap();
    wait_for_result(&mut app);
    assert_eq!(app.editor.lines, vec!["É tail"]);
    let _ = fs::remove_file(path);
}

#[test]
#[cfg(target_os = "linux")]
fn dropping_editor_cleans_up_running_process() {
    let (db, mut app, path) = app_with_note("original");
    let pid_path = std::env::temp_dir().join(format!("slate-script-pid-{}", Ulid::new()));
    configure(
        &mut app,
        &format!(
            "printf '%s' $$ > '{}'; cat >/dev/null; sleep 30",
            pid_path.display()
        ),
        ScriptInput::None,
        ScriptOutput::Insert,
    );
    app.execute_terminal_command(&db, "run example");
    let started = Instant::now();
    while !pid_path.exists() && started.elapsed() < Duration::from_secs(2) {
        std::thread::sleep(Duration::from_millis(10));
    }
    let pid = fs::read_to_string(&pid_path).unwrap();
    let started = Instant::now();
    drop(app);
    assert!(started.elapsed() < Duration::from_secs(2));
    assert!(!std::path::Path::new(&format!("/proc/{pid}")).exists());
    let _ = fs::remove_file(pid_path);
    let _ = fs::remove_file(path);
}

#[test]
fn locked_note_cannot_run_scripts_and_registered_names_complete() {
    let (db, mut app, path) = app_with_note("original");
    configure(&mut app, "exit 0", ScriptInput::None, ScriptOutput::Insert);
    app.command_input = "run ex".into();
    assert!(app.open_command_completion_menu());
    assert_eq!(app.command_input, "run example");
    app.active_note.access_mode = NoteAccessMode::Encrypted;
    app.active_note.is_unlocked = false;
    app.execute_terminal_command(&db, "run example");
    assert!(app.status.contains("locked"));
    assert_eq!(app.editor.lines, vec!["original"]);
    let _ = fs::remove_file(path);
}

#[test]
fn normal_bindings_preserve_pending_vim_operators() {
    let (db, mut app, path) = app_with_note("one two");
    let config = ScriptConfig::parse("[keybindings.normal]\nw='help'").unwrap();
    app.scripts = super::super::scripts::ScriptState::new(Ok(config));
    app.mode = UiMode::Normal;
    app.handle_key(&db, Key::Char('d')).unwrap();
    app.handle_key(&db, Key::Char('w')).unwrap();
    assert_eq!(app.editor.lines, vec!["two"]);
    assert!(app.help.is_none());
    let _ = fs::remove_file(path);
}

#[test]
#[cfg(unix)]
fn selection_script_copies_selected_lines_without_joining_whole_note() {
    let body = format!("{}é tail", "unselected line\n".repeat(5000));
    let (db, mut app, path) = app_with_note(&body);
    configure(
        &mut app,
        "cat >/dev/null; printf '{\"text\":\"É\"}'",
        ScriptInput::Selection,
        ScriptOutput::ReplaceSelection,
    );
    app.editor.joined_text_cache = None;
    app.mode = UiMode::Visual;
    app.editor.cursor_line = 5000;
    app.editor.selection_anchor = Some((5000, 0));
    app.execute_terminal_command(&db, "run example");
    assert!(app.editor.joined_text_cache.is_none());
    wait_for_result(&mut app);
    assert_eq!(app.editor.lines[5000], "É tail");
    assert_eq!(app.editor.lines[0], "unselected line");
    let _ = fs::remove_file(path);
}
