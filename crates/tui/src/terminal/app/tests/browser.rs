use super::*;
use crate::terminal::browser::{Level, Scope};

fn type_text(app: &mut TerminalApp, db: &Db, text: &str) {
    for ch in text.chars() {
        app.handle_key(db, Key::Char(ch)).expect("key");
    }
}

fn hover_note(app: &mut TerminalApp, db: &Db, note_id: &str) {
    let pos = app.browser.note_position(note_id).expect("note listed");
    app.browser.note_cursor = pos;
    app.handle_key(db, Key::Char('R')).expect("refresh preview");
}

/// Browser opened from normal mode on `n1`, with notes `n2` and `n3` saved.
fn browser_app() -> (Db, TerminalApp, PathBuf) {
    let (db, mut app, path) = app_with_linked_notes(
        "# Budget\nrent := 1200\n",
        &[
            ("n2", "# Meeting notes\n- agenda"),
            ("n3", "Groceries\nmilk"),
        ],
    );
    app.mode = UiMode::Normal;
    app.handle_key(&db, Key::Ctrl('b')).expect("open browser");
    assert_eq!(app.mode, UiMode::Browser);
    (db, app, path)
}

#[test]
fn browser_opens_on_all_notes_with_the_active_note_hovered() {
    let (db, mut app, path) = browser_app();
    assert_eq!(app.browser.level(), Level::Notes);
    assert_eq!(app.browser.scope, Some(Scope::All));
    assert_eq!(
        app.browser.hovered_note().map(|n| n.id.as_str()),
        Some("n1")
    );

    let (rows, cursor) = render_screen(&mut app);
    let screen = rows.join("\n");
    assert!(rows[0].contains("Slate") && rows[0].contains("All notes"));
    for text in [
        "Unsorted",
        "Budget",
        "Meeting notes",
        "Groceries",
        "rent := 1200",
    ] {
        assert!(screen.contains(text), "missing {text}:\n{screen}");
    }
    let hints = rows.last().unwrap();
    assert!(hints.contains("space mark") && hints.contains("y yank"));
    assert!(!hints.contains("d leave"), "nothing to leave in All notes");
    assert!(!cursor.visible);

    app.handle_key(&db, Key::Char('q')).expect("close");
    assert_eq!(app.mode, UiMode::Normal);
    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn yank_into_new_collection_then_move_and_undo() {
    let (db, mut app, path) = browser_app();

    // New collection from the collections level.
    app.handle_key(&db, Key::Char('h')).expect("up");
    assert_eq!(app.browser.level(), Level::Collections);
    app.handle_key(&db, Key::Char('n')).expect("new collection");
    type_text(&mut app, &db, "Work");
    app.handle_key(&db, Key::Enter).expect("create");
    let work = db
        .get_collection_by_name("Work")
        .expect("lookup")
        .expect("created");
    assert_eq!(
        app.browser.hovered_collection().map(|e| e.scope.clone()),
        Some(Scope::Collection(work.id.clone()))
    );

    // Yank n2 from All notes, paste onto the hovered collection.
    app.handle_key(&db, Key::Char('g')).expect("g");
    app.handle_key(&db, Key::Char('g')).expect("gg");
    app.handle_key(&db, Key::Enter).expect("enter all notes");
    hover_note(&mut app, &db, "n2");
    app.handle_key(&db, Key::Char('y')).expect("yank");
    app.handle_key(&db, Key::Char('h')).expect("up");
    let work_pos = app
        .browser
        .collection_matches
        .iter()
        .position(|idx| app.browser.collections[*idx].scope == Scope::Collection(work.id.clone()))
        .expect("work listed");
    app.browser.collection_cursor = work_pos;
    app.handle_key(&db, Key::Char('p')).expect("paste");
    assert_eq!(
        db.get_note_collection_ids("n2").expect("ids"),
        vec![work.id.clone()]
    );
    assert!(app.browser.clipboard.is_some(), "yank keeps the clipboard");
    assert_eq!(
        app.browser.message.as_deref(),
        Some("1 note joined Work · u undo")
    );

    // Leave it again from inside the collection, then undo.
    app.handle_key(&db, Key::Enter).expect("enter work");
    assert_eq!(app.browser.notes.len(), 1);
    let (rows, _) = render_screen(&mut app);
    assert!(rows.last().unwrap().contains("d leave"));
    app.handle_key(&db, Key::Char('d')).expect("leave");
    assert_eq!(
        app.browser.message.as_deref(),
        Some("1 note left Work · u undo")
    );
    assert!(db.get_note_collection_ids("n2").expect("ids").is_empty());
    assert!(app.browser.notes.is_empty());
    app.handle_key(&db, Key::Char('u')).expect("undo");
    assert_eq!(
        db.get_note_collection_ids("n2").expect("ids"),
        vec![work.id.clone()]
    );

    // Marked notes cut from Unsorted land in Work.
    app.handle_key(&db, Key::Char('h')).expect("up");
    app.browser.collection_cursor = 1;
    app.handle_key(&db, Key::Enter).expect("enter unsorted");
    assert_eq!(app.browser.scope, Some(Scope::Unsorted));
    app.handle_key(&db, Key::Ctrl('a')).expect("mark all");
    app.handle_key(&db, Key::Char('x')).expect("cut");
    app.handle_key(&db, Key::Char('h')).expect("up");
    app.browser.collection_cursor = work_pos;
    app.handle_key(&db, Key::Char('p')).expect("paste");
    let counts = db.collection_note_counts().expect("counts");
    assert_eq!(counts.unsorted, 0);
    assert!(app.browser.clipboard.is_none(), "cut clears the clipboard");

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn rename_pins_titles_without_touching_the_text() {
    let (db, mut app, path) = browser_app();

    hover_note(&mut app, &db, "n2");
    app.handle_key(&db, Key::Char('r')).expect("rename");
    for _ in 0.."Meeting notes".len() {
        app.handle_key(&db, Key::Backspace).expect("erase");
    }
    type_text(&mut app, &db, "Standup");
    app.handle_key(&db, Key::Enter).expect("submit");
    let n2 = db.get_note("n2").expect("lookup").expect("note");
    assert_eq!(n2.body, "# Meeting notes\n- agenda");
    assert_eq!(n2.pinned_title.as_deref(), Some("Standup"));

    hover_note(&mut app, &db, "n1");
    app.handle_key(&db, Key::Char('r')).expect("rename");
    app.handle_key(&db, Key::Ctrl('u')).expect("clear");
    type_text(&mut app, &db, "Budget 2027");
    app.handle_key(&db, Key::Enter).expect("submit");
    assert_eq!(app.editor.lines[0], "# Budget");
    assert!(!app.dirty);
    assert_eq!(app.active_note.pinned_title.as_deref(), Some("Budget 2027"));
    assert!(app
        .browser
        .notes
        .iter()
        .any(|note| note.id == "n1" && note.title == "Budget 2027"));

    // The open note's title bar shows the pinned title.
    app.handle_key(&db, Key::Char('q')).expect("close");
    let (rows, _) = render_screen(&mut app);
    assert!(rows[0].contains("Budget 2027"), "{}", rows[0]);

    // An empty title follows the first line again.
    app.handle_key(&db, Key::Ctrl('b')).expect("browser");
    hover_note(&mut app, &db, "n1");
    app.handle_key(&db, Key::Char('r')).expect("rename");
    app.handle_key(&db, Key::Ctrl('u')).expect("clear");
    app.handle_key(&db, Key::Enter).expect("submit");
    assert_eq!(app.active_note.pinned_title, None);
    assert!(app
        .browser
        .notes
        .iter()
        .any(|note| note.id == "n1" && note.title == "Budget"));

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn new_note_lands_in_the_open_collection_and_opens_on_enter() {
    let (db, mut app, path) = browser_app();
    let work = db.create_collection("Work", "").expect("collection");
    app.handle_key(&db, Key::Char('R')).expect("reload");
    app.handle_key(&db, Key::Char('h')).expect("up");
    app.browser.collection_cursor = app
        .browser
        .collection_matches
        .iter()
        .position(|idx| app.browser.collections[*idx].scope == Scope::Collection(work.id.clone()))
        .expect("listed");
    app.handle_key(&db, Key::Char('l')).expect("enter");
    app.handle_key(&db, Key::Char('n')).expect("new note");
    type_text(&mut app, &db, "Roadmap");
    app.handle_key(&db, Key::Enter).expect("create");

    let note = app
        .browser
        .hovered_note()
        .cloned()
        .expect("new note hovered");
    assert_eq!(note.title, "Roadmap");
    assert_eq!(
        db.get_note_collection_ids(&note.id).expect("ids"),
        vec![work.id.clone()]
    );
    app.handle_key(&db, Key::Enter).expect("open");
    assert_eq!(app.active_note.id, note.id);
    assert_eq!(app.mode, UiMode::Normal);

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn filter_delete_and_minus_key() {
    let (db, mut app, path) = browser_app();
    app.handle_key(&db, Key::Char('/')).expect("filter");
    type_text(&mut app, &db, "groc");
    assert_eq!(app.browser.note_matches.len(), 1);
    app.handle_key(&db, Key::Enter).expect("keep filter");
    assert!(app.browser.prompt.is_none());
    app.handle_key(&db, Key::Char('D')).expect("delete");
    assert!(app.browser.confirm.is_some());
    let (rows, _) = render_screen(&mut app);
    assert!(rows.iter().any(|row| row.contains("Delete \"Groceries\"?")));
    app.handle_key(&db, Key::Char('y')).expect("confirm");
    assert!(db.get_note("n3").expect("lookup").is_none());
    app.handle_key(&db, Key::Esc).expect("clear filter");
    assert!(!app.browser.has_filter());
    app.handle_key(&db, Key::Esc).expect("close");
    assert_eq!(app.mode, UiMode::Normal);

    // `-` opens the browser, but `dt-` still deletes up to a dash.
    app.editor.lines = vec!["a-b".to_string()];
    app.editor.cursor_line = 0;
    app.editor.cursor_col = 0;
    type_text(&mut app, &db, "dt-");
    assert_eq!(app.mode, UiMode::Normal);
    assert_eq!(app.editor.lines[0], "-b");
    app.handle_key(&db, Key::Char('-')).expect("-");
    assert_eq!(app.mode, UiMode::Browser);

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

/// Prints the browser screen; run with `--ignored --nocapture` to look at it.
#[test]
#[ignore]
fn print_browser_screen() {
    let (db, mut app, path) = browser_app();
    let work = db
        .create_collection("Work", "Things for the day job")
        .expect("collection");
    db.add_notes_to_collection(&work.id, &["n1".to_string(), "n2".to_string()])
        .expect("added");
    app.handle_key(&db, Key::Char('R')).expect("reload");
    let (rows, _) = render_screen(&mut app);
    println!("{}", rows.join("\n"));
    app.handle_key(&db, Key::Char('h')).expect("up");
    let (rows, _) = render_screen(&mut app);
    println!("{}", rows.join("\n"));
    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

/// Runs the pending browser search now and waits for its hits.
fn finish_search(app: &mut TerminalApp, db: &Db) {
    app.browser_search_due = None;
    app.poll_browser_search(db);
    let deadline = Instant::now() + Duration::from_secs(5);
    while app.browser_search_rx.is_some() {
        assert!(Instant::now() < deadline, "search timed out");
        std::thread::sleep(Duration::from_millis(5));
        app.poll_browser_search(db);
    }
}

#[test]
fn content_search_lists_hits_previews_the_line_and_opens_there() {
    let (db, mut app, path) = browser_app();
    app.handle_key(&db, Key::Ctrl('f')).expect("search");
    assert_eq!(app.browser.level(), Level::Search);
    let (rows, cursor) = render_screen(&mut app);
    assert!(rows.iter().any(|row| row.contains("search note text")));
    assert!(cursor.visible);

    type_text(&mut app, &db, "agenda");
    finish_search(&mut app, &db);
    let search = app.browser.search.as_ref().expect("search open");
    assert_eq!(search.hits.len(), 1);
    assert_eq!(search.hits[0].note.id, "n2");
    assert_eq!(search.hits[0].line_number, 2);
    let (rows, _) = render_screen(&mut app);
    let screen = rows.join("\n");
    assert!(screen.contains(":2"), "{screen}");
    assert!(screen.contains("- agenda") || screen.contains("agenda"));
    assert!(rows.last().unwrap().contains("open at match"));

    // `j` is typed into the query, not a motion.
    app.handle_key(&db, Key::Char('j')).expect("type j");
    assert_eq!(app.browser.search.as_ref().unwrap().query, "agendaj");
    app.handle_key(&db, Key::Backspace).expect("erase");
    finish_search(&mut app, &db);

    // New hits arrive unselected; Enter opens only a picked hit.
    assert_eq!(app.browser.search.as_ref().unwrap().selected, None);
    app.handle_key(&db, Key::Enter).expect("nothing picked");
    assert_eq!(app.mode, UiMode::Browser);
    app.handle_key(&db, Key::ArrowDown).expect("pick");
    app.handle_key(&db, Key::Enter).expect("open hit");
    assert_eq!(app.active_note.id, "n2");
    assert_eq!(app.editor.cursor_line, 1);
    assert_eq!(app.mode, UiMode::Normal);

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn content_search_stays_in_the_open_collection_and_esc_returns() {
    let (db, mut app, path) = browser_app();
    let work = db.create_collection("Work", "").expect("collection");
    db.add_notes_to_collection(&work.id, &["n1".to_string()])
        .expect("added");
    app.handle_key(&db, Key::Char('R')).expect("reload");
    app.handle_key(&db, Key::Char('h')).expect("up");
    app.browser.collection_cursor = app
        .browser
        .collection_matches
        .iter()
        .position(|idx| app.browser.collections[*idx].scope == Scope::Collection(work.id.clone()))
        .expect("listed");
    app.handle_key(&db, Key::Enter).expect("enter work");

    // Ctrl+/ as legacy terminals send it (0x1F, Ctrl+_).
    app.handle_key(&db, Key::Ctrl('_')).expect("search");
    type_text(&mut app, &db, "agenda");
    finish_search(&mut app, &db);
    assert!(app.browser.search.as_ref().unwrap().hits.is_empty());
    let (rows, _) = render_screen(&mut app);
    assert!(rows.iter().any(|row| row.contains("no matches")));

    app.handle_key(&db, Key::Ctrl('u')).expect("clear");
    type_text(&mut app, &db, "rent");
    finish_search(&mut app, &db);
    let hits = &app.browser.search.as_ref().unwrap().hits;
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].note.id, "n1");

    app.handle_key(&db, Key::Esc).expect("back");
    assert_eq!(app.browser.level(), Level::Notes);
    assert_eq!(app.browser.scope, Some(Scope::Collection(work.id.clone())));
    assert!(app.browser.search.is_none());

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn note_and_collection_pickers_are_popups_with_browser_rows() {
    let (db, mut app, path) = browser_app();
    app.handle_key(&db, Key::Esc).expect("close browser");
    let work = db.create_collection("Work", "day job").expect("collection");
    db.add_notes_to_collection(&work.id, &["n2".to_string()])
        .expect("added");

    app.handle_key(&db, Key::Ctrl('p')).expect("switcher");
    assert_eq!(app.mode, UiMode::Switcher);
    type_text(&mut app, &db, "meet");
    let (rows, cursor) = render_screen(&mut app);
    let screen = rows.join("\n");
    assert!(rows[2].contains("Notes · all notes"), "{screen}");
    assert!(screen.contains("Meeting notes"));
    assert!(!screen.contains("- agenda"), "no preview:\n{screen}");
    assert!(!screen.contains(" n2 "), "note ids are not shown");
    assert!(screen.contains("Tab search text"));
    // Editor title bar still shows behind the popup.
    assert!(rows[0].contains("Budget"));
    assert_eq!((cursor.row, cursor.col), (3, 6 + 4));

    app.handle_key(&db, Key::Esc).expect("close");
    app.handle_key(&db, Key::Ctrl('g')).expect("collections");
    assert_eq!(app.mode, UiMode::CollectionSwitcher);
    type_text(&mut app, &db, "work");
    let (rows, _) = render_screen(&mut app);
    let screen = rows.join("\n");
    assert!(rows[2].contains("Collections"));
    assert!(screen.contains("Work"));
    assert!(!screen.contains("Meeting notes"), "no preview:\n{screen}");
    app.handle_key(&db, Key::Enter).expect("work in");
    assert_eq!(app.working_collection_id.as_deref(), Some(work.id.as_str()));

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn history_lists_versions_previews_changes_and_restores_another_note() {
    let (db, mut app, path) = browser_app();
    db.save_note("n2", "# Meeting notes\n- agenda\n- budget")
        .expect("edit n2");
    app.handle_key(&db, Key::Char('R')).expect("reload");
    hover_note(&mut app, &db, "n2");
    app.handle_key(&db, Key::Char('H')).expect("history");
    assert_eq!(app.browser.level(), Level::History);
    let history = app.browser.history.as_ref().expect("history open");
    assert_eq!(history.versions.len(), 1);
    assert_eq!(
        (
            history.versions[0].lines_added,
            history.versions[0].lines_removed
        ),
        (1, 0)
    );

    app.handle_key(&db, Key::Char('j')).expect("select version");
    let (rows, _) = render_screen(&mut app);
    let screen = rows.join("\n");
    assert!(
        rows[0].contains("Meeting notes") && rows[0].contains("history"),
        "{screen}"
    );
    assert!(
        screen.contains("- - budget"),
        "restore removes the new line:\n{screen}"
    );
    assert!(rows.last().unwrap().contains("restore"));

    app.handle_key(&db, Key::Tab).expect("text");
    let (rows, _) = render_screen(&mut app);
    assert!(!rows.join("\n").contains("budget"));

    app.handle_key(&db, Key::Enter).expect("restore");
    assert!(app.browser.confirm.is_some());
    app.handle_key(&db, Key::Char('y')).expect("confirm");
    assert_eq!(
        db.get_note("n2").unwrap().unwrap().body,
        "# Meeting notes\n- agenda"
    );
    // The text before the restore is a version now, so it can be undone.
    let versions = &app.browser.history.as_ref().unwrap().versions;
    assert_eq!(versions.len(), 2);
    assert_eq!(
        db.note_version_text("n2", versions[0].id).unwrap(),
        "# Meeting notes\n- agenda\n- budget"
    );

    app.handle_key(&db, Key::Char('h')).expect("back");
    assert_eq!(app.browser.level(), Level::Notes);
    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn restoring_the_open_note_edits_its_buffer_and_history_opens_from_everywhere() {
    let (db, mut app, path) = browser_app();
    app.handle_key(&db, Key::Esc).expect("close browser");
    app.editor.lines = vec!["# Budget".into(), "rent := 1500".into()];
    app.dirty = true;
    app.save(&db).expect("save");

    app.execute_terminal_command(&db, "history");
    assert_eq!(app.mode, UiMode::Browser);
    assert_eq!(app.browser.level(), Level::History);
    assert_eq!(
        app.browser.history.as_ref().map(|h| h.note.id.as_str()),
        Some("n1")
    );
    app.handle_key(&db, Key::Char('G')).expect("oldest");
    app.handle_key(&db, Key::Enter).expect("restore");
    app.handle_key(&db, Key::Char('y')).expect("confirm");
    assert_eq!(app.editor.lines, vec!["# Budget", "rent := 1200", ""]);
    assert!(db.get_note("n1").unwrap().unwrap().body.contains("1200"));
    app.handle_key(&db, Key::Char('q')).expect("close");

    // Ctrl+R in the note switcher opens the selected note's history.
    app.handle_key(&db, Key::Ctrl('p')).expect("switcher");
    type_text(&mut app, &db, "meeting");
    app.handle_key(&db, Key::ArrowDown).expect("pick");
    app.handle_key(&db, Key::Ctrl('r')).expect("history");
    assert_eq!(app.mode, UiMode::Browser);
    assert_eq!(
        app.browser.history.as_ref().map(|h| h.note.id.as_str()),
        Some("n2")
    );
    let (rows, _) = render_screen(&mut app);
    assert!(rows.join("\n").contains("no older versions yet"));

    // Enter on the current text opens the note.
    app.handle_key(&db, Key::Enter).expect("open current");
    assert_eq!(app.mode, UiMode::Normal);
    assert_eq!(app.active_note.id, "n2");

    // Ctrl+R in the browser's note list opens the hovered note's history.
    app.handle_key(&db, Key::Ctrl('b')).expect("browser");
    hover_note(&mut app, &db, "n1");
    app.handle_key(&db, Key::Ctrl('r')).expect("history");
    assert_eq!(app.browser.level(), Level::History);
    assert_eq!(
        app.browser.history.as_ref().map(|h| h.note.id.as_str()),
        Some("n1")
    );
    app.handle_key(&db, Key::Char('q')).expect("close");

    // Ctrl+R in insert mode opens the open note's history.
    app.handle_key(&db, Key::Char('i')).expect("insert");
    assert_eq!(app.mode, UiMode::Editor);
    app.handle_key(&db, Key::Ctrl('r')).expect("history");
    assert_eq!(app.browser.level(), Level::History);
    assert_eq!(
        app.browser.history.as_ref().map(|h| h.note.id.as_str()),
        Some("n2")
    );

    drop(app);
    drop(db);
    cleanup_db_files(&path);
}
