//! Everything drawn over the editor: command palette, menus, the right-click
//! menu, text prompts (passwords, reminders, collections), the collection
//! browser and history, plus the which-key strip at the bottom.
use crate::keys::{EditingMode, KeyCommand};
use crate::window::{SlateWindow, MENUS};
use editor_core::types::CommandMode;
use editor_core::vim::{VimIntent, VimMode, VimPending};
use gpui::{
    div, prelude::*, px, AnyElement, Context, FontWeight, KeyDownEvent, Pixels, Point,
    SharedString, Window,
};

/// What a menu entry does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Act {
    Cmd(&'static str),
    Key(KeyCommand),
    Intent(VimIntent),
    Mode(EditingMode),
    Theme,
    Sub(&'static str),
    Prompt(PromptKind),
    Palette,
    TableRowBelow,
    TableRowAbove,
    TableColumnRight,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Item {
    pub label: &'static str,
    /// Standard shortcut, shown in both modes.
    pub std: &'static str,
    /// Vim keys or command, shown first in vim mode.
    pub vim: &'static str,
    pub act: Act,
}

const fn it(label: &'static str, std: &'static str, vim: &'static str, act: Act) -> Item {
    Item {
        label,
        std,
        vim,
        act,
    }
}
const SEP: Item = it("", "", "", Act::Palette);

pub fn menu_items(name: &str) -> &'static [Item] {
    use Act::*;
    match name {
        "File" => &[
            it("New note", "Ctrl+N", "", Key(KeyCommand::NewNote)),
            it("Today's note", "", ":today", Cmd("today")),
            it("Browse collections…", "Ctrl+O", ":browse", Key(KeyCommand::CollectionBrowser)),
            it("History…", "Ctrl+Shift+H", ":history", Key(KeyCommand::History)),
            SEP,
            it("Save", "Ctrl+S", ":w", Key(KeyCommand::Save)),
            it("Reload from disk", "", ":reload", Cmd("reload")),
            it("Export", "", "", Sub("export")),
            it("Note security", "", "", Sub("security")),
            SEP,
            it("Quit", "Ctrl+Q", ":q", Key(KeyCommand::Quit)),
        ],
        "Edit" => &[
            it("Undo", "Ctrl+Z", "u", Intent(VimIntent::Undo)),
            it("Redo", "Ctrl+Shift+Z", "Ctrl+R", Intent(VimIntent::Redo)),
            SEP,
            it("Cut", "Ctrl+X", "d", Key(KeyCommand::Cut)),
            it("Copy", "Ctrl+C", "y", Key(KeyCommand::Copy)),
            it("Paste", "Ctrl+V", "p", Key(KeyCommand::Paste)),
            it("Select all", "Ctrl+A", "ggVG", Key(KeyCommand::SelectAll)),
            SEP,
            it("Command palette…", "Ctrl+Shift+P", ":", Palette),
        ],
        "View" => &[
            it("Sidebar", "Ctrl+\\", "", Key(KeyCommand::ToggleSidebar)),
            it("Theme", "", "", Sub("theme")),
            it("Editing mode", "", "", Sub("editing")),
            SEP,
            it("Keys and commands", "F1", ":help", Cmd("help")),
        ],
        "Format" => &[
            it("Bold", "Ctrl+B", ":bold", Cmd("format bold")),
            it("Italic", "Ctrl+I", ":italic", Cmd("format italic")),
            it("Strikethrough", "", ":strike", Cmd("format strike")),
            it("Inline code", "", ":icode", Cmd("format code")),
            SEP,
            it("Heading", "", ":title", Cmd("paragraph title")),
            it("Checklist", "", ":clist", Cmd("paragraph clist")),
            it("Bulleted list", "", ":ulist", Cmd("paragraph ulist")),
            it("Numbered list", "", ":olist", Cmd("paragraph olist")),
            SEP,
            it("Clear formatting", "", ":unformat", Cmd("format clear")),
            it("Format document", "", ":format", Cmd("format")),
        ],
        "Calc" => &[
            it("Sum paragraph", "", ":sum", Cmd("sum")),
            it("Sum column", "", ":sum column", Cmd("sum column")),
            it("Average column", "", ":avg column", Cmd("avg column")),
            it("Sum document", "", ":sum doc", Cmd("sum doc")),
            SEP,
            it("Insert date", "", ":date", Cmd("date")),
            it("Set reminder…", "", ":remind", Prompt(PromptKind::Remind)),
            it("Run script…", "", ":run", Prompt(PromptKind::Run)),
            it("Modules", "", "", Sub("modules")),
        ],
        "Help" => &[
            it("Keys and commands", "F1", ":help", Cmd("help")),
            it("Command palette", "Ctrl+Shift+P", ":", Palette),
        ],
        "export" => &[
            it("Markdown", "", ":export md", Cmd("export md")),
            it("Plain text", "", ":export txt", Cmd("export txt")),
            it("PDF", "", ":export pdf", Cmd("export pdf")),
        ],
        "security" => &[
            it("Encrypt note…", "", ":encrypt", Prompt(PromptKind::Encrypt)),
            it("Decrypt note…", "", ":decrypt", Prompt(PromptKind::Decrypt)),
        ],
        "theme" => &[it("Dark", "", "", Theme), it("Light", "", "", Theme)],
        "editing" => &[
            it("Vim", "", "", Mode(EditingMode::Vim)),
            it("Standard", "", "", Mode(EditingMode::Standard)),
        ],
        "modules" => &[
            it("Math", "", ":module math", Cmd("module toggle math")),
            it("Variables", "", ":module variables", Cmd("module toggle variables")),
            it("Tables", "", ":module table", Cmd("module toggle table")),
            it("Styling", "", ":module style", Cmd("module toggle style")),
            it("Cross-note values", "", ":module cross-note", Cmd("module toggle cross-note")),
        ],
        "format" => &[
            it("Bold", "Ctrl+B", ":bold", Cmd("format bold")),
            it("Italic", "Ctrl+I", ":italic", Cmd("format italic")),
            it("Strikethrough", "", ":strike", Cmd("format strike")),
            it("Inline code", "", ":icode", Cmd("format code")),
        ],
        "turn" => &[
            it("Heading", "", ":title", Cmd("paragraph title")),
            it("Checklist", "", ":clist", Cmd("paragraph clist")),
            it("Bulleted list", "", ":ulist", Cmd("paragraph ulist")),
            it("Numbered list", "", ":olist", Cmd("paragraph olist")),
        ],
        "table" => &[
            it("Insert row above", "", "O", TableRowAbove),
            it("Insert row below", "", "o", TableRowBelow),
            it("Insert column right", "", "", TableColumnRight),
            SEP,
            it("Delete row", "", "dd", Intent(VimIntent::DeleteLine)),
            SEP,
            it("Sum column", "", ":sum column", Cmd("sum column")),
            it("Average column", "", ":avg column", Cmd("avg column")),
        ],
        _ => &[],
    }
}

fn context_items(table: bool) -> Vec<Item> {
    use Act::*;
    let mut items = vec![
        it("Cut", "Ctrl+X", "d", Key(KeyCommand::Cut)),
        it("Copy", "Ctrl+C", "y", Key(KeyCommand::Copy)),
        it("Paste", "Ctrl+V", "p", Key(KeyCommand::Paste)),
        SEP,
        it("Format", "", "", Sub("format")),
        it("Turn into", "", "", Sub("turn")),
    ];
    if table {
        items.push(it("Table", "", "", Sub("table")));
    }
    items.extend([
        SEP,
        it("Set reminder…", "", ":remind", Prompt(PromptKind::Remind)),
        it("Run script…", "", ":run", Prompt(PromptKind::Run)),
        SEP,
        it("Command palette…", "Ctrl+Shift+P", ":", Palette),
    ]);
    items
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptKind {
    Unlock,
    Encrypt,
    Decrypt,
    Remind,
    Run,
    NewCollection,
    ExportPath,
    UnlockCollection,
}

impl PromptKind {
    fn title(self) -> &'static str {
        match self {
            PromptKind::Unlock => "This note is encrypted. Password:",
            PromptKind::Encrypt => "Encrypt note with password:",
            PromptKind::Decrypt => "Password to remove encryption:",
            PromptKind::Remind => "Remind at (e.g. 2026-10-14 09:00, tomorrow 9am, in 2h):",
            PromptKind::Run => "Run script (name and arguments):",
            PromptKind::NewCollection => "New collection name:",
            PromptKind::ExportPath => "Export to file:",
            PromptKind::UnlockCollection => "This collection is encrypted. Password:",
        }
    }
    fn secret(self) -> bool {
        matches!(
            self,
            PromptKind::Unlock
                | PromptKind::Encrypt
                | PromptKind::Decrypt
                | PromptKind::UnlockCollection
        )
    }
}

pub struct Prompt {
    pub kind: PromptKind,
    pub text: String,
    /// First entry of a password that must be typed twice.
    pub first: Option<String>,
    /// Extra data for the prompt (export format).
    pub extra: String,
}

#[derive(Default)]
pub enum Overlay {
    #[default]
    None,
    Palette {
        query: String,
        selected: usize,
    },
    Menu {
        name: &'static str,
        sub: Option<&'static str>,
    },
    Context {
        pos: Point<Pixels>,
        table: bool,
        sub: Option<&'static str>,
    },
    Prompt(Prompt),
    Browser(crate::browser::Browser),
    History(crate::history::History),
}

pub fn close(win: &mut SlateWindow) {
    win.overlay = Overlay::None;
}

pub fn open_palette(win: &mut SlateWindow, initial: &str, cx: &mut Context<SlateWindow>) {
    win.overlay = Overlay::Palette {
        query: initial.trim_start_matches(':').to_string(),
        selected: 0,
    };
    cx.notify();
}

pub fn open_prompt(win: &mut SlateWindow, kind: PromptKind, extra: &str, cx: &mut Context<SlateWindow>) {
    win.overlay = Overlay::Prompt(Prompt {
        kind,
        text: String::new(),
        first: None,
        extra: extra.to_string(),
    });
    cx.notify();
}

pub fn open_browser(win: &mut SlateWindow, cx: &mut Context<SlateWindow>) {
    match crate::browser::Browser::open(&win.host) {
        Ok(b) => win.overlay = Overlay::Browser(b),
        Err(err) => win.set_status(err),
    }
    cx.notify();
}

pub fn open_history(win: &mut SlateWindow, cx: &mut Context<SlateWindow>) {
    if let Err(err) = win.host.save() {
        win.set_status(format!("save failed: {err}"));
    }
    match crate::history::History::open(&win.host) {
        Ok(h) => win.overlay = Overlay::History(h),
        Err(err) => win.set_status(err),
    }
    cx.notify();
}

pub fn open_context_menu(
    win: &mut SlateWindow,
    pos: Point<Pixels>,
    table: bool,
    cx: &mut Context<SlateWindow>,
) {
    win.overlay = Overlay::Context {
        pos,
        table,
        sub: None,
    };
    cx.notify();
}

pub fn menu_open(win: &SlateWindow, label: &str) -> bool {
    matches!(win.overlay, Overlay::Menu { name, .. } if name == label)
}

pub fn toggle_menu(win: &mut SlateWindow, label: &'static str, cx: &mut Context<SlateWindow>) {
    win.overlay = if menu_open(win, label) {
        Overlay::None
    } else {
        Overlay::Menu {
            name: label,
            sub: None,
        }
    };
    cx.notify();
}

/// Run a menu entry. Submenus open in place.
pub fn activate(win: &mut SlateWindow, act: Act, cx: &mut Context<SlateWindow>) {
    if let Act::Sub(sub) = act {
        match &mut win.overlay {
            Overlay::Menu { sub: s, .. } | Overlay::Context { sub: s, .. } => *s = Some(sub),
            _ => {}
        }
        cx.notify();
        return;
    }
    win.overlay = Overlay::None;
    match act {
        Act::Cmd(cmd) => win.run_command(cmd, cx),
        Act::Key(k) => win.run_key_command(k, cx),
        Act::Intent(i) => win.run_action(i, cx),
        Act::Mode(m) => win.set_mode(m, cx),
        Act::Theme => win.toggle_theme(cx),
        Act::Prompt(kind) => open_prompt(win, kind, "", cx),
        Act::Palette => open_palette(win, "", cx),
        Act::TableRowBelow => crate::commands::table_insert_row(win, false, cx),
        Act::TableRowAbove => crate::commands::table_insert_row(win, true, cx),
        Act::TableColumnRight => crate::commands::table_insert_column(win, cx),
        Act::Sub(_) => {}
    }
}

fn suggestions(query: &str) -> Vec<editor_core::types::CommandSuggestion> {
    editor_core::commands::list_command_suggestions(CommandMode::Vim, query)
}

/// Keys for the open overlay. Returns whether the key was used.
pub fn on_key(
    win: &mut SlateWindow,
    ev: &KeyDownEvent,
    _window: &mut Window,
    cx: &mut Context<SlateWindow>,
) -> bool {
    let k = &ev.keystroke;
    let typed = k
        .key_char
        .clone()
        .filter(|s| !s.is_empty() && !k.modifiers.control);
    match &mut win.overlay {
        Overlay::None => false,
        Overlay::Palette { query, selected } => {
            let list = suggestions(query);
            match k.key.as_str() {
                "escape" => win.overlay = Overlay::None,
                "up" => *selected = selected.saturating_sub(1),
                "down" => *selected = (*selected + 1).min(list.len().saturating_sub(1)),
                "tab" => {
                    if let Some(s) = list.get(*selected) {
                        *query = format!("{} ", s.value);
                    }
                }
                "backspace" => {
                    query.pop();
                    *selected = 0;
                }
                "enter" => {
                    // A bare prefix runs the highlighted command; text with
                    // arguments runs as typed.
                    let raw = match list.get(*selected) {
                        Some(s) if !query.contains(' ') && !query.is_empty() => s.value.clone(),
                        _ => query.clone(),
                    };
                    win.overlay = Overlay::None;
                    win.run_command(&raw, cx);
                    return true;
                }
                _ => {
                    if let Some(text) = typed {
                        query.push_str(&text);
                        *selected = 0;
                    }
                }
            }
            cx.notify();
            true
        }
        Overlay::Menu { .. } | Overlay::Context { .. } => {
            if k.key == "escape" {
                win.overlay = Overlay::None;
                cx.notify();
                return true;
            }
            false
        }
        Overlay::Prompt(p) => {
            match k.key.as_str() {
                "escape" => {
                    win.overlay = Overlay::None;
                    win.set_status("cancelled");
                }
                "backspace" => {
                    p.text.pop();
                }
                "enter" => {
                    let Overlay::Prompt(p) = std::mem::take(&mut win.overlay) else {
                        unreachable!()
                    };
                    crate::commands::submit_prompt(win, p, cx);
                    return true;
                }
                _ => {
                    if k.modifiers.control && k.key == "v" {
                        if let Some(text) = cx.read_from_clipboard().and_then(|i| i.text()) {
                            p.text.push_str(text.trim_end_matches(['\n', '\r']));
                        }
                    } else if let Some(text) = typed {
                        p.text.push_str(&text);
                    }
                }
            }
            cx.notify();
            true
        }
        Overlay::Browser(_) => crate::browser::on_key(win, ev, cx),
        Overlay::History(_) => crate::history::on_key(win, ev, cx),
    }
}

/// Menu entry row; `vim` keys come first in vim mode.
fn menu_row(
    win: &SlateWindow,
    item: Item,
    id: SharedString,
    highlighted: bool,
    cx: &mut Context<SlateWindow>,
) -> AnyElement {
    let t = win.theme;
    if item.label.is_empty() {
        return div().h(px(1.0)).mx(px(6.0)).my(px(4.0)).bg(t.border).into_any_element();
    }
    let vim = win.mode == EditingMode::Vim;
    let (key, alt) = if vim && !item.vim.is_empty() {
        (item.vim, item.std)
    } else {
        (item.std, "")
    };
    let checked = match item.act {
        Act::Mode(m) => m == win.mode,
        Act::Theme => (item.label == "Light") == win.light,
        Act::Key(KeyCommand::ToggleSidebar) => win.sidebar,
        Act::Cmd(c) if c.starts_with("module toggle ") => {
            let m = win.host.modules;
            match &c["module toggle ".len()..] {
                "math" => m.math,
                "variables" => m.variables,
                "table" => m.table,
                "style" => m.style,
                _ => m.cross_note,
            }
        }
        _ => false,
    };
    let act = item.act;
    div()
        .id(id)
        .flex()
        .items_center()
        .gap(px(8.0))
        .h(px(30.0))
        .pl(px(6.0))
        .pr(px(10.0))
        .rounded(px(5.0))
        .cursor_pointer()
        .when(highlighted, |d| d.bg(t.active))
        .hover(|s| s.bg(t.active))
        .on_click(cx.listener(move |this, _, _, cx| activate(this, act, cx)))
        .child(div().w(px(14.0)).text_color(t.blue).child(if checked { "✓" } else { "" }))
        .child(div().flex_1().whitespace_nowrap().child(item.label))
        .child(
            div()
                .font_family(win.fonts.mono.clone())
                .text_size(px(11.5))
                .text_color(t.muted)
                .child(key),
        )
        .child(
            div()
                .font_family(win.fonts.mono.clone())
                .text_size(px(11.0))
                .text_color(t.faint)
                .child(alt),
        )
        .child(
            div()
                .w(px(8.0))
                .text_color(t.muted)
                .child(if matches!(item.act, Act::Sub(_)) { "›" } else { "" }),
        )
        .into_any_element()
}

fn panel(win: &SlateWindow, left: f32, top: f32, width: f32) -> gpui::Div {
    let t = win.theme;
    div()
        .absolute()
        .left(px(left))
        .top(px(top))
        .w(px(width))
        .p(px(5.0))
        .bg(t.panel)
        .border_1()
        .border_color(t.border)
        .rounded(px(8.0))
        .shadow_lg()
        .text_size(px(13.0))
        .text_color(t.text)
        .flex()
        .flex_col()
}

fn menu_panels(
    win: &SlateWindow,
    items: &[Item],
    sub: Option<&'static str>,
    left: f32,
    top: f32,
    cx: &mut Context<SlateWindow>,
) -> Vec<AnyElement> {
    const W: f32 = 300.0;
    let mut out = Vec::new();
    let rows: Vec<AnyElement> = items
        .iter()
        .enumerate()
        .map(|(i, item)| {
            let open = matches!(item.act, Act::Sub(s) if Some(s) == sub);
            menu_row(win, *item, format!("m{left}-{i}").into(), open, cx)
        })
        .collect();
    out.push(panel(win, left, top, W).children(rows).into_any_element());
    if let Some(sub) = sub {
        let offset: f32 = items
            .iter()
            .take_while(|i| !matches!(i.act, Act::Sub(s) if s == sub))
            .map(|i| if i.label.is_empty() { 9.0 } else { 30.0 })
            .sum();
        let rows: Vec<AnyElement> = menu_items(sub)
            .iter()
            .enumerate()
            .map(|(i, item)| menu_row(win, *item, format!("s{sub}-{i}").into(), false, cx))
            .collect();
        out.push(
            panel(win, left + W - 6.0, top + offset, 270.0)
                .children(rows)
                .into_any_element(),
        );
    }
    out
}

fn menu_left(name: &str) -> f32 {
    let mut x = 36.0;
    for label in MENUS {
        if label == name {
            break;
        }
        x += label.len() as f32 * 7.0 + 18.0 + 2.0;
    }
    x
}

fn palette(win: &SlateWindow, query: &str, selected: usize) -> AnyElement {
    let t = win.theme;
    let list = suggestions(query);
    let rows = list.iter().take(12).enumerate().map(|(i, s)| {
        div()
            .flex()
            .gap(px(16.0))
            .px(px(12.0))
            .py(px(7.0))
            .rounded(px(6.0))
            .when(i == selected, |d| d.bg(t.active))
            .child(
                div()
                    .w(px(170.0))
                    .flex_none()
                    .font_family(win.fonts.mono.clone())
                    .text_color(t.text)
                    .child(s.value.clone()),
            )
            .child(
                div()
                    .flex_1()
                    .text_color(if i == selected { t.text } else { t.muted })
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .child(s.description.clone()),
            )
    });
    div()
        .absolute()
        .top(px(96.0))
        .left_0()
        .right_0()
        .flex()
        .justify_center()
        .child(
            div()
                .w(px(640.0))
                .bg(t.panel)
                .border_1()
                .border_color(t.border)
                .rounded(px(10.0))
                .shadow_lg()
                .overflow_hidden()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(10.0))
                        .h(px(52.0))
                        .px(px(16.0))
                        .border_b_1()
                        .border_color(t.border)
                        .font_family(win.fonts.mono.clone())
                        .text_size(px(16.0))
                        .child(div().text_color(t.blue).font_weight(FontWeight::SEMIBOLD).child(":"))
                        .child(div().flex_1().text_color(t.heading).child(format!("{query}▏")))
                        .child(
                            div()
                                .font_family(win.fonts.sans.clone())
                                .text_size(px(11.5))
                                .text_color(t.faint)
                                .child(format!("{} matches", list.len())),
                        ),
                )
                .child(div().p(px(6.0)).flex().flex_col().children(rows))
                .child(
                    div()
                        .flex()
                        .gap(px(18.0))
                        .px(px(16.0))
                        .py(px(9.0))
                        .border_t_1()
                        .border_color(t.border)
                        .text_size(px(11.5))
                        .text_color(t.muted)
                        .child("Tab complete")
                        .child("↑ ↓ select")
                        .child("Enter run")
                        .child("Esc close"),
                ),
        )
        .into_any_element()
}

