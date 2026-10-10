//! Commands the editor core leaves to the host, and the actions behind
//! prompts and table menus. Everything goes through `app-core` storage and
//! the note session; features whose engine lives only in the terminal app
//! (PDF export, backups, web search, clipboard watching, image paste, folds)
//! report that they are not in the desktop app yet.
use crate::overlays::{self, Prompt, PromptKind};
use crate::window::SlateWindow;
use app_core::storage::NoteModules;
use chrono::{
    Datelike, Duration as ChronoDuration, Local, NaiveDate, NaiveTime, TimeZone, Timelike,
};
use editor_core::command_catalog::{
    parse_backup_command, parse_collection_command, parse_export_command, parse_web_search_command,
    CollectionCommandAction, CommandId, ExportFormat,
};
use gpui::Context;
use note_session::input::InputOutcome;

fn notify_status(win: &mut SlateWindow, msg: impl Into<String>, cx: &mut Context<SlateWindow>) {
    win.set_status(msg);
    cx.notify();
}

fn not_yet(win: &mut SlateWindow, what: &str, cx: &mut Context<SlateWindow>) {
    notify_status(
        win,
        format!("{what} is not in the desktop app yet (use `slate`)"),
        cx,
    );
}

/// Arguments after the command word, e.g. `foo bar` in `run foo bar`.
fn args_after<'a>(raw: &'a str, word: &str) -> &'a str {
    raw.trim()
        .strip_prefix(word)
        .map(str::trim)
        .unwrap_or_default()
}

pub fn run_host_command(
    win: &mut SlateWindow,
    id: Option<CommandId>,
    raw: &str,
    cx: &mut Context<SlateWindow>,
) {
    let Some(id) = id else {
        notify_status(win, format!("unknown command: {raw}"), cx);
        return;
    };
    use CommandId::*;
    match id {
        Write => win.save(cx),
        WriteQuit | Quit => win.quit(cx),
        Reload => reload(win, cx),
        Today => today(win, cx),
        Date => insert_date(win, cx),
        Browse => overlays::open_browser(win, cx),
        History => overlays::open_history(win, cx),
        Help => overlays::open_palette(win, "", cx),
        Remind => {
            let when = args_after(raw, "remind");
            if when.is_empty() {
                overlays::open_prompt(win, PromptKind::Remind, "", cx);
            } else {
                set_reminder(win, when, cx);
            }
        }
        RemindToggle => {
            let line = win.host.doc.cursor_line;
            if win.host.session.reminders().contains_key(&line) {
                clear_reminder(win, cx);
            } else {
                overlays::open_prompt(win, PromptKind::Remind, "", cx);
            }
        }
        ModuleStatus => {
            let m = win.host.modules;
            let on = |b: bool| if b { "on" } else { "off" };
            notify_status(
                win,
                format!(
                    "math {} · variables {} · table {} · style {} · cross-note {}",
                    on(m.math),
                    on(m.variables),
                    on(m.table),
                    on(m.style),
                    on(m.cross_note)
                ),
                cx,
            );
        }
        ModuleOnMath
        | ModuleOffMath
        | ModuleToggleMath
        | ModuleOnTable
        | ModuleOffTable
        | ModuleToggleTable
        | ModuleOnVariables
        | ModuleOffVariables
        | ModuleToggleVariables
        | ModuleOnStyle
        | ModuleOffStyle
        | ModuleToggleStyle
        | ModuleOnCrossNote
        | ModuleOffCrossNote
        | ModuleToggleCrossNote => set_module(win, id, cx),
        ChooseCollection | ClearCollection | CreateCollection | DeleteCollection
        | UpdateCollection | PurgeCollection | AddToCollection | RemoveFromCollection => {
            collection_command(win, raw, cx)
        }
        NoteEncrypt => overlays::open_prompt(win, PromptKind::Encrypt, "", cx),
        NoteDecrypt => overlays::open_prompt(win, PromptKind::Decrypt, "", cx),
        ExportPdf | ExportMd | ExportTxt => match parse_export_command(raw) {
            Some(cmd) => export(win, cmd.format, cmd.path, cx),
            None => notify_status(win, "usage: export md|txt|pdf [path]", cx),
        },
        Run => {
            let args = args_after(raw, "run");
            if args.is_empty() {
                overlays::open_prompt(win, PromptKind::Run, "", cx);
            } else {
                run_script(win, args, cx);
            }
        }
        RunCancel => notify_status(win, "no script is running", cx),
        BackupExport | BackupLoad => {
            let _ = parse_backup_command(raw);
            not_yet(win, "backup", cx)
        }
        WebSearch => {
            let _ = parse_web_search_command(raw);
            not_yet(win, "web search", cx)
        }
        CurrencyRefresh => not_yet(win, "currency refresh", cx),
        ClipWatch | ClipWatchStop => not_yet(win, "clipboard watching", cx),
        PasteImage => {
            if !win.paste_image(cx) {
                notify_status(win, "no image on the clipboard", cx);
            }
        }
        Fold | Unfold | FoldToggle => not_yet(win, "folding", cx),
        // Core commands never reach the host.
        _ => notify_status(win, format!("{raw}: nothing to do"), cx),
    }
}

