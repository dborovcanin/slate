use super::*;
use app_core::storage::{Db, NoteModules};
use std::path::PathBuf;

struct TestDb {
    path: PathBuf,
}

impl TestDb {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("slate-mcp-test-{}.db", ulid::Ulid::new()));
        Self { path }
    }

    fn open(&self) -> Db {
        Db::open(self.path.clone()).expect("db opens")
    }

    fn server(&self) -> Server {
        Server::new(self.open(), NoteDefaults::default())
    }
}

impl Drop for TestDb {
    fn drop(&mut self) {
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", self.path.display()));
        }
    }
}

fn request(server: &mut Server, method: &str, params: Value) -> Value {
    let message = json!({ "jsonrpc": "2.0", "id": 7, "method": method, "params": params });
    server
        .handle_message(&message.to_string())
        .expect("a request is answered")
}

/// Calls a tool and returns its structured result, or its error text.
fn call(server: &mut Server, tool: &str, arguments: Value) -> Result<Value, String> {
    let response = request(
        server,
        "tools/call",
        json!({ "name": tool, "arguments": arguments }),
    );
    let result = &response["result"];
    if result["isError"] == json!(true) {
        Err(result["content"][0]["text"]
            .as_str()
            .expect("error text")
            .to_string())
    } else {
        let text = result["content"][0]["text"].as_str().expect("text content");
        assert_eq!(
            serde_json::from_str::<Value>(text).expect("text is JSON"),
            result["structuredContent"]
        );
        Ok(result["structuredContent"].clone())
    }
}

fn create(server: &mut Server, text: &str) -> Value {
    call(server, "create_note", json!({ "text": text })).expect("note created")
}

#[test]
fn initialize_agrees_on_a_protocol_version() {
    let db = TestDb::new();
    let mut server = db.server();
    let known = request(
        &mut server,
        "initialize",
        json!({ "protocolVersion": "2025-03-26", "capabilities": {}, "clientInfo": { "name": "t", "version": "1" } }),
    );
    assert_eq!(known["id"], json!(7));
    assert_eq!(known["result"]["protocolVersion"], json!("2025-03-26"));
    assert_eq!(known["result"]["serverInfo"]["name"], json!("slate"));
    assert!(known["result"]["capabilities"]["tools"].is_object());

    let unknown = request(
        &mut server,
        "initialize",
        json!({ "protocolVersion": "1999-01-01" }),
    );
    assert_eq!(
        unknown["result"]["protocolVersion"],
        json!(PROTOCOL_VERSIONS[0])
    );
}

#[test]
fn notifications_get_no_answer_and_bad_messages_get_errors() {
    let db = TestDb::new();
    let mut server = db.server();
    let initialized = json!({ "jsonrpc": "2.0", "method": "notifications/initialized" });
    assert!(server.handle_message(&initialized.to_string()).is_none());

    assert_eq!(request(&mut server, "ping", json!({}))["result"], json!({}));
    assert_eq!(
        request(&mut server, "resources/list", json!({}))["error"]["code"],
        json!(METHOD_NOT_FOUND)
    );
    let parse = server.handle_message("{not json").expect("answered");
    assert_eq!(parse["error"]["code"], json!(PARSE_ERROR));
    assert_eq!(parse["id"], Value::Null);
    let unknown_tool = request(&mut server, "tools/call", json!({ "name": "delete_note" }));
    assert_eq!(unknown_tool["error"]["code"], json!(INVALID_PARAMS));
}

#[test]
fn lists_every_tool_with_an_input_schema() {
    let db = TestDb::new();
    let mut server = db.server();
    let tools = request(&mut server, "tools/list", json!({}))["result"]["tools"].clone();
    let names = tools
        .as_array()
        .expect("tool list")
        .iter()
        .map(|tool| {
            assert_eq!(tool["inputSchema"]["type"], json!("object"));
            tool["name"].as_str().expect("name").to_string()
        })
        .collect::<Vec<_>>();
    assert_eq!(
        names,
        [
            "list_notes",
            "list_collections",
            "search_notes",
            "read_note",
            "create_note",
            "append_to_note",
            "replace_in_note",
            "update_note"
        ]
    );
}

