//! The main window: title bar, notes sidebar, editor, status bar and the
//! overlays drawn over them.
//!
//! Painting and input routing only. Line content comes from `NoteHost`, key
//! semantics from `note_session::input`, commands from `editor_core`.
use crate::editor_lines::{self, CursorShape, LineStyle};
use crate::keys::{self, EditingMode, KeyCommand};
use crate::note_view::{CommandRun, LineKind, LineView, NoteHost};
use crate::theme::Theme;
use editor_core::markdown_tokens::FenceState;
use editor_core::vim::{VimAction, VimIntent, VimKey, VimMode};
use gpui::{
    div, list, prelude::*, px, AnyElement, ClipboardItem, Context, FocusHandle, FontWeight,
    KeyDownEvent, ListAlignment, ListState, MouseButton, SharedString, Window,
};
use note_session::display::mapping::Affinity;
use note_session::input::{HostRequest, InputOutcome};
use std::time::Duration;

/// The word around character column `col` of `text`: a run of letters,
/// digits and `_`, or of other non-space characters.
fn word_at(text: &str, col: usize) -> (usize, usize) {
    let chars: Vec<char> = text.chars().collect();
    if chars.is_empty() {
        return (0, 0);
    }
    let col = col.min(chars.len() - 1);
    let class = |c: char| {
        if c.is_alphanumeric() || c == '_' {
            0
        } else if c.is_whitespace() {
            1
        } else {
            2
        }
    };
    let k = class(chars[col]);
    let mut from = col;
    while from > 0 && class(chars[from - 1]) == k {
        from -= 1;
    }
    let mut to = col + 1;
    while to < chars.len() && class(chars[to]) == k {
        to += 1;
    }
    (from, to)
}

pub const MENUS: [&str; 6] = ["File", "Edit", "View", "Format", "Calc", "Help"];
/// Sidebar entries; the rest of the notes are one search away.
const SIDEBAR_NOTES: usize = 200;
/// Unsaved edits are written once typing pauses this long.
const AUTOSAVE_DELAY: Duration = Duration::from_millis(1500);

pub struct Fonts {
    pub sans: SharedString,
    pub mono: SharedString,
}

pub struct SlateWindow {
    pub(crate) host: NoteHost,
    pub(crate) theme: Theme,
    pub(crate) light: bool,
    pub(crate) fonts: Fonts,
    pub(crate) focus: FocusHandle,
    pub(crate) mode: EditingMode,
    pub(crate) sidebar: bool,
    /// Transient message in the status bar.
    pub(crate) status: Option<String>,
    pub(crate) overlay: crate::overlays::Overlay,
    /// Lines of the table under the mouse, which shows its add row/column bars.
    hover_table: Option<(usize, usize)>,
    /// Text layouts from the last paint, by line, to map mouse positions to characters.
    layouts: std::cell::RefCell<std::collections::HashMap<usize, gpui::TextLayout>>,
    /// Where a mouse drag started; set while the left button is held.
    drag_anchor: Option<(usize, usize)>,
    /// Window size in pixels from the last paint, to reveal far jumps and fit the status bar.
    pub(crate) viewport: std::cell::Cell<(f32, f32)>,
    fences: Vec<FenceState>,
    cache: Vec<Option<LineView>>,
    list: ListState,
}

impl SlateWindow {
    pub fn new(host: NoteHost, light: bool, fonts: Fonts, cx: &mut Context<Self>) -> Self {
        let count = host.doc.lines().len();
        let mut this = Self {
            host,
            theme: if light { Theme::light() } else { Theme::dark() },
            light,
            fonts,
            focus: cx.focus_handle(),
            mode: EditingMode::Vim,
            sidebar: true,
            status: None,
            overlay: Default::default(),
            hover_table: None,
            layouts: Default::default(),
            drag_anchor: None,
            viewport: std::cell::Cell::new((1280.0, 800.0)),
            fences: Vec::new(),
            cache: Vec::new(),
            list: ListState::new(count, ListAlignment::Top, px(600.0)),
        };
        this.reload_lines();
        cx.spawn(async move |this, cx| loop {
            cx.background_executor()
                .timer(Duration::from_millis(500))
                .await;
            let alive = this.update(cx, |this, cx| {
                if this.host.autosave_due(AUTOSAVE_DELAY) {
                    this.save(cx);
                }
            });
            if alive.is_err() {
                break;
            }
        })
        .detach();
        this
    }

    pub fn focus_handle(&self) -> &FocusHandle {
        &self.focus
    }

    /// The note was replaced: rebuild every line and start at the top.
    pub(crate) fn reload_lines(&mut self) {
        self.fences = self.host.fence_starts();
        let count = self.fences.len();
        self.cache = vec![None; count];
        self.list.reset(count);
    }