fn reload(win: &mut SlateWindow, cx: &mut Context<SlateWindow>) {
    if win.host.session.dirty() {
        notify_status(win, "unsaved changes; save first or undo them", cx);
        return;
    }
    let id = win.host.note_id().to_string();
    match win.host.db.get_note(&id) {
        Ok(Some(note)) => {
            win.host.open_note(note);
            win.reload_lines();
            notify_status(win, "reloaded", cx);
        }
        Ok(None) => notify_status(win, "note no longer exists", cx),
        Err(err) => notify_status(win, err, cx),
    }
}

fn stamp() -> app_core::daily::LocalStamp {
    let now = Local::now();
    app_core::daily::LocalStamp {
        year: now.year(),
        month: now.month(),
        day: now.day(),
        hour: now.hour(),
        minute: now.minute(),
    }
}

fn today(win: &mut SlateWindow, cx: &mut Context<SlateWindow>) {
    let config = app_core::config::load_daily_notes_config();
    let label = Local::now().format("%A, %B %-d, %Y").to_string();
    match app_core::daily::ensure_daily_note(&win.host.db, &config, stamp(), &label) {
        Ok(note) => {
            win.host.refresh_notes();
            win.open_note(&note.id, cx);
        }
        Err(err) => notify_status(win, err, cx),
    }
}

fn insert_date(win: &mut SlateWindow, cx: &mut Context<SlateWindow>) {
    let before = win.snapshot_cursor();
    let text = Local::now().format("%Y-%m-%d").to_string();
    let outcome = win.host.insert_text(&text);
    win.after_input(before, outcome, cx);
}

pub fn new_note(win: &mut SlateWindow, cx: &mut Context<SlateWindow>) {
    let id = ulid::Ulid::new().to_string();
    let collection = win.host.working.as_ref().map(|(id, _)| id.clone());
    match win.host.db.create_note_with_context(
        &id,
        NoteModules::default(),
        None,
        collection.as_deref(),
    ) {
        Ok(_) => {
            win.host.refresh_notes();
            win.open_note(&id, cx);
            notify_status(win, "new note", cx);
        }
        Err(err) => notify_status(win, err, cx),
    }
}

