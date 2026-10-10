//! The main window: title bar, notes sidebar, editor, status bar and the
//! overlays drawn over them.
//!
//! Painting and input routing only. Line content comes from `NoteHost`, key
//! semantics from `note_session::input`, commands from `editor_core`.
use crate::completion::{self, Completion, Picked};
use crate::editor_lines::{self, CursorShape, LineStyle};
use crate::images::{self, ImageSlot};
use crate::keys::{self, EditingMode, KeyCommand};
use crate::note_view::{CommandRun, LineKind, LineView, NoteHost};
use crate::settings::{CommandBarStyle, Settings};
use crate::sidebar_search::{self, Hit};
use crate::theme::{Theme, ThemeMode};
use editor_core::markdown_tokens::FenceState;
use editor_core::vim::{VimAction, VimIntent, VimKey, VimMode, VimPending};
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
type CellBounds = std::rc::Rc<std::cell::RefCell<Vec<gpui::Bounds<gpui::Pixels>>>>;

/// The sidebar's search box while it has the keyboard.
#[derive(Default)]
pub(crate) struct SidebarSearch {
    query: String,
    hits: Vec<Hit>,
    selected: usize,
}

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
    pub(crate) theme_mode: ThemeMode,
    theme_config: app_core::config::ThemeConfig,
    pub(crate) command_bar: CommandBarStyle,
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
    /// Painted cell bounds of table rows, by line.
    cell_bounds: std::cell::RefCell<std::collections::HashMap<usize, CellBounds>>,
    /// Where a mouse drag started; set while the left button is held.
    drag_anchor: Option<(usize, usize)>,
    /// Window size in pixels from the last paint, to reveal far jumps and fit the status bar.
    pub(crate) viewport: std::cell::Cell<(f32, f32)>,
    fences: Vec<FenceState>,
    /// Autocomplete popup, with the cursor it was computed for.
    pub(crate) currency: crate::currency::Currency,
    sidebar_search: Option<SidebarSearch>,
    completion: Option<Completion>,
    completion_pos: (usize, usize),
    completion_min: usize,
    /// The OS window is fullscreen because of preview mode.
    fullscreen: bool,
    scroll_drag: bool,
    images: std::cell::RefCell<std::collections::HashMap<String, ImageSlot>>,
    cache: Vec<Option<LineView>>,
    list: ListState,
}

impl SlateWindow {
    pub fn new(
        host: NoteHost,
        settings: Settings,
        fonts: Fonts,
        currency: crate::currency::Currency,
        currency_problem: Option<String>,
        cx: &mut Context<Self>,
    ) -> Self {
        let theme_config = app_core::config::load_theme_config();
        let theme_mode = settings.theme;
        let count = host.doc.lines().len();
        let mut this = Self {
            host,
            theme: Theme::for_mode(theme_mode, &theme_config),
            theme_mode,
            theme_config,
            command_bar: settings.command_bar,
            fonts,
            focus: cx.focus_handle(),
            mode: if settings.vim {
                EditingMode::Vim
            } else {
                EditingMode::Standard
            },
            sidebar: settings.sidebar,
            status: currency_problem,
            overlay: Default::default(),
            hover_table: None,
            layouts: Default::default(),
            cell_bounds: Default::default(),
            drag_anchor: None,
            viewport: std::cell::Cell::new((1280.0, 800.0)),
            fences: Vec::new(),
            currency,
            sidebar_search: None,
            completion: None,
            completion_pos: (0, 0),
            completion_min: usize::from(
                app_core::config::load_theme_config().variables_autocomplete_min_chars,
            )
            .clamp(1, 8),
            fullscreen: false,
            scroll_drag: false,
            images: Default::default(),
            cache: Vec::new(),
            list: ListState::new(count, ListAlignment::Top, px(600.0)),
        };
        this.host.insert_only = this.mode == EditingMode::Standard;
        if this.host.insert_only {
            this.host.input.vim.mode = VimMode::Insert;
        }
        this.reload_lines();
        cx.spawn(async move |this, cx| loop {
            cx.background_executor()
                .timer(Duration::from_millis(500))
                .await;
            let alive = this.update(cx, |this, cx| {
                if this.host.autosave_due(AUTOSAVE_DELAY) {
                    this.save(cx);
                }
                this.apply_currency(cx);
            });
            if alive.is_err() {
                break;
            }
        })
        .detach();
        this
    }