    /// Same lines, new look (theme, mode, calc results): restyle on the next
    /// paint without touching the list, so the scroll position stays.
    pub(crate) fn restyle(&mut self) {
        self.fences = self.host.fence_starts();
        self.cache = vec![None; self.fences.len()];
    }

    /// Text changed from line `first` on. Only the lines that were replaced
    /// are remeasured, so the scroll position stays; items are remeasured
    /// whenever they are painted, so edits that keep the line count need no
    /// list update at all.
    pub(crate) fn resync_items(&mut self, first: usize) {
        let old = self.cache.len();
        self.restyle();
        let count = self.cache.len();
        if count == old {
            return;
        }
        let first = first.min(old).min(count);
        let delta = count as isize - old as isize;
        let removed_end = (first + 1 + (-delta).max(0) as usize).min(old);
        let added = count - first - (old - removed_end);
        self.list.splice(first..removed_end, added);
    }

    /// First line of the list that is on screen (or near it).
    fn top_line(&self) -> usize {
        self.list.logical_scroll_top().item_ix
    }

    /// Scroll so the cursor line is visible. Lines that were never painted
    /// have no height yet, so a far jump lands the cursor mid-screen and a
    /// second pass, once those lines are measured, makes it exact.
    pub(crate) fn reveal_cursor(&self) {
        let ix = self.host.doc.cursor_line;
        let rows = ((self.viewport.get().1 / 26.0) as usize).max(8);
        let top = self.top_line();
        let near = (ix >= top && ix + 2 < top + rows) || (ix < top && top - ix < rows);
        if near {
            self.list.scroll_to_reveal_item(ix);
        } else {
            self.list.scroll_to(gpui::ListOffset {
                item_ix: ix.saturating_sub(rows / 2),
                offset_in_item: px(0.0),
            });
        }
    }

    /// Restyle lines `from..=to` (clamped) on the next paint.
    fn invalidate(&mut self, from: usize, to: usize) {
        let n = self.cache.len();
        if n == 0 {
            return;
        }
        let (from, to) = (from.min(n - 1), to.min(n - 1));
        let (from, to) = (from.min(to), from.max(to));
        for slot in &mut self.cache[from..=to] {
            *slot = None;
        }
        self.list.splice(from..to + 1, to + 1 - from);
    }

    pub(crate) fn set_status(&mut self, msg: impl Into<String>) {
        self.status = Some(msg.into());
    }

    pub(crate) fn save(&mut self, cx: &mut Context<Self>) {
        match self.host.save() {
            Ok(true) => self.set_status("saved"),
            Ok(false) => {}
            Err(err) => self.set_status(format!("save failed: {err}")),
        }
        cx.notify();
    }

    pub(crate) fn open_note(&mut self, id: &str, cx: &mut Context<Self>) {
        if id == self.host.note_id() {
            return;
        }
        match self.host.switch_to(id) {
            Ok(()) => {
                if self.mode == EditingMode::Standard {
                    self.host.input.vim.mode = VimMode::Insert;
                }
                self.reload_lines();
                self.list.scroll_to_reveal_item(0);
                if self.host.locked() {
                    crate::overlays::open_prompt(self, crate::overlays::PromptKind::Unlock, "", cx);
                }
            }
            Err(err) => self.set_status(err),
        }
        cx.notify();
    }

    pub(crate) fn set_mode(&mut self, mode: EditingMode, cx: &mut Context<Self>) {
        self.mode = mode;
        self.host.input = Default::default();
        if mode == EditingMode::Standard {
            // Standard editing types directly: the session stays in insert.
            self.host.input.vim.mode = VimMode::Insert;
        }
        self.host.doc.selection_anchor = None;
        self.restyle();
        self.set_status(match mode {
            EditingMode::Vim => "vim editing",
            EditingMode::Standard => "standard editing",
        });
        cx.notify();
    }

    pub(crate) fn toggle_theme(&mut self, cx: &mut Context<Self>) {
        self.light = !self.light;
        self.theme = if self.light {
            Theme::light()
        } else {
            Theme::dark()
        };
        self.restyle();
        cx.notify();
    }