fn set_module(win: &mut SlateWindow, id: CommandId, cx: &mut Context<SlateWindow>) {
    use CommandId::*;
    let mut m = win.host.modules;
    let (slot, name, value): (&mut bool, &str, Option<bool>) = match id {
        ModuleOnMath => (&mut m.math, "math", Some(true)),
        ModuleOffMath => (&mut m.math, "math", Some(false)),
        ModuleToggleMath => (&mut m.math, "math", None),
        ModuleOnTable => (&mut m.table, "table", Some(true)),
        ModuleOffTable => (&mut m.table, "table", Some(false)),
        ModuleToggleTable => (&mut m.table, "table", None),
        ModuleOnVariables => (&mut m.variables, "variables", Some(true)),
        ModuleOffVariables => (&mut m.variables, "variables", Some(false)),
        ModuleToggleVariables => (&mut m.variables, "variables", None),
        ModuleOnStyle => (&mut m.style, "style", Some(true)),
        ModuleOffStyle => (&mut m.style, "style", Some(false)),
        ModuleToggleStyle => (&mut m.style, "style", None),
        ModuleOnCrossNote => (&mut m.cross_note, "cross-note", Some(true)),
        ModuleOffCrossNote => (&mut m.cross_note, "cross-note", Some(false)),
        _ => (&mut m.cross_note, "cross-note", None),
    };
    *slot = value.unwrap_or(!*slot);
    let state = if *slot { "on" } else { "off" };
    let message = format!("{name} {state}");
    let note_id = win.host.note_id().to_string();
    match win.host.db.set_note_modules(&note_id, m) {
        Ok(_) => {
            win.host.modules = m;
            win.host.recompute_calc();
            win.restyle();
            notify_status(win, message, cx);
        }
        Err(err) => notify_status(win, err, cx),
    }
}

fn collection_command(win: &mut SlateWindow, raw: &str, cx: &mut Context<SlateWindow>) {
    let Some(cmd) = parse_collection_command(raw) else {
        notify_status(
            win,
            "usage: collection create|delete|join|leave|choose|clear <name>",
            cx,
        );
        return;
    };
    let db = &win.host.db;
    let note_id = win.host.note_id().to_string();
    let find = |name: &Option<String>| -> Result<app_core::storage::Collection, String> {
        let name = name.as_deref().ok_or("collection name required")?;
        db.get_collection_by_name(name)?
            .ok_or_else(|| format!("no collection named {name}"))
    };
    let result: Result<String, String> = match cmd.action {
        CollectionCommandAction::Create => match cmd.collection.as_deref() {
            Some(name) => db
                .create_collection(name, "")
                .map(|c| format!("created {}", c.name)),
            None => {
                overlays::open_prompt(win, PromptKind::NewCollection, "", cx);
                return;
            }
        },
        CollectionCommandAction::Delete => find(&cmd.collection).and_then(|c| {
            db.delete_collection(&c.id)
                .map(|_| format!("deleted {}", c.name))
        }),
        CollectionCommandAction::Purge => find(&cmd.collection).and_then(|c| {
            db.purge_collection(&c.id)
                .map(|n| format!("purged {} ({n} notes)", c.name))
        }),
        CollectionCommandAction::Add => find(&cmd.collection).and_then(|c| {
            db.add_notes_to_collection(&c.id, std::slice::from_ref(&note_id))
                .map(|_| format!("added to {}", c.name))
        }),
        CollectionCommandAction::Remove => find(&cmd.collection).and_then(|c| {
            db.remove_notes_from_collection(&c.id, std::slice::from_ref(&note_id))
                .map(|_| format!("removed from {}", c.name))
        }),
        CollectionCommandAction::Choose => {
            overlays::open_browser(win, cx);
            return;
        }
        CollectionCommandAction::Clear => Ok("showing all notes".to_string()),
        CollectionCommandAction::Update => Err("rename a collection from the browser".into()),
    };
    match result {
        Ok(msg) => notify_status(win, msg, cx),
        Err(err) => notify_status(win, err, cx),
    }
}