    /// Apply a finished rates refresh: new rates change every conversion.
    pub(crate) fn apply_currency(&mut self, cx: &mut Context<Self>) {
        let Some((status, changed)) = self.currency.poll() else {
            return;
        };
        if changed {
            self.host.refresh_calc_after_rates();
            self.restyle();
        }
        self.set_status(status);
        cx.notify();
    }

    pub fn focus_handle(&self) -> &FocusHandle {
        &self.focus
    }

    /// The note was replaced: rebuild every line and start at the top.
    pub(crate) fn reload_lines(&mut self) {
        self.fences = self.host.fence_starts();
        let count = self.fences.len();
        self.cache = vec![None; count];
        self.images.borrow_mut().clear();
        self.completion = None;
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
        self.host.insert_only = mode == EditingMode::Standard;
        self.host.input = Default::default();
        if mode == EditingMode::Standard {
            // Standard editing types directly: the session stays in insert.
            self.host.input.vim.mode = VimMode::Insert;
        }
        self.host.doc.selection_anchor = None;
        self.restyle();
        self.persist();
        self.set_status(match mode {
            EditingMode::Vim => "vim editing",
            EditingMode::Standard => "standard editing",
        });
        cx.notify();
    }

    /// Remember the preferences that survive a restart.
    pub(crate) fn persist(&self) {
        Settings {
            command_bar: self.command_bar,
            vim: self.mode == EditingMode::Vim,
            theme: self.theme_mode,
            sidebar: self.sidebar,
        }
        .save();
    }

    pub(crate) fn set_command_bar(&mut self, style: CommandBarStyle, cx: &mut Context<Self>) {
        self.command_bar = style;
        self.persist();
        self.set_status(match style {
            CommandBarStyle::Popup => "command line: popup",
            CommandBarStyle::Bottom => "command line: bottom of the window",
        });
        cx.notify();
    }