    /// Bring the view in line with an input outcome and act on requests.
    pub(crate) fn after_input(
        &mut self,
        before: (usize, Option<(usize, usize)>),
        outcome: InputOutcome,
        cx: &mut Context<Self>,
    ) {
        let (old_cursor, old_anchor) = before;
        if outcome.text_changed {
            let first = outcome
                .first_changed_line
                .unwrap_or_else(|| self.top_line());
            self.resync_items(first);
        } else {
            let cursor = self.host.doc.cursor_line;
            self.invalidate(old_cursor, old_cursor);
            self.invalidate(cursor, cursor);
            let anchor = self.host.doc.selection_anchor.or(old_anchor);
            if let Some((line, _)) = anchor {
                self.invalidate(
                    line.min(cursor).min(old_cursor),
                    line.max(cursor).max(old_cursor),
                );
            }
        }
        for request in outcome.requests {
            self.handle_request(request, cx);
        }
        self.reveal_cursor();
        // Lines near the cursor were just remeasured: reveal once more.
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(32))
                .await;
            let _ = this.update(cx, |this, cx| {
                this.list.scroll_to_reveal_item(this.host.doc.cursor_line);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn handle_request(&mut self, request: HostRequest, cx: &mut Context<Self>) {
        match request {
            HostRequest::CopyToClipboard(text) => {
                cx.write_to_clipboard(ClipboardItem::new_string(text));
                self.set_status("yanked to clipboard");
            }
            HostRequest::PasteFromClipboard(action) => self.paste_clipboard(&action, cx),
            HostRequest::OpenCommandBar => self.open_palette(":", cx),
            HostRequest::OpenSearch => self.set_status("search is not in the desktop app yet"),
            HostRequest::SearchNext | HostRequest::SearchPrev => {}
            HostRequest::Unsupported(intent) => {
                self.set_status(format!("{intent:?} is not in the desktop app yet"))
            }
        }
    }

    fn paste_clipboard(&mut self, action: &VimAction, cx: &mut Context<Self>) {
        let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) else {
            self.set_status("clipboard is empty");
            return;
        };
        let before = self.snapshot_cursor();
        let outcome = self.host.paste_clipboard(text, action);
        self.after_input(before, outcome, cx);
    }

    pub(crate) fn snapshot_cursor(&self) -> (usize, Option<(usize, usize)>) {
        (self.host.doc.cursor_line, self.host.doc.selection_anchor)
    }

    /// Feed one key to the session's input pipeline.
    pub(crate) fn send_key(&mut self, key: VimKey, cx: &mut Context<Self>) {
        let before = self.snapshot_cursor();
        let outcome = self.host.handle_key(key);
        if self.mode == EditingMode::Standard && self.host.input.mode() != VimMode::Insert {
            self.host.input.vim.mode = VimMode::Insert;
        }
        self.after_input(before, outcome, cx);
    }

    /// Run a vim action directly (menus, standard shortcuts).
    pub(crate) fn run_action(&mut self, intent: VimIntent, cx: &mut Context<Self>) {
        let before = self.snapshot_cursor();
        let action = VimAction {
            intent,
            count: 1,
            target_char: None,
        };
        let outcome = self.host.apply_vim_action(&action);
        self.after_input(before, outcome, cx);
    }

    pub(crate) fn copy_selection(&mut self, cx: &mut Context<Self>) {
        match self.host.selected_text() {
            Some(text) => {
                cx.write_to_clipboard(ClipboardItem::new_string(text));
                self.set_status("copied");
            }
            None => self.set_status("nothing selected"),
        }
        cx.notify();
    }

    pub(crate) fn cut_selection(&mut self, cx: &mut Context<Self>) {
        let before = self.snapshot_cursor();
        match self.host.cut_selection() {
            Some((text, outcome)) => {
                cx.write_to_clipboard(ClipboardItem::new_string(text));
                self.leave_visual();
                self.after_input(before, outcome, cx);
            }
            None => {
                self.set_status("nothing selected");
                cx.notify();
            }
        }
    }

    pub(crate) fn paste_text(&mut self, cx: &mut Context<Self>) {
        let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) else {
            self.set_status("clipboard is empty");
            return;
        };
        let before = self.snapshot_cursor();
        let outcome = self.host.insert_text(&text.replace("\r\n", "\n"));
        self.after_input(before, outcome, cx);
    }

    fn leave_visual(&mut self) {
        if matches!(
            self.host.input.mode(),
            VimMode::Visual | VimMode::VisualLine
        ) {
            self.host.input.vim = Default::default();
        }
        self.host.doc.selection_anchor = None;
    }

    /// Run a command-bar command; host commands go to `commands`.
    pub(crate) fn run_command(&mut self, raw: &str, cx: &mut Context<Self>) {
        let before = self.snapshot_cursor();
        match self.host.run_command(raw) {
            CommandRun::Done {
                message,
                clipboard,
                quit,
            } => {
                if let Some(text) = clipboard {
                    cx.write_to_clipboard(ClipboardItem::new_string(text));
                }
                if !message.is_empty() {
                    self.set_status(message);
                }
                let top = self.top_line();
                self.resync_items(top);
                self.after_input(before, InputOutcome::default(), cx);
                if quit {
                    self.quit(cx);
                }
            }
            CommandRun::Host { id, raw } => crate::commands::run_host_command(self, id, &raw, cx),
        }
    }

    pub(crate) fn quit(&mut self, cx: &mut Context<Self>) {
        if let Err(err) = self.host.save() {
            self.set_status(format!("not quitting, save failed: {err}"));
            cx.notify();
            return;
        }
        cx.quit();
    }

