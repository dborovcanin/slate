use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::cursor::SetCursorStyle;
use ratatui::crossterm::event::{
    DisableBracketedPaste, EnableBracketedPaste, KeyboardEnhancementFlags,
    PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{
    self, disable_raw_mode, enable_raw_mode, BeginSynchronizedUpdate, EndSynchronizedUpdate,
    EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::{Frame, Terminal};
use std::io::{self, Stdout};

use std::sync::Arc;
use super::graphics::GraphicsContext;
use super::input;

/// Where the terminal cursor goes after a frame, in 0-based cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CursorPlacement {
    pub row: u16,
    pub col: u16,
    pub block: bool,
}

/// Owns the terminal for the lifetime of an interactive session: raw mode,
/// alternate screen, bracketed paste, keyboard enhancement, and the ratatui
/// `Terminal` that diffs and flushes frames. Restores everything on drop.
pub struct TerminalSession {
    terminal: Terminal<CrosstermBackend<Stdout>>,
    keyboard_enhanced: bool,
    cursor_block: Option<bool>,
    graphics: Arc<GraphicsContext>,
}

impl TerminalSession {
    pub fn enter() -> Result<Self, String> {
        enable_raw_mode().map_err(|e| format!("Failed to enable raw terminal mode: {e}"))?;
        let mut stdout = io::stdout();
        if let Err(e) = execute!(stdout, EnterAlternateScreen, EnableBracketedPaste) {
            let _ = disable_raw_mode();
            return Err(format!("Failed to initialize terminal screen: {e}"));
        }
        // Kitty keyboard protocol: lets Shift+Enter, Ctrl+Backspace and Esc be
        // reported unambiguously on terminals that support it.
        let keyboard_enhanced = terminal::supports_keyboard_enhancement().unwrap_or(false)
            && execute!(
                stdout,
                PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
            )
            .is_ok();

        let graphics = Arc::new(GraphicsContext::new());

        let mut session = Self {
            terminal: match Terminal::new(CrosstermBackend::new(stdout)) {
                Ok(terminal) => terminal,
                Err(e) => {
                    restore_terminal(false);
                    return Err(format!("Failed to create terminal: {e}"));
                }
            },
            keyboard_enhanced,
            cursor_block: None,
            graphics,
        };
        let size = session
            .terminal
            .size()
            .map_err(|e| format!("Failed to read terminal size: {e}"))?;
        input::set_terminal_size(size.height, size.width);
        session
            .terminal
            .clear()
            .map_err(|e| format!("Failed to clear terminal: {e}"))?;
        Ok(session)
    }

    /// Renders one frame. `render` paints the buffer and returns where the
    /// cursor should be; ratatui flushes only the cells that changed.
    pub fn draw(
        &mut self,
        render: impl FnOnce(&mut Frame) -> Result<CursorPlacement, String>,
    ) -> Result<(), String> {
        let mut outcome: Result<CursorPlacement, String> = Err(String::new());
        // Synchronized update: the terminal presents the frame atomically,
        // avoiding tearing on large redraws. Ignored by terminals without support.
        let _ = execute!(self.terminal.backend_mut(), BeginSynchronizedUpdate);
        let drawn = self
            .terminal
            .draw(|frame| {
                let area = frame.area();
                input::set_terminal_size(area.height, area.width);
                outcome = render(frame);
                if let Ok(cursor) = &outcome {
                    frame.set_cursor_position((cursor.col, cursor.row));
                }
            })
            .map(|_| ())
            .map_err(|e| format!("Failed to draw terminal UI: {e}"));
        let _ = execute!(self.terminal.backend_mut(), EndSynchronizedUpdate);
        drawn?;
        let cursor = outcome?;
        if self.cursor_block != Some(cursor.block) {
            let style = if cursor.block {
                SetCursorStyle::BlinkingBlock
            } else {
                SetCursorStyle::BlinkingBar
            };
            execute!(self.terminal.backend_mut(), style)
                .map_err(|e| format!("Failed to set cursor style: {e}"))?;
            self.cursor_block = Some(cursor.block);
        }
        Ok(())
    }

    pub fn graphics(&self) -> &Arc<GraphicsContext> {
        &self.graphics
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        let _ = execute!(self.terminal.backend_mut(), SetCursorStyle::DefaultUserShape);
        let _ = self.terminal.show_cursor();
        restore_terminal(self.keyboard_enhanced);
    }
}

fn restore_terminal(keyboard_enhanced: bool) {
    let mut stdout = io::stdout();
    if keyboard_enhanced {
        let _ = execute!(stdout, PopKeyboardEnhancementFlags);
    }
    let _ = execute!(stdout, DisableBracketedPaste, LeaveAlternateScreen);
    let _ = disable_raw_mode();
}