fn export(
    win: &mut SlateWindow,
    format: ExportFormat,
    path: Option<String>,
    cx: &mut Context<SlateWindow>,
) {
    if format == ExportFormat::Pdf {
        not_yet(win, "PDF export", cx);
        return;
    }
    let Some(path) = path else {
        overlays::open_prompt(win, PromptKind::ExportPath, format.as_str(), cx);
        return;
    };
    let text = win.host.doc.lines().join("\n");
    let path = std::path::PathBuf::from(shellexpand_home(&path));
    match std::fs::write(&path, text) {
        Ok(()) => notify_status(win, format!("exported to {}", path.display()), cx),
        Err(err) => notify_status(win, format!("export failed: {err}"), cx),
    }
}

fn shellexpand_home(path: &str) -> String {
    match (path.strip_prefix("~/"), std::env::var("HOME")) {
        (Some(rest), Ok(home)) => format!("{home}/{rest}"),
        _ => path.to_string(),
    }
}

/// Parse a reminder time: `YYYY-MM-DD HH:MM`, `HH:MM` (today or tomorrow),
/// `tomorrow [9am|09:00]`, or `in 30m` / `in 2h` / `in 3d`.
pub fn parse_when(input: &str, now: chrono::DateTime<Local>) -> Option<chrono::DateTime<Local>> {
    let s = input.trim().to_ascii_lowercase();
    if let Some(rest) = s.strip_prefix("in ") {
        let rest = rest.trim();
        let (num, unit) = rest.split_at(rest.find(|c: char| !c.is_ascii_digit())?);
        let n: i64 = num.parse().ok()?;
        let d = match unit.trim() {
            "m" | "min" | "mins" | "minutes" => ChronoDuration::minutes(n),
            "h" | "hour" | "hours" => ChronoDuration::hours(n),
            "d" | "day" | "days" => ChronoDuration::days(n),
            _ => return None,
        };
        return Some(now + d);
    }
    let time_of = |t: &str| -> Option<NaiveTime> {
        let t = t.trim();
        if let Ok(t) = NaiveTime::parse_from_str(t, "%H:%M") {
            return Some(t);
        }
        let (digits, pm) = if let Some(d) = t.strip_suffix("pm") {
            (d, true)
        } else {
            (t.strip_suffix("am")?, false)
        };
        let h: u32 = digits.trim().parse().ok()?;
        NaiveTime::from_hms_opt(if pm { h % 12 + 12 } else { h % 12 }, 0, 0)
    };
    let at =
        |date: NaiveDate, time: NaiveTime| Local.from_local_datetime(&date.and_time(time)).single();
    if let Some(rest) = s.strip_prefix("tomorrow") {
        let date = now.date_naive() + ChronoDuration::days(1);
        let time = if rest.trim().is_empty() {
            NaiveTime::from_hms_opt(9, 0, 0)?
        } else {
            time_of(rest)?
        };
        return at(date, time);
    }
    if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(&s, "%Y-%m-%d %H:%M") {
        return Local.from_local_datetime(&dt).single();
    }
    if let Ok(date) = NaiveDate::parse_from_str(&s, "%Y-%m-%d") {
        return at(date, NaiveTime::from_hms_opt(9, 0, 0)?);
    }
    let time = time_of(&s)?;
    let today = at(now.date_naive(), time)?;
    Some(if today > now {
        today
    } else {
        at(now.date_naive() + ChronoDuration::days(1), time)?
    })
}

fn set_reminder(win: &mut SlateWindow, when: &str, cx: &mut Context<SlateWindow>) {
    let Some(at) = parse_when(when, Local::now()) else {
        notify_status(win, format!("could not read the time: {when}"), cx);
        return;
    };
    let line = win.host.doc.cursor_line;
    let mark = note_session::LineReminderGhost {
        remind_at_ms: at.timestamp_millis(),
        display_at: at.format("%b %-d, %H:%M").to_string(),
        line_text: win.host.doc.lines()[line].clone(),
        reminded_at_ms: None,
    };
    if win
        .host
        .session
        .set_reminder(&win.host.doc, line, Some(mark), true)
    {
        let before = win.snapshot_cursor();
        win.after_input(before, InputOutcome::default(), cx);
        win.save(cx);
        notify_status(
            win,
            format!("reminder set for {}", at.format("%b %-d, %H:%M")),
            cx,
        );
    } else {
        notify_status(win, "a reminder needs an editable note", cx);
    }
}

