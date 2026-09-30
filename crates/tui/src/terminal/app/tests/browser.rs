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
    assert!(rows.last().unwrap().contains("space mark"));
    assert!(!cursor.visible);

    app.handle_key(&db, Key::Char('q')).expect("close");
    assert_eq!(app.mode, UiMode::Normal);
    drop(app);
    drop(db);
    cleanup_db_files(&path);
}

#[test]
fn copy_into_new_collection_then_move_and_undo() {
    let (db, mut app, path) = browser_app();

    // New collection from the collections level.
    app.handle_key(&db, Key::Char('h')).expect("up");
    assert_eq!(app.browser.level(), Level::Collections);
    app.handle_key(&db, Key::Char('a')).expect("new collection");
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

    // Copy n2 from All notes, paste onto the hovered collection.
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
    assert!(app.browser.clipboard.is_some(), "copy keeps the clipboard");

    // Remove it again from inside the collection, then undo.
    app.handle_key(&db, Key::Enter).expect("enter work");
    assert_eq!(app.browser.notes.len(), 1);
    app.handle_key(&db, Key::Char('d')).expect("remove");
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
fn rename_rewrites_other_notes_in_the_db_and_the_open_note_in_its_buffer() {
    let (db, mut app, path) = browser_app();

    hover_note(&mut app, &db, "n2");
    app.handle_key(&db, Key::Char('r')).expect("rename");
    for _ in 0.."Meeting notes".len() {
        app.handle_key(&db, Key::Backspace).expect("erase");
    }
    type_text(&mut app, &db, "Standup");
    app.handle_key(&db, Key::Enter).expect("submit");
    let n2 = db.get_note("n2").expect("lookup").expect("note");
    assert_eq!(n2.body, "# Standup\n- agenda");

    hover_note(&mut app, &db, "n1");
    app.handle_key(&db, Key::Char('r')).expect("rename");
    app.handle_key(&db, Key::Ctrl('u')).expect("clear");
    type_text(&mut app, &db, "Budget 2027");
    app.handle_key(&db, Key::Enter).expect("submit");
    assert_eq!(app.editor.lines[0], "# Budget 2027");
    let n1 = db.get_note("n1").expect("lookup").expect("note");
    assert!(n1.body.starts_with("# Budget 2027\n"));
    assert!(app
        .browser
        .notes
        .iter()
        .any(|note| note.id == "n1" && note.title == "Budget 2027"));

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
    app.handle_key(&db, Key::Char('a')).expect("new note");
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