#[test]
fn creates_and_reads_a_note() {
    let db = TestDb::new();
    let mut server = db.server();
    let created = create(&mut server, "# Groceries\n\n- milk");
    assert_eq!(created["title"], json!("Groceries"));
    let id = created["id"].as_str().expect("id");

    let read = call(&mut server, "read_note", json!({ "id": id })).expect("read");
    assert_eq!(read["text"], json!("# Groceries\n\n- milk"));
    assert_eq!(read["revision"], created["revision"]);
    assert_eq!(read["collections"], json!([]));

    let listed = call(&mut server, "list_notes", json!({})).expect("listed");
    assert_eq!(listed["notes"][0]["id"], json!(id));
    assert_eq!(listed["truncated"], json!(false));
}

#[test]
fn creates_notes_in_a_collection_by_name() {
    let db = TestDb::new();
    let mut server = db.server();
    db.open()
        .create_collection("Work", "")
        .expect("collection created");

    let created = call(
        &mut server,
        "create_note",
        json!({ "text": "# Standup", "collection": "Work" }),
    )
    .expect("created");
    let read = call(&mut server, "read_note", json!({ "id": created["id"] })).expect("read");
    assert_eq!(read["collections"], json!(["Work"]));

    let collections = call(&mut server, "list_collections", json!({})).expect("listed");
    assert_eq!(collections["collections"][0]["name"], json!("Work"));
    assert_eq!(collections["collections"][0]["notes"], json!(1));

    let in_work = call(&mut server, "list_notes", json!({ "collection": "Work" })).expect("listed");
    assert_eq!(in_work["notes"].as_array().expect("notes").len(), 1);

    let missing = call(
        &mut server,
        "create_note",
        json!({ "text": "x", "collection": "Nope" }),
    )
    .expect_err("unknown collection");
    assert!(missing.contains("collection not found"));
}

#[test]
fn list_notes_honors_the_limit() {
    let db = TestDb::new();
    let mut server = db.server();
    for title in ["# One", "# Two", "# Three"] {
        create(&mut server, title);
    }
    let listed = call(&mut server, "list_notes", json!({ "limit": 2 })).expect("listed");
    assert_eq!(listed["notes"].as_array().expect("notes").len(), 2);
    assert_eq!(listed["truncated"], json!(true));
    let bad = call(&mut server, "list_notes", json!({ "limit": 0 })).expect_err("bad limit");
    assert!(bad.contains("positive integer"));
}

#[test]
fn appends_on_a_new_line() {
    let db = TestDb::new();
    let mut server = db.server();
    let id = create(&mut server, "# Log")["id"].clone();
    let appended = call(
        &mut server,
        "append_to_note",
        json!({ "id": id, "text": "- shipped it" }),
    )
    .expect("appended");
    let read = call(&mut server, "read_note", json!({ "id": id })).expect("read");
    assert_eq!(read["text"], json!("# Log\n- shipped it"));
    assert_eq!(read["revision"], appended["revision"]);
}

#[test]
fn update_needs_the_current_revision() {
    let db = TestDb::new();
    let mut server = db.server();
    let created = create(&mut server, "# Plan\nold");
    let id = created["id"].clone();
    let first = call(
        &mut server,
        "update_note",
        json!({ "id": id, "text": "# Plan\nnew", "revision": created["revision"] }),
    )
    .expect("updated");
    assert_ne!(first["revision"], created["revision"]);

    // The revision read before the first update is stale now.
    let stale = call(
        &mut server,
        "update_note",
        json!({ "id": id, "text": "# Plan\nlost", "revision": created["revision"] }),
    )
    .expect_err("stale revision");
    assert!(stale.contains("changed since that revision"), "{stale}");
    let read = call(&mut server, "read_note", json!({ "id": id })).expect("read");
    assert_eq!(read["text"], json!("# Plan\nnew"));
}

