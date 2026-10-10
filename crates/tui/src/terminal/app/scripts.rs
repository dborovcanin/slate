//! Terminal adapters for core script configuration, workers, and edit plans.
use super::{Db, Key, TerminalApp, UiMode};
use crate::editor_core::buffer::{document_text_len, line_and_byte_for_offset};
use crate::editor_core::types::TextRange;
use app_core::scripts::{ScriptConfig, ScriptInput, ScriptOutput, ScriptRequest, ScriptResponse};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc, Arc,
};
use std::time::{Duration, Instant};

struct Binding {
    mode: String,
    keys: Vec<Key>,
    command: String,
}
struct RunningScript {
    name: String,
    note_id: String,
    generation: u64,
    range: TextRange,
    output: ScriptOutput,
    cancel: Arc<AtomicBool>,
    rx: mpsc::Receiver<Result<ScriptResponse, String>>,
    worker: Option<std::thread::JoinHandle<()>>,
}
pub(super) struct ScriptState {
    pub(super) config: Result<ScriptConfig, String>,
    bindings: Vec<Binding>,
    pending: Vec<Key>,
    pending_mode: Option<UiMode>,
    pending_since: Option<Instant>,
    replaying: bool,
    running: Option<RunningScript>,
}
impl Drop for RunningScript {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Release);
        // On editor exit, wait for process cleanup before the host exits.
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
impl ScriptState {
    pub(super) fn new(config: Result<ScriptConfig, String>) -> Self {
        let mut bindings = Vec::new();
        let config = config.and_then(|config| {
            for (mode, entries) in &config.keybindings {
                for (sequence, command) in entries {
                    let keys = parse_keys(sequence)?;
                    if command.trim().is_empty() {
                        return Err("keybinding command is empty".into());
                    }
                    if bindings.iter().any(|b: &Binding| {
                        b.mode == *mode && (b.keys.starts_with(&keys) || keys.starts_with(&b.keys))
                    }) {
                        return Err(format!("overlapping keybinding: {mode}.{sequence}"));
                    }
                    bindings.push(Binding {
                        mode: mode.clone(),
                        keys,
                        command: command.clone(),
                    });
                }
            }
            if bindings.len() > 128 {
                return Err("at most 128 keybindings are supported".into());
            }
            Ok(config)
        });
        if config.is_err() {
            bindings.clear();
        }
        Self {
            config,
            bindings,
            pending: Vec::new(),
            pending_mode: None,
            pending_since: None,
            replaying: false,
            running: None,
        }
    }
}
fn parse_keys(sequence: &str) -> Result<Vec<Key>, String> {
    let mut chars = sequence.chars();
    let mut keys = Vec::new();
    while let Some(ch) = chars.next() {
        let key = if ch == '<' {
            let mut token = String::new();
            let mut closed = false;
            for ch in chars.by_ref() {
                if ch == '>' {
                    closed = true;
                    break;
                }
                token.push(ch);
            }
            if !closed {
                return Err(format!("unclosed key token: {sequence}"));
            }
            match token.as_str() {
                "Space" => Key::Char(' '),
                "Esc" => Key::Esc,
                "Enter" => Key::Enter,
                "Tab" => Key::Tab,
                "Backspace" => Key::Backspace,
                "Delete" => Key::Delete,
                "Up" => Key::ArrowUp,
                "Down" => Key::ArrowDown,
                "Left" => Key::ArrowLeft,
                "Right" => Key::ArrowRight,
                "Home" => Key::Home,
                "End" => Key::End,
                "lt" => Key::Char('<'),
                t if t.starts_with("C-") && t.chars().count() == 3 => {
                    Key::Ctrl(t.chars().nth(2).unwrap().to_ascii_lowercase())
                }
                _ => return Err(format!("unsupported key token: <{token}>")),
            }
        } else {
            Key::Char(ch)
        };
        keys.push(key);
    }
    if keys.is_empty() || keys.len() > 8 {
        return Err("key sequence must contain 1..8 keys".into());
    }
    Ok(keys)
}
pub(super) fn mode_name(mode: UiMode) -> Option<&'static str> {
    match mode {
        UiMode::Normal => Some("normal"),
        UiMode::Editor => Some("editor"),
        UiMode::Visual | UiMode::VisualLine => Some("visual"),
        _ => None,
    }
}
impl TerminalApp {
    pub(super) fn try_script_keybinding(&mut self, db: &Db, key: &Key) -> Result<bool, String> {
        if self.scripts.replaying
            || self.scripts.bindings.is_empty()
            || mode_name(self.mode).is_none()
        {
            return Ok(false);
        }
        // Normal-mode bindings start between Vim commands. A pending operator,
        // count or `z` fold prefix must still receive its next key unchanged.
        if self.mode == UiMode::Normal
            && self.scripts.pending.is_empty()
            && (self.vim_state.pending.is_some()
                || !self.vim_state.count_buffer.is_empty()
                || self
                    .folds
                    .pending_prefix_until
                    .is_some_and(|until| Instant::now() <= until))
        {
            return Ok(false);
        }
        if !self.scripts.pending.is_empty() && self.scripts.pending_mode != Some(self.mode) {
            self.scripts.pending.clear();
        }
        if self.scripts.pending.is_empty()
            && !self.scripts.bindings.iter().any(|b| {
                Some(b.mode.as_str()) == mode_name(self.mode) && b.keys.first() == Some(key)
            })
        {
            return Ok(false);
        }
        if *key == Key::Esc && !self.scripts.pending.is_empty() {
            self.scripts.pending.clear();
            self.scripts.pending_since = None;
            return Ok(true);
        }
        self.scripts.pending.push(key.clone());
        self.scripts.pending_mode = Some(self.mode);
        self.scripts.pending_since = Some(Instant::now());
        let matching = self.scripts.bindings.iter().find(|b| {
            Some(b.mode.as_str()) == mode_name(self.mode)
                && b.keys.starts_with(&self.scripts.pending)
        });
        if let Some(binding) = matching {
            if binding.keys.len() == self.scripts.pending.len() {
                let command = binding.command.clone();
                self.scripts.pending.clear();
                self.scripts.pending_since = None;
                self.command_selection_linewise = self.mode == UiMode::VisualLine;
                self.command_selection = self.capture_visual_command_selection();
                self.session.history.break_coalescing();
                self.execute_terminal_command(db, &command);
                self.command_selection = None;
                self.command_selection_linewise = false;
                self.session.history.break_coalescing();
            }
        } else {
            // Replay the prefix as ordinary input, then handle the breaking key
            // afresh so that it can still start a binding of its own.
            self.scripts.pending.pop();
            self.replay_pending_binding(db)?;
            self.handle_key(db, key.clone())?;
        }
        Ok(true)
    }
    fn replay_pending_binding(&mut self, db: &Db) -> Result<(), String> {
        let keys = std::mem::take(&mut self.scripts.pending);
        self.scripts.pending_since = None;
        self.scripts.replaying = true;
        let result = keys
            .into_iter()
            .try_for_each(|key| self.handle_key(db, key));
        self.scripts.replaying = false;
        self.render_state.dirty = true;
        result
    }
    pub(super) fn poll_script_binding_timeout(&mut self, db: &Db) -> Result<(), String> {
        if self
            .scripts
            .pending_since
            .is_some_and(|t| t.elapsed() >= Duration::from_millis(900))
        {
            self.replay_pending_binding(db)?;
        }
        Ok(())
    }
    pub(super) fn cancel_script(&mut self) {
        if let Some(run) = &self.scripts.running {
            run.cancel.store(true, Ordering::Release);
            self.status = format!("cancelling script {}", run.name);
        } else {
            self.status = "no script is running".into();
        }
    }
    pub(super) fn cancel_script_on_note_change(&mut self) {
        if let Some(run) = &self.scripts.running {
            run.cancel.store(true, Ordering::Release);
        }
        self.scripts.pending.clear();
        self.scripts.pending_since = None;
    }
    pub(super) fn start_script(&mut self, arguments: &str) {
        if let Err(error) = self.start_script_inner(arguments) {
            self.status = format!("run: {error}");
        }
    }
    fn start_script_inner(&mut self, arguments: &str) -> Result<(), String> {
        if self.scripts.running.is_some() {
            return Err("a script is already running; use :run-cancel".into());
        }
        let mut args = app_core::scripts::parse_arguments(arguments)?;
        if args.is_empty() {
            return Err("usage: run <name> [args]".into());
        }
        let name = args.remove(0);
        let config = self.scripts.config.as_ref().map_err(Clone::clone)?;
        let script = config
            .scripts
            .get(&name)
            .ok_or_else(|| format!("unknown script: {name}"))?
            .clone();
        // Locked/protected notes cannot be exposed to external processes.
        if !self.active_note_is_editable() {
            return Err("note is read-only or locked".into());
        }
        let cursor = self.byte_offset_for_line_col(self.editor.cursor_line, self.editor.cursor_col);
        let mut range = TextRange {
            from: cursor,
            to: cursor,
        };
        let selection = self
            .command_selection
            .or_else(|| self.capture_visual_command_selection());
        let mut selected_text = String::new();
        if script.input == ScriptInput::Selection || script.output == ScriptOutput::ReplaceSelection
        {
            let selection = selection.ok_or("script requires a selection")?;
            let linewise = self.command_selection_linewise || self.mode == UiMode::VisualLine;
            let (first_line, _) =
                line_and_byte_for_offset(&self.editor.lines, selection.anchor.min(selection.head));
            let (last_line, _) =
                line_and_byte_for_offset(&self.editor.lines, selection.anchor.max(selection.head));
            let (snapshot, start) =
                self.build_scoped_snapshot_for_line_span(first_line, last_line, None);
            let selection = crate::editor_core::types::SelectionSnapshot {
                anchor: selection.anchor.saturating_sub(start),
                head: selection.head.saturating_sub(start),
            };
            let selected = crate::editor_core::scripts::visual_selection_range(
                &snapshot.text,
                selection,
                linewise,
            )?;
            if script.output == ScriptOutput::ReplaceSelection {
                range = TextRange {
                    from: start + selected.from,
                    to: start + selected.to,
                };
            }
            if script.input == ScriptInput::Selection {
                let text = &snapshot.text[selected.from..selected.to];
                if text.len() > app_core::scripts::MAX_INPUT_BYTES {
                    return Err("script input exceeds 16 MiB".into());
                }
                selected_text = text.to_owned();
            }
        }
        let text = match script.input {
            ScriptInput::None => String::new(),
            ScriptInput::Note => {
                if document_text_len(&self.editor.lines) > app_core::scripts::MAX_INPUT_BYTES {
                    return Err("script input exceeds 16 MiB".into());
                }
                self.joined_text_cached_ref().to_owned()
            }
            ScriptInput::Selection => selected_text,
        };
        let request = ScriptRequest {
            version: 1,
            args,
            text,
            note_id: Some(self.active_note.id.clone()),
        };
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = cancel.clone();
        let (tx, rx) = mpsc::channel();
        let output = script.output;
        let worker = std::thread::spawn(move || {
            let _ = tx.send(app_core::scripts::run_script(
                &script,
                &request,
                worker_cancel,
            ));
        });
        self.scripts.running = Some(RunningScript {
            name: name.clone(),
            note_id: self.active_note.id.clone(),
            generation: self.editor.text_generation,
            range,
            output,
            cancel,
            rx,
            worker: Some(worker),
        });
        self.status = format!("running script {name} (:run-cancel to stop)");
        Ok(())
    }
    pub(super) fn poll_script_result(&mut self) {
        if mode_name(self.mode).is_none() {
            return;
        }
        let Some(run) = &self.scripts.running else {
            return;
        };
        let response = match run.rx.try_recv() {
            Ok(result) => result,
            Err(mpsc::TryRecvError::Empty) => return,
            Err(mpsc::TryRecvError::Disconnected) => Err("script worker disconnected".into()),
        };
        let run = self.scripts.running.take().unwrap();
        self.render_state.dirty = true;
        if run.cancel.load(Ordering::Acquire) {
            self.status = format!("script {} cancelled", run.name);
            return;
        }
        match response {
            Err(error) => self.status = format!("script {}: {error}", run.name),
            Ok(response) => {
                if run.note_id != self.active_note.id
                    || run.generation != self.editor.text_generation
                    || !self.active_note_is_editable()
                {
                    self.status = format!("script {}: buffer changed; result discarded", run.name);
                    return;
                }
                if run.output != ScriptOutput::Message {
                    self.session.history.break_coalescing();
                    let op = crate::editor_core::scripts::plan_result(run.range, response.text);
                    self.apply_edit_operation(&op);
                    self.editor.selection_anchor = None;
                    if matches!(self.mode, UiMode::Visual | UiMode::VisualLine) {
                        // Leave Visual as Escape does: the Vim state machine
                        // must agree with the UI mode, or `i`/`u` stay inert.
                        self.mode = UiMode::Normal;
                        self.vim_state.mode = crate::editor_core::vim::VimMode::Normal;
                        self.vim_state.pending = None;
                        self.vim_state.pending_count = None;
                        self.vim_state.count_buffer.clear();
                    }
                    self.session.history.break_coalescing();
                    self.adjust_cursor();
                    self.adjust_scroll();
                    self.status = response
                        .message
                        .unwrap_or_else(|| format!("script {} finished", run.name));
                } else {
                    self.status = response.message.unwrap_or(response.text);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn key_sequences_and_conflicts() {
        assert_eq!(
            parse_keys("<Space>tc").unwrap(),
            vec![Key::Char(' '), Key::Char('t'), Key::Char('c')]
        );
        assert_eq!(parse_keys("<C-r>").unwrap(), vec![Key::Ctrl('r')]);
        assert!(parse_keys("<Nope>").is_err());
        let cfg = ScriptConfig::parse("[keybindings.normal]\ng='help'\ngg='run foo'").unwrap();
        assert!(ScriptState::new(Ok(cfg)).config.is_err());
    }
}