    /// Move or extend the selection in standard editing.
    fn standard_move(
        &mut self,
        key: Option<VimKey>,
        to_end: Option<bool>,
        select: bool,
        cx: &mut Context<Self>,
    ) {
        let before = self.snapshot_cursor();
        let doc = &mut self.host.doc;
        if select && doc.selection_anchor.is_none() {
            doc.selection_anchor = Some((doc.cursor_line, doc.cursor_col));
        }
        if !select {
            doc.selection_anchor = None;
        }
        match (key, to_end) {
            (Some(key), _) => {
                let outcome = self.host.handle_key(key);
                self.after_input(before, outcome, cx);
            }
            (None, Some(end)) => {
                let doc = &mut self.host.doc;
                doc.cursor_col = if end {
                    doc.lines()[doc.cursor_line].chars().count()
                } else {
                    0
                };
                self.after_input(before, InputOutcome::default(), cx);
            }
            _ => {}
        }
    }

    fn select_all(&mut self, cx: &mut Context<Self>) {
        let before = self.snapshot_cursor();
        let doc = &mut self.host.doc;
        let last = doc.lines().len().saturating_sub(1);
        doc.selection_anchor = Some((0, 0));
        doc.cursor_line = last;
        doc.cursor_col = doc.lines()[last].chars().count();
        self.after_input(before, InputOutcome::default(), cx);
    }

    /// Typing over a selection in standard editing replaces it.
    fn standard_type_over(&mut self, key: VimKey, cx: &mut Context<Self>) -> bool {
        if self.mode != EditingMode::Standard || self.host.doc.selection_anchor.is_none() {
            return false;
        }
        let before = self.snapshot_cursor();
        match key {
            VimKey::Char(ch) => {
                let outcome = self.host.insert_text(&ch.to_string());
                self.after_input(before, outcome, cx);
                true
            }
            VimKey::Backspace | VimKey::Delete => {
                if let Some((_, outcome)) = self.host.cut_selection() {
                    self.after_input(before, outcome, cx);
                }
                true
            }
            _ => {
                self.host.doc.selection_anchor = None;
                false
            }
        }
    }

    pub(crate) fn run_key_command(&mut self, command: KeyCommand, cx: &mut Context<Self>) {
        match command {
            KeyCommand::Vim(key) => {
                if !self.standard_type_over(key, cx) {
                    self.send_key(key, cx);
                }
            }
            KeyCommand::Move { key, select } => self.standard_move(Some(key), None, select, cx),
            KeyCommand::Home { select } => self.standard_move(None, Some(false), select, cx),
            KeyCommand::End { select } => self.standard_move(None, Some(true), select, cx),
            KeyCommand::Undo => self.run_action(VimIntent::Undo, cx),
            KeyCommand::Redo => self.run_action(VimIntent::Redo, cx),
            KeyCommand::Copy => self.copy_selection(cx),
            KeyCommand::Cut => self.cut_selection(cx),
            KeyCommand::Paste => self.paste_text(cx),
            KeyCommand::SelectAll => self.select_all(cx),
            KeyCommand::Save => self.save(cx),
            KeyCommand::CommandPalette => self.open_palette("", cx),
            KeyCommand::CollectionBrowser => crate::overlays::open_browser(self, cx),
            KeyCommand::History => crate::overlays::open_history(self, cx),
            KeyCommand::Find => {
                self.set_status("search is not in the desktop app yet");
                cx.notify();
            }
            KeyCommand::Bold => self.run_command("format bold", cx),
            KeyCommand::Italic => self.run_command("format italic", cx),
            KeyCommand::NewNote => crate::commands::new_note(self, cx),
            KeyCommand::ToggleSidebar => {
                self.sidebar = !self.sidebar;
                cx.notify();
            }
            KeyCommand::Quit => self.quit(cx),
            KeyCommand::Escape => {
                let before = self.snapshot_cursor();
                self.host.doc.selection_anchor = None;
                self.after_input(before, InputOutcome::default(), cx);
            }
            KeyCommand::Ignore => {}
        }
    }

    fn on_key(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if crate::overlays::on_key(self, ev, window, cx) {
            return;
        }
        self.status = None;
        let command = keys::map(&ev.keystroke, self.mode);
        self.run_key_command(command, cx);
    }

    pub(crate) fn open_palette(&mut self, initial: &str, cx: &mut Context<Self>) {
        crate::overlays::open_palette(self, initial, cx);
    }

    fn move_cursor_to(&mut self, line: usize, cx: &mut Context<Self>) {
        let before = self.snapshot_cursor();
        if self.mode == EditingMode::Standard {
            self.host.doc.selection_anchor = None;
        }
        self.host.set_cursor_line(line);
        self.after_input(before, InputOutcome::default(), cx);
    }