fn clear_reminder(win: &mut SlateWindow, cx: &mut Context<SlateWindow>) {
    let line = win.host.doc.cursor_line;
    if win
        .host
        .session
        .set_reminder(&win.host.doc, line, None, true)
    {
        let before = win.snapshot_cursor();
        win.after_input(before, InputOutcome::default(), cx);
        win.save(cx);
        notify_status(win, "reminder removed", cx);
    }
}

/// Run a registered script on a background thread; its result goes through
/// the session's script ticket, so a stale result never overwrites edits.
fn run_script(win: &mut SlateWindow, args: &str, cx: &mut Context<SlateWindow>) {
    let parsed = match app_core::scripts::parse_arguments(args) {
        Ok(parsed) if !parsed.is_empty() => parsed,
        Ok(_) => return notify_status(win, "usage: run <name> [args]", cx),
        Err(err) => return notify_status(win, err, cx),
    };
    let config = match app_core::config::load_script_config() {
        Ok(config) => config,
        Err(err) => return notify_status(win, err, cx),
    };
    let Some(script) = config.scripts.get(&parsed[0]).cloned() else {
        let names: Vec<_> = config.scripts.keys().cloned().collect();
        let known = if names.is_empty() {
            "none registered; see docs/scripting.md".to_string()
        } else {
            names.join(", ")
        };
        return notify_status(
            win,
            format!("no script {}; scripts: {known}", parsed[0]),
            cx,
        );
    };
    let Some((ticket, request)) = win.host.script_request(&script, parsed[1..].to_vec()) else {
        return notify_status(win, "this script needs a selection", cx);
    };
    let name = parsed[0].clone();
    notify_status(win, format!("running {name}…"), cx);
    let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let task = cx
        .background_executor()
        .spawn(async move { app_core::scripts::run_script(&script, &request, cancel) });
    cx.spawn(async move |this, cx| {
        let result = task.await;
        let _ = this.update(cx, |win, cx| match result {
            Ok(response) => {
                let before = win.snapshot_cursor();
                match win.host.apply_script(&ticket, &response) {
                    Ok(outcome) => {
                        win.after_input(before, outcome, cx);
                        let msg = response.message.clone().unwrap_or(format!("{name} done"));
                        notify_status(win, msg, cx);
                    }
                    Err(err) => notify_status(win, format!("{name}: {err}"), cx),
                }
            }
            Err(err) => notify_status(win, format!("{name} failed: {err}"), cx),
        });
    })
    .detach();
}

/// Insert an empty row above or below the cursor's table row.
pub fn table_insert_row(win: &mut SlateWindow, above: bool, cx: &mut Context<SlateWindow>) {
    let line = win.host.doc.cursor_line;
    let text = win.host.doc.lines()[line].clone();
    if !table_syntax::is_table_line(&text) {
        return notify_status(win, "not in a table", cx);
    }
    let cells = table_syntax::split_table_cells(&text).len().max(1);
    let row = format!("|{}", " |".repeat(cells));
    let at = if above || table_syntax::is_delimiter_line_in(win.host.doc.lines(), line + 1) {
        // Never between a header and its delimiter row.
        if above && table_syntax::is_delimiter_line_in(win.host.doc.lines(), line + 1) {
            line + 2
        } else if above {
            line
        } else {
            line + 2
        }
    } else {
        line + 1
    };
    let before = win.snapshot_cursor();
    let outcome = win.host.insert_lines(at, vec![row]);
    win.host.doc.cursor_line = at;
    win.host.doc.cursor_col = 2;
    win.after_input(before, outcome, cx);
}