fn prompt(win: &SlateWindow, p: &Prompt) -> AnyElement {
    let t = win.theme;
    let shown = if p.kind.secret() {
        "•".repeat(p.text.chars().count())
    } else {
        p.text.clone()
    };
    let title = if p.kind == PromptKind::Encrypt && p.first.is_some() {
        "Repeat the password:"
    } else {
        p.kind.title()
    };
    div()
        .absolute()
        .top(px(140.0))
        .left_0()
        .right_0()
        .flex()
        .justify_center()
        .child(
            div()
                .w(px(480.0))
                .p(px(16.0))
                .bg(t.panel)
                .border_1()
                .border_color(t.border)
                .rounded(px(10.0))
                .shadow_lg()
                .flex()
                .flex_col()
                .gap(px(10.0))
                .child(div().text_color(t.heading).child(title))
                .child(
                    div()
                        .h(px(34.0))
                        .px(px(10.0))
                        .flex()
                        .items_center()
                        .rounded(px(6.0))
                        .border_1()
                        .border_color(t.blue)
                        .bg(t.bg)
                        .font_family(win.fonts.mono.clone())
                        .child(format!("{shown}▏")),
                )
                .child(
                    div()
                        .text_size(px(11.5))
                        .text_color(t.muted)
                        .child("Enter confirm · Esc cancel"),
                ),
        )
        .into_any_element()
}