    /// The source column under `position` on line `ix`, from the last paint.
    fn column_at(&self, ix: usize, position: gpui::Point<gpui::Pixels>) -> Option<usize> {
        let layout = self.layouts.borrow().get(&ix)?.clone();
        let line = self.cache.get(ix)?.as_ref()?;
        let map = line.map.as_ref()?;
        // gpui panics for a layout that was measured but never painted.
        let index = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            layout.index_for_position(position)
        }))
        .ok()?;
        let byte = match index {
            Ok(b) | Err(b) => b,
        };
        let text: String = line.runs.iter().map(|r| r.text.as_str()).collect();
        let shown = text[..byte.min(text.len())].chars().count();
        let hit = map.display_to_source(line.display_skip + shown, Affinity::After)?;
        let len = self.host.doc.lines().get(ix)?.chars().count();
        Some(
            hit.caret
                .or(hit.owner.map(|r| r.start))
                .unwrap_or(len)
                .min(len),
        )
    }

    /// Button down on line `ix`: place the cursor at the character, or select
    /// a word (double click) or the line (triple click).
    fn mouse_down(
        &mut self,
        ix: usize,
        position: gpui::Point<gpui::Pixels>,
        clicks: usize,
        cx: &mut Context<Self>,
    ) {
        let before = self.snapshot_cursor();
        let text = self.host.doc.lines()[ix].clone();
        let len = text.chars().count();
        let col = self.column_at(ix, position).unwrap_or(0).min(len);
        let vim = self.mode == EditingMode::Vim;
        // A click leaves Visual mode and any selection.
        if matches!(
            self.host.input.mode(),
            VimMode::Visual | VimMode::VisualLine
        ) {
            self.host.input.vim = Default::default();
        }
        self.host.doc.selection_anchor = None;
        self.drag_anchor = None;
        match clicks {
            0 | 1 => {
                self.host.doc.cursor_line = ix;
                self.host.doc.cursor_col = col;
                self.drag_anchor = Some((ix, col));
            }
            2 => {
                let (from, to) = word_at(&text, col);
                self.host.doc.cursor_line = ix;
                if vim && self.host.input.mode() == VimMode::Normal {
                    // Visual includes the character under the cursor.
                    self.host.doc.selection_anchor = Some((ix, from));
                    self.host.doc.cursor_col = to.saturating_sub(1).max(from);
                    self.host.input.vim.mode = VimMode::Visual;
                } else {
                    self.host.doc.selection_anchor = Some((ix, from));
                    self.host.doc.cursor_col = to;
                }
            }
            _ => {
                self.host.doc.cursor_line = ix;
                if vim && self.host.input.mode() == VimMode::Normal {
                    self.host.doc.selection_anchor = Some((ix, 0));
                    self.host.doc.cursor_col = 0;
                    self.host.input.vim.mode = VimMode::VisualLine;
                } else {
                    self.host.doc.selection_anchor = Some((ix, 0));
                    self.host.doc.cursor_col = len;
                }
            }
        }
        self.after_input(before, InputOutcome::default(), cx);
    }

    /// The mouse moved over line `ix` with the button held: extend the
    /// selection to the character under it.
    fn mouse_drag(
        &mut self,
        ix: usize,
        position: gpui::Point<gpui::Pixels>,
        cx: &mut Context<Self>,
    ) {
        let Some(anchor) = self.drag_anchor else {
            return;
        };
        let len = self
            .host
            .doc
            .lines()
            .get(ix)
            .map_or(0, |l| l.chars().count());
        let col = self.column_at(ix, position).unwrap_or(0).min(len);
        if (ix, col) == (self.host.doc.cursor_line, self.host.doc.cursor_col) {
            return;
        }
        let before = self.snapshot_cursor();
        self.host.doc.selection_anchor = Some(anchor);
        self.host.doc.cursor_line = ix;
        self.host.doc.cursor_col = col;
        if self.mode == EditingMode::Vim && self.host.input.mode() == VimMode::Normal {
            self.host.input.vim.mode = VimMode::Visual;
        }
        self.after_input(before, InputOutcome::default(), cx);
    }

    fn line(&mut self, ix: usize) -> Option<LineView> {
        if ix >= self.cache.len() {
            return None;
        }
        if self.cache[ix].is_none() {
            self.cache[ix] = Some(self.host.line_view(ix, &self.fences[ix]));
        }
        self.cache[ix].clone()
    }

    fn render_line(
        &mut self,
        ix: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(line) = self.line(ix) else {
            return div().into_any_element();
        };
        let t = self.theme;
        let cursor = self.host.doc.cursor_line;
        let is_cursor = ix == cursor;
        let number = if is_cursor || self.mode == EditingMode::Standard {
            ix + 1
        } else {
            ix.abs_diff(cursor)
        };
        let shape = if self.host.input.mode() == VimMode::Insert {
            CursorShape::Bar
        } else {
            CursorShape::Block
        };
        let style = LineStyle {
            theme: &t,
            sans: &self.fonts.sans,
            cursor: shape,
            focused: self.focus.is_focused(window),
        };
        // The number sits beside the first row of the line, centred on it.
        let gutter_top = match &line.kind {
            LineKind::Heading(1) => 13.0,
            LineKind::Heading(2) => 7.6,
            LineKind::Heading(_) => 2.2,
            LineKind::TableRow { .. } => 4.0,
            _ => 0.0,
        };
        let gutter = div()
            .w(px(38.0))
            .flex_none()
            .pt(px(gutter_top))
            .pr(px(12.0))
            .flex()
            .justify_end()
            .text_size(px(12.0))
            .text_color(if is_cursor { t.text } else { t.faint })
            .child(number.to_string());
        let reminder = self
            .host
            .session
            .reminders()
            .get(&ix)
            .map(|r| r.display_at.clone());
        let delimiter = line.kind == LineKind::TableDelimiter;
        let table = matches!(line.kind, LineKind::TableRow { .. });
        let in_table = table || delimiter;
        let mut text_layout = None;
        let body = editor_lines::body(&line, &style, &mut text_layout);
        match text_layout {
            Some(layout) => {
                self.layouts.borrow_mut().insert(ix, layout);
            }
            None => {
                self.layouts.borrow_mut().remove(&ix);
            }
        }
        let hovered = self
            .hover_table
            .is_some_and(|(start, end)| (start..=end).contains(&ix));
        let table_last = in_table
            && !self
                .host
                .doc
                .lines()
                .get(ix + 1)
                .is_some_and(|l| table_syntax::is_table_line(l));
        let row = div()
            .id(("line", ix))
            .flex()
            .items_start()
            .w_full()
            .map(|d| {
                if delimiter {
                    d.h(px(0.0)).overflow_hidden()
                } else {
                    d.min_h(px(26.0))
                }
            })
            .when(is_cursor, |d| d.bg(t.cursorline))
            .when(line.line_selected, |d| d.bg(t.blue.opacity(0.22)))
            .on_hover(cx.listener(move |this, hovering: &bool, _, cx| {
                if !*hovering {
                    return;
                }
                let bounds = if in_table {
                    editor_core::table::table_block_bounds(this.host.doc.lines(), ix)
                } else {
                    None
                };
                if this.hover_table != bounds {
                    this.hover_table = bounds;
                    cx.notify();
                }
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, ev: &gpui::MouseDownEvent, window, cx| {
                    window.focus(&this.focus);
                    crate::overlays::close(this);
                    this.mouse_down(ix, ev.position, ev.click_count, cx);
                }),
            )
            .on_mouse_move(cx.listener(move |this, ev: &gpui::MouseMoveEvent, _, cx| {
                if ev.dragging() {
                    this.mouse_drag(ix, ev.position, cx);
                }
            }))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, ev: &gpui::MouseDownEvent, window, cx| {
                    window.focus(&this.focus);
                    this.move_cursor_to(ix, cx);
                    crate::overlays::open_context_menu(this, ev.position, table, cx);
                }),
            )
            .child(gutter)
            .child(div().flex_1().min_w_0().child(body))
            .when(table, |d| {
                // Space for the add-column bar is always reserved, so
                // hovering never moves the table.
                d.child(
                    div()
                        .id(("add-column", ix))
                        .w(px(18.0))
                        .h(px(22.0))
                        .flex_none()
                        .ml(px(4.0))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(4.0))
                        .cursor_pointer()
                        .when(hovered, |d| d.bg(t.chip).text_color(t.muted).child("+"))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.move_cursor_to(ix, cx);
                            crate::commands::table_insert_column(this, cx);
                        })),
                )
            })
            .when_some(reminder, |d, at| {
                d.child(
                    div()
                        .ml(px(12.0))
                        .mt(px(4.0))
                        .px(px(8.0))
                        .rounded(px(9.0))
                        .bg(t.chip)
                        .text_size(px(11.0))
                        .text_color(t.muted)
                        .font_family(self.fonts.sans.clone())
                        .child(format!("⏰ {at}")),
                )
            });
        div()
            .w_full()
            .child(row)
            .when(table_last, |d| {
                d.child(
                    div()
                        .id(("add-row", ix))
                        .h(px(18.0))
                        .mt(px(2.0))
                        .ml(px(38.0))
                        .mr(px(22.0))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(4.0))
                        .cursor_pointer()
                        .when(hovered, |d| d.bg(t.chip).text_color(t.muted).child("+"))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.move_cursor_to(ix, cx);
                            crate::commands::table_insert_row(this, false, cx);
                        })),
                )
            })
            .into_any_element()
    }

    fn title_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        div()
            .h(px(38.0))
            .flex_none()
            .flex()
            .items_center()
            .gap(px(12.0))
            .px(px(14.0))
            .bg(t.panel)
            .border_b_1()
            .border_color(t.border)
            .child(
                div()
                    .size(px(14.0))
                    .rounded(px(3.0))
                    .border_2()
                    .border_color(t.blue),
            )
            .child(
                div()
                    .flex()
                    .gap(px(2.0))
                    .children(MENUS.iter().map(|label| {
                        let label = *label;
                        let open = crate::overlays::menu_open(self, label);
                        div()
                            .id(SharedString::from(format!("menu-{label}")))
                            .px(px(9.0))
                            .py(px(4.0))
                            .rounded(px(5.0))
                            .text_size(px(12.5))
                            .text_color(if open { t.heading } else { t.muted })
                            .when(open, |d| d.bg(t.active))
                            .hover(|s| s.bg(t.active))
                            .cursor_pointer()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                crate::overlays::toggle_menu(this, label, cx)
                            }))
                            .child(label)
                    })),
            )
            .child(div().w(px(1.0)).h(px(16.0)).bg(t.border))
            .child(
                div()
                    .flex()
                    .gap(px(6.0))
                    .text_size(px(12.5))
                    .text_color(t.muted)
                    .child("Notes")
                    .child(div().text_color(t.faint).child("/"))
                    .child(
                        div()
                            .text_color(t.text)
                            .font_weight(FontWeight::MEDIUM)
                            .child(self.host.title.clone()),
                    ),
            )
    }

    fn sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        let current = self.host.note_id().to_string();
        let items = self.host.notes.iter().take(SIDEBAR_NOTES).map(|note| {
            let active = note.id == current;
            let id = note.id.clone();
            div()
                .id(SharedString::from(format!("note-{}", note.id)))
                .px(px(10.0))
                .py(px(6.0))
                .rounded(px(6.0))
                .cursor_pointer()
                .when(active, |d| d.bg(t.active))
                .hover(|s| s.bg(t.active))
                .on_click(cx.listener(move |this, _, _, cx| this.open_note(&id, cx)))
                .child(
                    div()
                        .text_size(px(13.0))
                        .text_color(if active { t.heading } else { t.text })
                        .when(active, |d| d.font_weight(FontWeight::SEMIBOLD))
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .child(if note.title.is_empty() {
                            "Untitled".to_string()
                        } else {
                            note.title.clone()
                        }),
                )
        });
        div()
            .id("sidebar")
            .w(px(248.0))
            .flex_none()
            .h_full()
            .overflow_y_scroll()
            .bg(t.panel)
            .border_r_1()
            .border_color(t.border)
            .p(px(10.0))
            .flex()
            .flex_col()
            .gap(px(2.0))
            .child(
                div()
                    .flex()
                    .justify_between()
                    .px(px(8.0))
                    .pb(px(8.0))
                    .text_size(px(11.0))
                    .text_color(t.muted)
                    .child("NOTES")
                    .child(
                        div()
                            .text_color(t.faint)
                            .child(format!("{}", self.host.notes.len())),
                    ),
            )
            .children(items)
    }

    fn status_bar(&self) -> impl IntoElement {
        let t = &self.theme;
        let m = self.host.modules;
        let (width, _) = self.viewport.get();
        let line = self.host.doc.cursor_line;
        let ghost = self
            .cache
            .get(line)
            .and_then(|l| l.as_ref())
            .and_then(|l| l.ghost.as_deref())
            .map(|g| g.trim().trim_start_matches(['=', '→']).trim().to_string());
        // Every segment stays on one line; the title and message give way first.
        let chip = |label: &'static str| {
            div()
                .flex_none()
                .whitespace_nowrap()
                .px(px(7.0))
                .rounded(px(9.0))
                .bg(t.chip)
                .child(label)
        };
        let (pill, pill_bg) = match self.host.input.mode() {
            VimMode::Normal => ("NORMAL", t.blue),
            VimMode::Insert => ("INSERT", t.amber),
            VimMode::Visual => ("VISUAL", t.muted),
            VimMode::VisualLine => ("V-LINE", t.muted),
        };
        let vim = self.mode == EditingMode::Vim;
        let pending = self.host.input.pending_keys();
        let position = if vim {
            format!("{}:{}", line + 1, self.host.doc.cursor_col + 1)
        } else {
            format!("Ln {}, Col {}", line + 1, self.host.doc.cursor_col + 1)
        };
        let wide = width >= 900.0;
        let divider = || {
            div()
                .flex_none()
                .whitespace_nowrap()
                .px(px(10.0))
                .border_l_1()
                .border_color(t.border)
        };
        div()
            .h(px(28.0))
            .w_full()
            .flex_none()
            .overflow_hidden()
            .flex()
            .items_center()
            .gap(px(6.0))
            .px(px(6.0))
            .bg(t.panel)
            .border_t_1()
            .border_color(t.border)
            .text_size(px(11.5))
            .text_color(t.muted)
            .when(vim, |d| {
                d.child(
                    div()
                        .flex_none()
                        .whitespace_nowrap()
                        .px(px(9.0))
                        .rounded(px(4.0))
                        .bg(pill_bg)
                        .text_color(t.on_accent)
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(pill),
                )
            })
            .child(
                div()
                    .flex()
                    .items_center()
                    .min_w_0()
                    .max_w(px(260.0))
                    .px(px(6.0))
                    .text_color(t.text)
                    .child(
                        div()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .child(self.host.title.clone()),
                    )
                    .when(self.host.session.dirty(), |d| {
                        d.child(div().flex_none().pl(px(4.0)).text_color(t.amber).child("●"))
                    }),
            )
            .when(wide && m.math, |d| d.child(chip("math")))
            .when(wide && m.variables, |d| d.child(chip("variables")))
            .when(wide && m.table, |d| d.child(chip("table")))
            .when(!pending.is_empty(), |d| {
                d.child(
                    div()
                        .flex_none()
                        .whitespace_nowrap()
                        .px(px(8.0))
                        .font_family(self.fonts.mono.clone())
                        .text_color(t.faint)
                        .child(pending),
                )
            })
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .px(px(8.0))
                    .text_color(t.text)
                    .children(self.status.clone()),
            )
            .when_some(ghost, |d, g| {
                d.child(
                    div()
                        .flex_none()
                        .whitespace_nowrap()
                        .px(px(10.0))
                        .font_family(self.fonts.mono.clone())
                        .text_color(t.amber)
                        .child(format!("= {g}")),
                )
            })
            .when(wide, |d| {
                d.child(divider().child(if vim { "Vim" } else { "Standard" }))
            })
            .child(
                divider()
                    .font_family(self.fonts.mono.clone())
                    .child(position),
            )
    }
}