/// Add an empty column at the right end of the cursor's table.
pub fn table_insert_column(win: &mut SlateWindow, cx: &mut Context<SlateWindow>) {
    let line = win.host.doc.cursor_line;
    let Some((start, end)) = editor_core::table::table_block_bounds(win.host.doc.lines(), line)
    else {
        return notify_status(win, "not in a table", cx);
    };
    let before = win.snapshot_cursor();
    let outcome = win.host.append_table_column(start, end);
    win.after_input(before, outcome, cx);
}

pub fn submit_prompt(win: &mut SlateWindow, mut p: Prompt, cx: &mut Context<SlateWindow>) {
    let text = std::mem::take(&mut p.text);
    match p.kind {
        PromptKind::Unlock => {
            let id = win.host.note_id().to_string();
            match win.host.db.unlock_note(&id, &text) {
                Ok(note) => {
                    win.host.open_note(note);
                    win.reload_lines();
                    notify_status(win, "unlocked", cx);
                }
                Err(err) => {
                    win.set_status(format!("unlock failed: {err}"));
                    overlays::open_prompt(win, PromptKind::Unlock, "", cx);
                }
            }
        }
        PromptKind::Encrypt if p.first.is_none() => {
            if text.is_empty() {
                return notify_status(win, "password required", cx);
            }
            p.first = Some(text);
            win.overlay = overlays::Overlay::Prompt(p);
            cx.notify();
        }
        PromptKind::Encrypt | PromptKind::Decrypt => {
            if p.kind == PromptKind::Encrypt && p.first.as_deref() != Some(text.as_str()) {
                return notify_status(win, "passwords differ; try again", cx);
            }
            if let Err(err) = win.host.save() {
                return notify_status(win, format!("save failed: {err}"), cx);
            }
            let id = win.host.note_id().to_string();
            let result = if p.kind == PromptKind::Encrypt {
                win.host.db.encrypt_note(&id, &text)
            } else {
                win.host.db.decrypt_note(&id, &text)
            };
            match result {
                Ok(note) => {
                    win.host.open_note(note);
                    win.reload_lines();
                    notify_status(
                        win,
                        if p.kind == PromptKind::Encrypt {
                            "note encrypted at rest"
                        } else {
                            "note decrypted"
                        },
                        cx,
                    );
                }
                Err(err) => notify_status(win, err, cx),
            }
        }
        PromptKind::Remind => set_reminder(win, &text, cx),
        PromptKind::Run => run_script(win, &text, cx),
        PromptKind::NewCollection => match win.host.db.create_collection(text.trim(), "") {
            Ok(c) => notify_status(win, format!("created {}", c.name), cx),
            Err(err) => notify_status(win, err, cx),
        },
        PromptKind::UnlockCollection => match win.host.db.unlock_collection(&p.extra, &text) {
            Ok(()) => {
                overlays::open_browser(win, cx);
                notify_status(win, "collection unlocked", cx);
            }
            Err(err) => {
                win.set_status(format!("unlock failed: {err}"));
                overlays::open_prompt(win, PromptKind::UnlockCollection, &p.extra, cx);
            }
        },
        PromptKind::ExportPath => {
            let format = match p.extra.as_str() {
                "txt" => ExportFormat::Txt,
                _ => ExportFormat::Md,
            };
            export(win, format, Some(text), cx);
        }
    }
}

/// Put the cursor on the heading named `heading` (as in `[[note#heading]]`).
fn jump_to_heading(win: &mut SlateWindow, heading: &str, cx: &mut Context<SlateWindow>) {
    let normalize = |value: &str| {
        value
            .trim_end_matches(|c: char| c == '#' || c.is_whitespace())
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase()
    };
    let needle = normalize(heading);
    if needle.is_empty() {
        return;
    }
    let found = win.host.doc.lines().iter().position(|line| {
        let trimmed = line.trim_start();
        trimmed.starts_with('#') && normalize(trimmed.trim_start_matches('#')) == needle
    });
    if let Some(line) = found {
        let before = win.snapshot_cursor();
        win.host.set_cursor_line(line);
        win.after_input(before, InputOutcome::default(), cx);
    }
}