/// Keys that may follow a pending vim prefix, from the shared engine's map.
fn which_keys(pending: VimPending) -> &'static [(&'static str, &'static str)] {
    match pending {
        VimPending::Go => &[
            ("g", "first line"),
            ("j", "screen row down"),
            ("k", "screen row up"),
        ],
        VimPending::Delete => &[
            ("d", "line"),
            ("w", "to next word"),
            ("b", "to previous word"),
            ("e", "to word end"),
            ("$", "to line end"),
            ("0", "to line start"),
            ("t", "till character"),
            ("i", "inside…"),
            ("a", "around…"),
        ],
        VimPending::Yank => &[
            ("y", "line"),
            ("w", "to next word"),
            ("b", "to previous word"),
            ("$", "to line end"),
            ("0", "to line start"),
            ("i", "inside…"),
            ("a", "around…"),
        ],
        VimPending::Change => &[
            ("c", "line"),
            ("w", "to next word"),
            ("$", "to line end"),
            ("t", "till character"),
            ("i", "inside…"),
            ("a", "around…"),
        ],
        VimPending::DeleteInner
        | VimPending::DeleteAround
        | VimPending::YankInner
        | VimPending::YankAround
        | VimPending::ChangeInner
        | VimPending::ChangeAround => &[
            ("w", "word"),
            ("|", "table cell"),
            ("( )", "parentheses"),
            ("[ ]", "brackets"),
            ("{ }", "braces"),
            ("\"", "quotes"),
            ("`", "backticks"),
            ("*", "asterisks"),
            ("~", "tildes"),
            ("_", "underscores"),
        ],
        VimPending::DeleteTill | VimPending::ChangeTill => &[("any", "character to stop at")],
        VimPending::MacroRecord | VimPending::MacroPlay => &[("a–z", "register")],
    }
}