impl SlateWindow {
    /// Stand-in for the text of an encrypted note that is not unlocked.
    fn locked_view(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        div()
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(12.0))
            .font_family(self.fonts.sans.clone())
            .child(div().text_size(px(34.0)).child("🔒"))
            .child(
                div()
                    .text_size(px(16.0))
                    .text_color(t.heading)
                    .child("This note is encrypted"),
            )
            .child(
                div()
                    .id("unlock")
                    .h(px(32.0))
                    .px(px(16.0))
                    .flex()
                    .items_center()
                    .rounded(px(6.0))
                    .cursor_pointer()
                    .bg(t.blue)
                    .text_color(t.on_accent)
                    .font_weight(FontWeight::SEMIBOLD)
                    .on_click(cx.listener(|this, _, _, cx| {
                        crate::overlays::open_prompt(
                            this,
                            crate::overlays::PromptKind::Unlock,
                            "",
                            cx,
                        )
                    }))
                    .child("Unlock"),
            )
    }
}

impl Render for SlateWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        let size = window.viewport_size();
        self.viewport
            .set((f32::from(size.width), f32::from(size.height)));
        let editor = div()
            .flex_1()
            .min_w_0()
            .h_full()
            .pt(px(20.0))
            .font_family(self.fonts.mono.clone())
            .text_size(px(14.0))
            .line_height(px(26.0))
            .text_color(t.text)
            .map(|d| {
                if self.host.locked() {
                    return d.child(self.locked_view(cx));
                }
                d.child(
                    list(
                        self.list.clone(),
                        cx.processor(|this, ix, window, cx| this.render_line(ix, window, cx)),
                    )
                    .size_full(),
                )
            });
        div()
            .id("slate")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::on_key))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, _| this.drag_anchor = None),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _, _, _| this.drag_anchor = None),
            )
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .bg(t.bg)
            .text_color(t.text)
            .font_family(self.fonts.sans.clone())
            .text_size(px(13.0))
            .child(self.title_bar(cx))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .when(self.sidebar, |d| d.child(self.sidebar(cx)))
                    .child(editor),
            )
            .children(crate::overlays::which_key(self))
            .child(self.status_bar())
            .children(crate::overlays::render(self, window, cx))
    }
}

#[cfg(test)]
mod tests {
    use super::word_at;

    #[test]
    fn double_click_selects_words_and_symbol_runs() {
        assert_eq!(word_at("let total_cost = 5;", 6), (4, 14));
        assert_eq!(word_at("let total_cost = 5;", 15), (15, 16));
        assert_eq!(word_at("let total_cost = 5;", 14), (14, 15));
        assert_eq!(word_at("a := b", 3), (2, 4));
        assert_eq!(word_at("héllo wörld", 8), (6, 11));
        assert_eq!(word_at("", 0), (0, 0));
        assert_eq!(word_at("end", 99), (0, 3));
    }
}