/// `Ctrl+]`: open the note the wiki link under the cursor points to.
pub fn follow_link(win: &mut SlateWindow, cx: &mut Context<SlateWindow>) {
    if !follow_wiki_link(win, cx) {
        notify_status(win, "no link at the cursor", cx);
    }
}

fn follow_wiki_link(win: &mut SlateWindow, cx: &mut Context<SlateWindow>) -> bool {
    let line = win.host.doc.lines()[win.host.doc.cursor_line].clone();
    let Some(link) =
        editor_core::markdown_tokens::wiki_link_at_cursor(&line, win.host.doc.cursor_col)
    else {
        return false;
    };
    let heading = link.heading.clone();
    let destination = |title: &str| match &heading {
        Some(h) => format!("→ {title}#{h}"),
        None => format!("→ {title}"),
    };
    match win.host.db.get_note_meta(&link.note_id) {
        Ok(Some(summary)) if summary.id == win.host.note_id() => {
            // Reloading would drop unsaved edits; this text is the note.
            if let Some(h) = &heading {
                jump_to_heading(win, h, cx);
            }
            notify_status(win, destination(&summary.title), cx);
        }
        Ok(Some(summary)) => {
            win.open_note(&summary.id, cx);
            if win.host.note_id() == summary.id {
                if let Some(h) = &heading {
                    jump_to_heading(win, h, cx);
                }
                notify_status(win, destination(&summary.title), cx);
            }
        }
        Ok(None) => notify_status(win, "wiki-link: broken (note deleted)", cx),
        Err(err) => notify_status(win, format!("wiki-link error: {err}"), cx),
    }
    true
}

/// `gd`: follow the wiki link at the cursor, or jump to the definition of
/// the variable there.
pub fn go_to_definition(win: &mut SlateWindow, cx: &mut Context<SlateWindow>) {
    if follow_wiki_link(win, cx) {
        return;
    }
    let m = win.host.modules;
    let target = m.math.then(|| {
        editor_core::calc_plan::variable_definition_at(
            win.host.doc.lines(),
            win.host.session.calc().calc_dependency_index.as_ref(),
            win.host.doc.cursor_line,
            win.host.doc.cursor_col,
            editor_core::calc_plan::CalcFeatureMask {
                math_enabled: m.math,
                table_enabled: m.table,
                variables_enabled: m.variables,
            },
        )
    });
    match target.flatten() {
        Some(target) => {
            let before = win.snapshot_cursor();
            win.host.doc.cursor_line = target.line;
            win.host.doc.cursor_col = target.col;
            win.after_input(before, InputOutcome::default(), cx);
            notify_status(win, format!("definition: {}", target.name), cx);
        }
        None => notify_status(win, "no link or variable at the cursor", cx),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> chrono::DateTime<Local> {
        Local.with_ymd_and_hms(2026, 10, 10, 14, 30, 0).unwrap()
    }

    #[test]
    fn relative_and_absolute_reminder_times() {
        let n = now();
        assert_eq!(parse_when("in 2h", n), Some(n + ChronoDuration::hours(2)));
        assert_eq!(
            parse_when("in 30m", n),
            Some(n + ChronoDuration::minutes(30))
        );
        let t = parse_when("tomorrow 9am", n).unwrap();
        assert_eq!((t.day(), t.hour()), (11, 9));
        let t = parse_when("2026-10-14 09:00", n).unwrap();
        assert_eq!((t.month(), t.day(), t.hour()), (10, 14, 9));
        // A time already past today means tomorrow.
        let t = parse_when("09:00", n).unwrap();
        assert_eq!(t.day(), 11);
        let t = parse_when("5pm", n).unwrap();
        assert_eq!((t.day(), t.hour()), (10, 17));
        assert_eq!(parse_when("whenever", n), None);
    }
}