    pub(crate) fn set_theme(&mut self, mode: ThemeMode, cx: &mut Context<Self>) {
        self.theme_mode = mode;
        self.theme = Theme::for_mode(mode, &self.theme_config);
        self.restyle();
        self.persist();
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
        self.update_completion(outcome.text_changed);
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

    fn cursor_pos(&self) -> (usize, usize) {
        (self.host.doc.cursor_line, self.host.doc.cursor_col)
    }

    /// Keep the autocomplete popup in step with typing: recompute it after
    /// an edit, close it when the cursor moved away or insert mode ended.
    fn update_completion(&mut self, text_changed: bool) {
        if self.host.input.mode() != VimMode::Insert || self.host.preview {
            self.completion = None;
            return;
        }
        if !text_changed {
            if self.cursor_pos() != self.completion_pos {
                self.completion = None;
            }
            return;
        }
        self.completion = match self.completion.take() {
            Some(mut open) if open.is_wiki() => {
                completion::refresh(&self.host, &mut open).then_some(open)
            }
            previous => completion::variable(&self.host, self.completion_min, previous.as_ref()),
        };
        self.completion_pos = self.cursor_pos();
    }

    /// `Esc` on an open popup: close it, undoing a half-made heading link.
    fn dismiss_completion(&mut self, cx: &mut Context<Self>) {
        let Some(open) = self.completion.take() else {
            return;
        };
        let before = self.snapshot_cursor();
        if let Some(outcome) = completion::cancel(&mut self.host, &open) {
            self.after_input(before, outcome, cx);
        }
        cx.notify();
    }

    fn accept_completion(&mut self, cx: &mut Context<Self>) {
        let Some(mut open) = self.completion.take() else {
            return;
        };
        let before = self.snapshot_cursor();
        match completion::accept(&mut self.host, &mut open) {
            Some(Picked::Closed(outcome)) => self.after_input(before, outcome, cx),
            Some(Picked::Headings(outcome)) => {
                self.completion = Some(open);
                self.after_input(before, outcome, cx);
            }
            None => {}
        }
        cx.notify();
    }

    /// Keys the autocomplete popup owns while typing; `true` when handled.
    fn completion_key(&mut self, ev: &KeyDownEvent, cx: &mut Context<Self>) -> bool {
        let m = ev.keystroke.modifiers;
        if self.host.preview
            || self.host.locked()
            || self.host.input.mode() != VimMode::Insert
            || m.control
            || m.alt
            || m.platform
        {
            return false;
        }
        // A popup with nothing to show does not take keys (Esc still closes it).
        let exists = self.completion.is_some();
        let open = self
            .completion
            .as_ref()
            .is_some_and(|c| !c.items.is_empty());
        match ev.keystroke.key.as_str() {
            "down" | "up" if open && !m.shift => {
                let delta = if ev.keystroke.key == "down" { 1 } else { -1 };
                if let Some(c) = &mut self.completion {
                    c.step(delta);
                }
                cx.notify();
                true
            }
            "escape" if exists => {
                self.dismiss_completion(cx);
                true
            }
            "enter" if open && !m.shift => {
                self.accept_completion(cx);
                true
            }
            "tab" if !m.shift => {
                if !open {
                    self.completion = completion::variable(&self.host, self.completion_min, None);
                }
                if self.completion.is_some() {
                    self.accept_completion(cx);
                    true
                } else {
                    false
                }
            }
            _ => false,
        }
    }

    fn handle_request(&mut self, request: HostRequest, cx: &mut Context<Self>) {
        match request {
            HostRequest::CopyToClipboard(text) => {
                cx.write_to_clipboard(ClipboardItem::new_string(text));
                self.set_status("yanked to clipboard");
            }
            HostRequest::PasteFromClipboard(action) => self.paste_clipboard(&action, cx),
            HostRequest::OpenWikiCompletion => {
                self.completion = Some(completion::open_wiki(&self.host));
                self.completion_pos = self.cursor_pos();
            }
            HostRequest::OpenCommandBar => crate::overlays::open_command_bar(self, ":", cx),
            HostRequest::OpenSearch => self.set_status("search is not in the desktop app yet"),
            HostRequest::SearchNext | HostRequest::SearchPrev => {}
            HostRequest::Unsupported(intent) => {
                self.set_status(format!("{intent:?} is not in the desktop app yet"))
            }
        }
    }

    /// Import a clipboard image into the note and link it on its own line.
    /// Returns false when the clipboard holds no image.
    pub(crate) fn paste_image(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(image) = cx.read_from_clipboard().and_then(|item| {
            item.into_entries().find_map(|e| match e {
                gpui::ClipboardEntry::Image(i) => Some(i),
                _ => None,
            })
        }) else {
            return false;
        };
        let sources = app_core::note_sources::NoteSourceService::new(self.host.db.clone());
        let note_id = self.host.session.note_id().to_string();
        match sources.import_image_bytes_by_id(
            &note_id,
            None,
            Some(image.format.mime_type()),
            &image.bytes,
        ) {
            Ok(imported) => {
                let before = self.snapshot_cursor();
                let outcome = self
                    .host
                    .insert_text(&format!("\n![Image]({})\n", imported.markdown_path));
                self.after_input(before, outcome, cx);
                self.set_status("pasted image");
            }
            Err(e) => self.set_status(format!("paste image failed: {e}")),
        }
        true
    }

    fn paste_clipboard(&mut self, action: &VimAction, cx: &mut Context<Self>) {
        if self.paste_image(cx) {
            return;
        }
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
        // `gd` follows a wiki link or goes to a variable's definition, which
        // the vim engine leaves to the host.
        if key == VimKey::Char('d') && self.host.input.vim.pending == Some(VimPending::Go) {
            self.host.input.vim.pending = None;
            crate::commands::go_to_definition(self, cx);
            return;
        }
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
        if self.paste_image(cx) {
            return;
        }
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

    /// Save before the window closes; `false` keeps it open.
    pub(crate) fn save_for_close(&mut self, cx: &mut Context<Self>) -> bool {
        match self.host.save() {
            Ok(_) => true,
            Err(err) => {
                self.set_status(format!("not closing, save failed: {err}"));
                cx.notify();
                false
            }
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

    /// `Ctrl+Left`/`Ctrl+Right`. Vim Normal and Visual use `b` and `w`.
    fn word_move(&mut self, forward: bool, select: bool, cx: &mut Context<Self>) {
        if self.host.input.mode() != VimMode::Insert {
            self.send_key(VimKey::Char(if forward { 'w' } else { 'b' }), cx);
            return;
        }
        let before = self.snapshot_cursor();
        let doc = &mut self.host.doc;
        if select && doc.selection_anchor.is_none() {
            doc.selection_anchor = Some((doc.cursor_line, doc.cursor_col));
        }
        if !select {
            doc.selection_anchor = None;
        }
        self.host.move_word(forward);
        self.after_input(before, InputOutcome::default(), cx);
    }

    /// `Ctrl+Backspace` and `Ctrl+Delete` in insert and standard editing.
    fn delete_word(&mut self, forward: bool, cx: &mut Context<Self>) {
        if self.host.input.mode() != VimMode::Insert {
            return;
        }
        let before = self.snapshot_cursor();
        if self.host.doc.selection_anchor.is_some() {
            if let Some((_, outcome)) = self.host.cut_selection() {
                self.after_input(before, outcome, cx);
            }
            return;
        }
        let outcome = if forward {
            let doc = &mut self.host.doc;
            doc.selection_anchor = Some((doc.cursor_line, doc.cursor_col));
            self.host.move_word(true);
            match self.host.cut_selection() {
                Some((_, outcome)) => outcome,
                None => {
                    self.host.doc.selection_anchor = None;
                    InputOutcome::default()
                }
            }
        } else {
            self.host.delete_word_backward()
        };
        self.after_input(before, outcome, cx);
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
            KeyCommand::Word { forward, select } => self.word_move(forward, select, cx),
            KeyCommand::DeleteWord { forward } => self.delete_word(forward, cx),
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
            KeyCommand::CommandBar => crate::overlays::open_command_bar(self, "", cx),
            KeyCommand::NoteSwitcher => crate::switcher::open_switcher(self, cx),
            KeyCommand::CollectionPicker => crate::switcher::open_picker(self, cx),
            KeyCommand::FollowLink => crate::commands::follow_link(self, cx),
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
                self.persist();
                cx.notify();
            }
            KeyCommand::Preview => self.toggle_preview(cx),
            KeyCommand::Quit => self.quit(cx),
            KeyCommand::Escape if self.host.preview => self.toggle_preview(cx),
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
        let k = &ev.keystroke;
        if k.modifiers.control && k.modifiers.shift && k.key == "f" {
            self.sidebar = true;
            self.sidebar_search = Some(SidebarSearch::default());
            cx.notify();
            return;
        }
        if self.sidebar_search.is_some() && self.sidebar_search_key(ev, cx) {
            return;
        }
        if self.host.preview {
            self.preview_key(ev, cx);
            return;
        }
        if self.completion_key(ev, cx) {
            return;
        }
        let command = keys::map(&ev.keystroke, self.mode);
        self.run_key_command(command, cx);
        if command == KeyCommand::Vim(VimKey::Char('#'))
            && self.host.input.mode() == VimMode::Insert
        {
            if let Some(open) = completion::open_wiki_at_cursor(&self.host) {
                self.completion = Some(open);
                self.completion_pos = self.cursor_pos();
                cx.notify();
            }
        }
    }

    /// Keys while the sidebar search has focus; `true` when handled.
    /// Shortcuts with `Ctrl` still reach the app.
    fn sidebar_search_key(&mut self, ev: &KeyDownEvent, cx: &mut Context<Self>) -> bool {
        let k = &ev.keystroke;
        if k.modifiers.control || k.modifiers.platform {
            return false;
        }
        let Some(search) = &mut self.sidebar_search else {
            return false;
        };
        match k.key.as_str() {
            "escape" => self.sidebar_search = None,
            "enter" => {
                let id = search.hits.get(search.selected).map(|h| h.id.clone());
                self.sidebar_search = None;
                if let Some(id) = id {
                    self.open_note(&id, cx);
                }
            }
            "down" | "up" => {
                let n = search.hits.len();
                if n > 0 {
                    let step = if k.key == "down" { 1 } else { n - 1 };
                    search.selected = (search.selected + step) % n;
                }
            }
            "backspace" => {
                search.query.pop();
                search.hits = sidebar_search::search(&self.host, &search.query);
                search.selected = 0;
            }
            _ => {
                if let Some(ch) = k
                    .key_char
                    .as_deref()
                    .filter(|c| !c.chars().any(char::is_control))
                {
                    search.query.push_str(ch);
                    search.hits = sidebar_search::search(&self.host, &search.query);
                    search.selected = 0;
                }
            }
        }
        cx.notify();
        true
    }

    /// Preview is read-only: only leaving it, quitting and scrolling work.
    fn preview_key(&mut self, ev: &KeyDownEvent, cx: &mut Context<Self>) {
        let page = f32::from(self.list.viewport_bounds().size.height) * 0.9;
        let by = |px_: f32| gpui::px(px_);
        match ev.keystroke.key.as_str() {
            "escape" | "f11" | "q" => self.toggle_preview(cx),
            "down" | "j" => self.list.scroll_by(by(48.0)),
            "up" | "k" => self.list.scroll_by(by(-48.0)),
            "pagedown" | "space" => self.list.scroll_by(by(page)),
            "pageup" => self.list.scroll_by(by(-page)),
            "home" | "g" => self.list.scroll_to(gpui::ListOffset {
                item_ix: 0,
                offset_in_item: gpui::px(0.0),
            }),
            "end" => self.list.scroll_to(gpui::ListOffset {
                item_ix: self.cache.len().saturating_sub(1),
                offset_in_item: gpui::px(0.0),
            }),
            _ => {
                if matches!(keys::map(&ev.keystroke, self.mode), KeyCommand::Quit) {
                    self.quit(cx);
                }
            }
        }
        cx.notify();
    }

    /// Enter or leave the distraction-free preview. Heights change (images
    /// and rendered links replace the cursor line), so measure again but
    /// keep the scroll position.
    pub(crate) fn toggle_preview(&mut self, cx: &mut Context<Self>) {
        if self.host.locked() {
            return;
        }
        let top = self.list.logical_scroll_top();
        self.host.preview = !self.host.preview;
        self.restyle();
        self.list.reset(self.cache.len());
        self.list.scroll_to(top);
        self.drag_anchor = None;
        cx.notify();
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
        let line = self.cache.get(ix)?.as_ref()?;
        if let LineKind::TableRow { cursor, .. } = &line.kind {
            return self.table_column_at(ix, cursor.as_ref().map(|c| c.cell), position);
        }
        let layout = self.layouts.borrow().get(&ix)?.clone();
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

    /// Source column for a click in table row `ix`: inside the cell that was
    /// hit, at the clicked character in the cell being edited and at the end
    /// of its content in the others.
    fn table_column_at(
        &self,
        ix: usize,
        active: Option<usize>,
        position: gpui::Point<gpui::Pixels>,
    ) -> Option<usize> {
        let bounds = self.cell_bounds.borrow().get(&ix)?.borrow().clone();
        let cell = bounds
            .iter()
            .position(|b| position.x < b.right())
            .unwrap_or(bounds.len().checked_sub(1)?);
        let (start, end) = self.host.table_cell_span(ix, cell)?;
        if active != Some(cell) {
            return Some(end);
        }
        let layout = self.layouts.borrow().get(&ix)?.clone();
        let index = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            layout.index_for_position(position)
        }))
        .ok()?;
        let byte = match index {
            Ok(b) | Err(b) => b,
        };
        let text = self.cache.get(ix)?.as_ref().and_then(|l| match &l.kind {
            LineKind::TableRow { cells, .. } => cells.get(cell).map(|c| c.text.clone()),
            _ => None,
        })?;
        let chars = text[..byte.min(text.len())].chars().count();
        Some((start + chars).min(end))
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
        if self.host.preview {
            return;
        }
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

    /// The cached image for `src`; the first request decodes it in the
    /// background and repaints when done.
    fn image_slot(&self, src: &str, cx: &mut Context<Self>) -> ImageSlot {
        if let Some(slot) = self.images.borrow().get(src) {
            return slot.clone();
        }
        self.images
            .borrow_mut()
            .insert(src.to_string(), ImageSlot::Loading);
        let db = self.host.db.clone();
        let note_id = self.host.session.note_id().to_string();
        let key = src.to_string();
        cx.spawn(async move |this, cx| {
            let loaded = {
                let key = key.clone();
                cx.background_executor()
                    .spawn(async move { images::load(db, &note_id, &key) })
                    .await
            };
            let slot = loaded.unwrap_or_else(ImageSlot::Failed);
            this.update(cx, |this, cx| {
                let key_for_match = key.clone();
                this.images.borrow_mut().insert(key, slot);
                // The row height changes once the image is known.
                for ix in 0..this.cache.len() {
                    let hit = matches!(
                        &this.cache[ix],
                        Some(l) if matches!(&l.kind, LineKind::Image { src, .. } if *src == key_for_match)
                            || l.below.as_deref() == Some(key_for_match.as_str())
                    );
                    if hit {
                        this.cache[ix] = None;
                        this.list.splice(ix..ix + 1, 1);
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        ImageSlot::Loading
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
        let image = match (&line.kind, &line.below) {
            (LineKind::Image { src, .. }, _) | (_, Some(src)) => Some(self.image_slot(src, cx)),
            _ => None,
        };
        let preview = self.host.preview;
        let cells: Option<CellBounds> = matches!(line.kind, LineKind::TableRow { .. })
            .then(|| self.cell_bounds.borrow_mut().entry(ix).or_default().clone());
        let style = LineStyle {
            cells: cells.as_ref(),
            image: image.as_ref(),
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
            .when(is_cursor, |d| {
                d.text_color(t.blue).font_weight(FontWeight::SEMIBOLD)
            })
            .when(!is_cursor, |d| d.text_color(t.faint))
            .child(number.to_string());
        let reminder = self
            .host
            .session
            .reminders()
            .get(&ix)
            .map(|r| r.display_at.clone());
        // Like a calc result: shown after the text, in the same colour, and
        // wrapped with the line instead of hanging off its edge. Table rows
        // have no room after their cells and keep a chip.
        let line = if let (Some(at), false) =
            (&reminder, matches!(line.kind, LineKind::TableRow { .. }))
        {
            let mut line = line;
            let ghost = match line.ghost.take() {
                Some(g) => format!("{g}   ⏰ {at}"),
                None => format!("⏰ {at}"),
            };
            line.ghost = Some(ghost);
            line
        } else {
            line
        };
        let reminder = reminder.filter(|_| matches!(line.kind, LineKind::TableRow { .. }));
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
                    this.sidebar_search = None;
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
            .when(!preview, |d| d.child(gutter))
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
                    .id("sidebar-toggle")
                    .size(px(18.0))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .rounded(px(4.0))
                    .hover(|s| s.bg(t.active))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.run_key_command(KeyCommand::ToggleSidebar, cx)
                    }))
                    .child(
                        div()
                            .size(px(14.0))
                            .rounded(px(3.0))
                            .border_2()
                            .border_color(t.blue)
                            .flex()
                            .when(self.sidebar, |d| {
                                d.child(div().w(px(4.0)).h_full().bg(t.blue.opacity(0.6)))
                            }),
                    ),
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
        let searching = self
            .sidebar_search
            .as_ref()
            .filter(|s| !s.query.trim().is_empty());
        let hit_rows: Vec<AnyElement> = searching
            .map(|s| {
                s.hits
                    .iter()
                    .enumerate()
                    .map(|(i, hit)| {
                        let id = hit.id.clone();
                        let on = i == s.selected;
                        div()
                            .id(("hit", i))
                            .px(px(10.0))
                            .py(px(6.0))
                            .rounded(px(6.0))
                            .cursor_pointer()
                            .when(on, |d| d.bg(t.active))
                            .hover(|s| s.bg(t.active))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.sidebar_search = None;
                                this.open_note(&id, cx)
                            }))
                            .child(
                                div()
                                    .text_size(px(13.0))
                                    .text_color(if on { t.heading } else { t.text })
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .child(hit.title.clone()),
                            )
                            .when_some(hit.snippet.clone(), |d, snippet| {
                                d.child(
                                    div()
                                        .text_size(px(11.0))
                                        .text_color(t.faint)
                                        .overflow_hidden()
                                        .whitespace_nowrap()
                                        .text_ellipsis()
                                        .child(snippet),
                                )
                            })
                            .into_any_element()
                    })
                    .collect()
            })
            .unwrap_or_default();
        let active = self.sidebar_search.is_some();
        let query = self
            .sidebar_search
            .as_ref()
            .map(|s| s.query.clone())
            .unwrap_or_default();
        let search_box = div()
            .id("sidebar-search")
            .mx(px(2.0))
            .mb(px(8.0))
            .px(px(8.0))
            .py(px(5.0))
            .rounded(px(6.0))
            .bg(t.bg)
            .border_1()
            .border_color(if active { t.blue } else { t.border })
            .text_size(px(12.5))
            .cursor_text()
            .overflow_hidden()
            .whitespace_nowrap()
            .on_click(cx.listener(|this, _, _, cx| {
                this.sidebar_search.get_or_insert_with(Default::default);
                cx.notify();
            }))
            .child(if query.is_empty() {
                div()
                    .text_color(t.faint)
                    .child(if active {
                        "Type to search…"
                    } else {
                        "Search notes  (Ctrl+Shift+F)"
                    })
                    .into_any_element()
            } else {
                div()
                    .text_color(t.text)
                    .child(format!("{query}{}", if active { "▏" } else { "" }))
                    .into_any_element()
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
                    .child(
                        div()
                            .id("sidebar-title")
                            .cursor_pointer()
                            .hover(|s| s.text_color(t.text))
                            .on_click(
                                cx.listener(|this, _, _, cx| {
                                    crate::switcher::open_picker(this, cx)
                                }),
                            )
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .child(
                                self.host
                                    .working
                                    .as_ref()
                                    .map(|(_, name)| name.to_uppercase())
                                    .unwrap_or_else(|| "NOTES".to_string()),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .gap(px(8.0))
                            .child(
                                div()
                                    .text_color(t.faint)
                                    .child(format!("{}", self.host.notes.len())),
                            )
                            .child(
                                div()
                                    .id("sidebar-collapse")
                                    .cursor_pointer()
                                    .hover(|s| s.text_color(t.text))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.run_key_command(KeyCommand::ToggleSidebar, cx)
                                    }))
                                    .child("‹"),
                            ),
                    ),
            )
            .child(search_box)
            .when(searching.is_none(), |d| d.children(items))
            .children(hit_rows)
            .when(searching.is_some_and(|s| s.hits.is_empty()), |d| {
                d.child(div().px(px(10.0)).text_color(t.faint).child("No matches"))
            })
    }

    /// A thin scroll bar on the editor's right edge; drag the thumb or click
    /// the track.
    fn scrollbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        let view = self.list.viewport_bounds().size.height;
        let max = self.list.max_offset_for_scrollbar().height;
        let offset = -self.list.scroll_px_offset_for_scrollbar().y;
        let total = f32::from(view + max).max(1.0);
        let h = f32::from(view);
        let visible = max > px(1.0);
        let thumb = (h * h / total).clamp(28.0, h.max(28.0));
        let top = if f32::from(max) > 0.0 {
            (f32::from(offset) / f32::from(max)).clamp(0.0, 1.0) * (h - thumb)
        } else {
            0.0
        };
        let list = self.list.clone();
        let jump = move |y: f32, h: f32, thumb: f32| {
            let frac = ((y - thumb / 2.0) / (h - thumb).max(1.0)).clamp(0.0, 1.0);
            list.set_offset_from_scrollbar(gpui::point(px(0.0), -(max * frac)));
        };
        let down = jump.clone();
        div()
            .id("scrollbar")
            .absolute()
            .top_0()
            .right_0()
            .bottom_0()
            .w(px(12.0))
            .when(!visible, |d| d.invisible())
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, ev: &gpui::MouseDownEvent, _, cx| {
                    let top = this.list.viewport_bounds().origin.y;
                    down(f32::from(ev.position.y - top), h, thumb);
                    this.scroll_drag = true;
                    cx.notify();
                }),
            )
            .on_mouse_move(cx.listener(move |this, ev: &gpui::MouseMoveEvent, _, cx| {
                if this.scroll_drag && ev.dragging() {
                    let top = this.list.viewport_bounds().origin.y;
                    jump(f32::from(ev.position.y - top), h, thumb);
                    cx.notify();
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, _| this.scroll_drag = false),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _, _, _| this.scroll_drag = false),
            )
            .child(
                div()
                    .absolute()
                    .top(px(top))
                    .right(px(2.0))
                    .w(px(6.0))
                    .h(px(thumb))
                    .rounded(px(3.0))
                    .bg(t.muted.opacity(0.45)),
            )
    }