#[test]
fn replaces_text_that_occurs_once() {
    let db = TestDb::new();
    let mut server = db.server();
    let created = create(&mut server, "# Todo\n- [ ] a\n- [ ] b\n- [ ] a");
    let id = created["id"].clone();

    let twice = call(
        &mut server,
        "replace_in_note",
        json!({ "id": id, "old_text": "- [ ] a", "new_text": "- [x] a" }),
    )
    .expect_err("ambiguous");
    assert!(twice.contains("occurs 2 times"));
    let missing = call(
        &mut server,
        "replace_in_note",
        json!({ "id": id, "old_text": "- [ ] c", "new_text": "" }),
    )
    .expect_err("missing");
    assert!(missing.contains("not found"));

    call(
        &mut server,
        "replace_in_note",
        json!({ "id": id, "old_text": "- [ ] b", "new_text": "- [x] b", "revision": created["revision"] }),
    )
    .expect("replaced");
    let read = call(&mut server, "read_note", json!({ "id": id })).expect("read");
    assert_eq!(read["text"], json!("# Todo\n- [ ] a\n- [x] b\n- [ ] a"));

    let stale = call(
        &mut server,
        "replace_in_note",
        json!({ "id": id, "old_text": "# Todo", "new_text": "# Done", "revision": created["revision"] }),
    )
    .expect_err("stale revision");
    assert!(stale.contains("changed since that revision"));
}

#[test]
fn writes_never_create_notes_or_reach_files() {
    let db = TestDb::new();
    let mut server = db.server();
    let missing = call(
        &mut server,
        "update_note",
        json!({ "id": "nope", "text": "x", "revision": "r" }),
    )
    .expect_err("missing note");
    assert!(missing.contains("note not found"));
    let missing = call(
        &mut server,
        "append_to_note",
        json!({ "id": "nope", "text": "x" }),
    )
    .expect_err("missing note");
    assert!(missing.contains("note not found"));
    assert!(db.open().get_note("nope").expect("lookup").is_none());

    let file = call(
        &mut server,
        "read_note",
        json!({ "id": "mdfile:L2V0Yy9wYXNzd2Q" }),
    )
    .expect_err("file note");
    assert!(file.contains("file-backed"));
}

#[test]
fn encrypted_notes_stay_closed() {
    let db = TestDb::new();
    let id = {
        let writer = db.open();
        let note = writer
            .create_note_with_context("secret", NoteModules::default(), Some("pw"), None)
            .expect("encrypted note");
        writer.save_note("secret", "# Secret").expect("saved");
        note.id
    };
    // A fresh server has never unlocked it, like a separate process.
    let mut server = db.server();
    for (tool, args) in [
        ("read_note", json!({ "id": id })),
        ("append_to_note", json!({ "id": id, "text": "x" })),
        (
            "update_note",
            json!({ "id": id, "text": "x", "revision": "r" }),
        ),
    ] {
        let error = call(&mut server, tool, args).expect_err("locked");
        assert!(error.contains("encrypted"), "{tool}: {error}");
    }
    let listed = call(&mut server, "list_notes", json!({})).expect("listed");
    assert_eq!(listed["notes"][0]["encrypted"], json!(true));
}

#[test]
fn searches_note_contents() {
    let db = TestDb::new();
    let mut server = db.server();
    let id = create(&mut server, "# Trip\npack the umbrella")["id"].clone();
    create(&mut server, "# Other\nnothing here");
    let found = call(&mut server, "search_notes", json!({ "query": "umbrella" })).expect("found");
    let results = found["results"].as_array().expect("results");
    assert_eq!(results.len(), 1);
    assert_eq!(results[0]["id"], id);
    assert_eq!(results[0]["line"], json!(2));
}

#[test]
fn serve_answers_line_by_line() {
    let db = TestDb::new();
    let mut server = db.server();
    let input = concat!(
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18"}}"#,
        "\n",
        r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
        "\n\n",
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#,
        "\n",
    );
    let mut output = Vec::new();
    serve(&mut server, input.as_bytes(), &mut output).expect("served");
    let lines = String::from_utf8(output).expect("utf8");
    let responses = lines.lines().collect::<Vec<_>>();
    assert_eq!(responses.len(), 2);
    let first: Value = serde_json::from_str(responses[0]).expect("json");
    assert_eq!(first["id"], json!(1));
    let second: Value = serde_json::from_str(responses[1]).expect("json");
    assert_eq!(second["id"], json!(2));
}