/// The which-key strip, docked above the status bar.
pub fn which_key(win: &SlateWindow) -> Option<AnyElement> {
    if win.mode != EditingMode::Vim || win.host.input.mode() == VimMode::Insert {
        return None;
    }
    let pending = win.host.input.vim.pending?;
    let t = win.theme;
    let keys = which_keys(pending);
    Some(
        div()
            .flex_none()
            .flex()
            .flex_wrap()
            .items_center()
            .gap_x(px(18.0))
            .gap_y(px(4.0))
            .px(px(14.0))
            .py(px(8.0))
            .bg(t.panel)
            .border_t_1()
            .border_color(t.border)
            .text_size(px(12.5))
            .child(
                div()
                    .px(px(8.0))
                    .rounded(px(4.0))
                    .bg(t.blue)
                    .text_color(t.on_accent)
                    .font_family(win.fonts.mono.clone())
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(win.host.input.pending_keys()),
            )
            .children(keys.iter().map(|(key, desc)| {
                div()
                    .flex()
                    .gap(px(6.0))
                    .child(
                        div()
                            .font_family(win.fonts.mono.clone())
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(t.heading)
                            .child(*key),
                    )
                    .child(div().text_color(t.muted).child(*desc))
            }))
            .child(div().text_color(t.faint).child("Esc cancel"))
            .into_any_element(),
    )
}

pub fn render(win: &SlateWindow, window: &mut Window, cx: &mut Context<SlateWindow>) -> Vec<AnyElement> {
    let _ = window;
    match &win.overlay {
        Overlay::None => Vec::new(),
        Overlay::Palette { query, selected } => vec![palette(win, query, *selected)],
        Overlay::Menu { name, sub } => {
            menu_panels(win, menu_items(name), *sub, menu_left(name), 34.0, cx)
        }
        Overlay::Context { pos, table, sub } => {
            let items = context_items(*table);
            let x = f32::from(pos.x).min(1280.0 - 580.0).max(8.0);
            let y = f32::from(pos.y).max(8.0);
            menu_panels(win, &items, *sub, x, y, cx)
        }
        Overlay::Prompt(p) => vec![prompt(win, p)],
        Overlay::Browser(b) => vec![crate::browser::render(win, b, cx)],
        Overlay::History(h) => vec![crate::history::render(win, h, cx)],
    }
}