    /// The autocomplete list, hanging under the text it completes.
    fn completion_popup(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let open = self.completion.as_ref()?;
        if open.items.is_empty() {
            return None;
        }
        let t = self.theme;
        let layout = self.layouts.borrow().get(&open.line)?.clone();
        let line = self.host.doc.lines().get(open.line)?;
        let byte = line
            .char_indices()
            .nth(open.anchor_col)
            .map_or(line.len(), |(b, _)| b);
        let at = layout.position_for_index(byte)?;
        let (width, height) = self.viewport.get();
        let max = if open.is_wiki() {
            completion::WIKI_VISIBLE
        } else {
            3
        };
        let (start, shown) = open.window(max);
        let popup_h = shown.len() as f32 * 26.0 + 8.0;
        let popup_w = 300.0_f32;
        let x = f32::from(at.x).min(width - popup_w - 8.0).max(8.0);
        let below = f32::from(at.y) + f32::from(layout.line_height()) + 4.0;
        let y = if below + popup_h > height - 40.0 {
            (f32::from(at.y) - popup_h - 4.0).max(8.0)
        } else {
            below
        };
        Some(
            div()
                .id("completion")
                .occlude()
                .absolute()
                .left(px(x))
                .top(px(y))
                .w(px(popup_w))
                .p(px(4.0))
                .rounded(px(8.0))
                .bg(t.panel)
                .border_1()
                .border_color(t.border)
                .shadow_lg()
                .font_family(self.fonts.sans.clone())
                .text_size(px(13.0))
                .children(shown.iter().enumerate().map(|(i, item)| {
                    let index = start + i;
                    let active = index == open.selected;
                    div()
                        .id(("completion-item", index))
                        .h(px(26.0))
                        .px(px(8.0))
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .rounded(px(5.0))
                        .cursor_pointer()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_color(if active { t.heading } else { t.text })
                        .when(active, |d| d.bg(t.active))
                        .hover(|s| s.bg(t.active))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(c) = &mut this.completion {
                                c.selected = index;
                            }
                            this.accept_completion(cx);
                        }))
                        .when(item.heading.is_some(), |d| {
                            d.child(div().text_color(t.faint).child("#"))
                        })
                        .child(item.label.clone())
                }))
                .into_any_element(),
        )
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
        // Whatever moved the view (wheel, scroll bar, preview keys), large
        // notes calculate what is now on screen.
        let top = self.list.logical_scroll_top().item_ix;
        let rows = (f32::from(size.height) / 20.0) as usize + 8;
        if self.host.ensure_calc_range(top, top + rows) {
            self.restyle();
        }
        if self.host.preview != self.fullscreen {
            window.toggle_fullscreen();
            self.fullscreen = self.host.preview;
        }
        let preview = self.host.preview;
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
                    div().size_full().flex().justify_center().child(
                        div()
                            .h_full()
                            .w_full()
                            .when(preview, |d| d.max_w(px(780.0)).px(px(24.0)))
                            .child(
                                list(
                                    self.list.clone(),
                                    cx.processor(|this, ix, window, cx| {
                                        this.render_line(ix, window, cx)
                                    }),
                                )
                                .size_full(),
                            ),
                    ),
                )
                .child(self.scrollbar(cx))
            })
            .relative()
            .when(preview, |d| d.pt(px(32.0)));
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
            .when(!preview, |d| d.child(self.title_bar(cx)))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .when(self.sidebar && !preview, |d| d.child(self.sidebar(cx)))
                    .child(editor),
            )
            .when(!preview, |d| {
                d.children(crate::overlays::which_key(self))
                    .child(self.status_bar())
            })
            .children(self.completion_popup(cx))
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
