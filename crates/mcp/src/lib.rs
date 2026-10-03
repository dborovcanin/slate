//! A Model Context Protocol server over stdio, so AI assistants and bots can
//! read and write Slate notes.
//!
//! The transport is newline-delimited JSON-RPC 2.0 on stdin/stdout, which is
//! all an MCP stdio server needs; it is small enough that no MCP SDK (and no
//! async runtime) is pulled in. Everything about notes lives in [`tools`],
//! on top of `app-core` storage, so the server shares the database with a
//! running editor the same way `slate append` does.

mod tools;

use serde_json::{json, Value};
use std::io::{BufRead, Write};

pub use tools::NoteDefaults;

/// Protocol revisions this server speaks, newest first. It only uses tools
/// with text results, which every revision supports alike.
const PROTOCOL_VERSIONS: &[&str] = &["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];

const INSTRUCTIONS: &str = "Slate is a markdown note-taking app. Each note has an id; its \
first line is its title (a `# Heading` line works well). Use search_notes or list_notes to \
find notes, read_note to get a note's text and revision, and append_to_note for adding to \
the end of a note. To change existing text, prefer replace_in_note (exact text, must occur \
once) over update_note, which replaces the whole note. Writes are checked against the \
revision you read: on a conflict, read the note again and redo the change. To put a note \
away, use archive_note. Encrypted notes and encrypted collections cannot be read or written.";

const PARSE_ERROR: i64 = -32700;
const INVALID_REQUEST: i64 = -32600;
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;

/// Answers MCP requests against a notes database.
pub struct Server {
    notes: tools::Notes,
}

impl Server {
    /// `allow_delete` offers `delete_note`; without it notes can only be
    /// archived.
    pub fn new(db: app_core::storage::Db, defaults: NoteDefaults, allow_delete: bool) -> Self {
        Self {
            notes: tools::Notes::new(db, defaults, allow_delete),
        }
    }

    /// Handles one JSON-RPC message and returns the response to send, if
    /// any (notifications get none).
    pub fn handle_message(&mut self, message: &str) -> Option<Value> {
        let message: Value = match serde_json::from_str(message) {
            Ok(value) => value,
            Err(error) => {
                return Some(error_response(
                    Value::Null,
                    PARSE_ERROR,
                    &format!("parse error: {error}"),
                ))
            }
        };
        let Some(object) = message.as_object() else {
            return Some(error_response(
                Value::Null,
                INVALID_REQUEST,
                "expected a JSON-RPC request object",
            ));
        };
        let id = object.get("id").cloned();
        let Some(method) = object.get("method").and_then(Value::as_str) else {
            // A response from the client (this server sends no requests) or
            // something malformed; answer only what carries an id.
            return id.map(|id| error_response(id, INVALID_REQUEST, "missing method"));
        };
        let params = object.get("params").cloned().unwrap_or(Value::Null);
        // Notifications (`notifications/initialized`, `notifications/cancelled`)
        // need no answer, and no request here runs long enough to cancel.
        let id = id?;
        Some(match self.handle_request(method, &params) {
            Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
            Err((code, message)) => error_response(id, code, &message),
        })
    }

    fn handle_request(&mut self, method: &str, params: &Value) -> Result<Value, (i64, String)> {
        match method {
            "initialize" => Ok(initialize_result(params)),
            "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({ "tools": self.notes.definitions() })),
            "tools/call" => {
                let name = params
                    .get("name")
                    .and_then(Value::as_str)
                    .ok_or((INVALID_PARAMS, "missing tool name".to_string()))?;
                let arguments = params.get("arguments").cloned().unwrap_or(json!({}));
                match self.notes.call(name, &arguments) {
                    Some(result) => Ok(tool_result(result)),
                    None => Err((INVALID_PARAMS, format!("unknown tool: {name}"))),
                }
            }
            _ => Err((METHOD_NOT_FOUND, format!("method not found: {method}"))),
        }
    }
}

/// Serves MCP over `input`/`output` (stdin/stdout) until `input` closes.
pub fn serve(
    server: &mut Server,
    input: impl BufRead,
    mut output: impl Write,
) -> std::io::Result<()> {
    for line in input.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        if let Some(response) = server.handle_message(&line) {
            serde_json::to_writer(&mut output, &response)?;
            output.write_all(b"\n")?;
            output.flush()?;
        }
    }
    Ok(())
}

fn initialize_result(params: &Value) -> Value {
    let requested = params.get("protocolVersion").and_then(Value::as_str);
    let version = requested
        .filter(|version| PROTOCOL_VERSIONS.contains(version))
        .unwrap_or(PROTOCOL_VERSIONS[0]);
    json!({
        "protocolVersion": version,
        "capabilities": { "tools": {} },
        "serverInfo": { "name": "slate", "version": env!("CARGO_PKG_VERSION") },
        "instructions": INSTRUCTIONS,
    })
}

/// A finished tool call: its JSON as text (what every client shows the
/// model) and as structured content; a failure is reported as a tool error
/// so the model sees why.
fn tool_result(result: Result<Value, String>) -> Value {
    match result {
        Ok(value) => json!({
            "content": [{ "type": "text", "text": value.to_string() }],
            "structuredContent": value,
            "isError": false,
        }),
        Err(error) => json!({
            "content": [{ "type": "text", "text": error }],
            "isError": true,
        }),
    }
}

fn error_response(id: Value, code: i64, message: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": code, "message": message },
    })
}

#[cfg(test)]
mod tests;
