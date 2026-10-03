//! The MCP tools: what a client may do with notes, on top of `app-core`.
//!
//! Every write goes through the same storage calls as the editor, with its
//! revision check, so a write never lands on text the client has not seen
//! and an editor holding the note notices the change. File-backed notes
//! (`mdfile:` ids) are not reachable, so a client cannot read or write
//! arbitrary files through the server, and deleting is only offered when
//! `[mcp] allow_delete` is on.

use app_core::config::NoteSecurityConfig;
use app_core::note_sources::MARKDOWN_NOTE_ID_PREFIX;
use app_core::storage::{Db, Note, NoteAccessMode, NoteModules, NoteSummary};
use serde_json::{json, Map, Value};
use ulid::Ulid;

const DEFAULT_LIST_LIMIT: usize = 50;
const MAX_LIST_LIMIT: usize = 500;
const DEFAULT_SEARCH_LIMIT: usize = 20;
/// Collection `archive_note` moves notes into, created when first needed.
const ARCHIVE_COLLECTION: &str = "Archive";

/// What a note created over MCP starts with, as for one created in the
/// editor: `[editor.modules]` and `[editor.security]`.
#[derive(Debug, Clone, Default)]
pub struct NoteDefaults {
    pub modules: NoteModules,
    pub security: NoteSecurityConfig,
}

pub(crate) struct Notes {
    db: Db,
    defaults: NoteDefaults,
    allow_delete: bool,
}

/// The tools `tools/list` advertises; `delete_note` only with `allow_delete`.
fn definitions(allow_delete: bool) -> Value {
    let mut tools = json!([
        {
            "name": "list_notes",
            "description": "List notes, most recently updated first.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "collection": { "type": "string", "description": "Only notes in this collection (by name)." },
                    "limit": { "type": "integer", "minimum": 1, "maximum": MAX_LIST_LIMIT, "description": "Most notes to return (default 50)." }
                }
            },
            "annotations": { "readOnlyHint": true }
        },
        {
            "name": "list_collections",
            "description": "List note collections with their note counts.",
            "inputSchema": { "type": "object", "properties": {} },
            "annotations": { "readOnlyHint": true }
        },
        {
            "name": "search_notes",
            "description": "Full-text search over note contents. Returns matching lines with snippets (matches wrapped in [[ ]]). Encrypted notes are not searched.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Words to search for." },
                    "collection": { "type": "string", "description": "Only notes in this collection (by name)." },
                    "limit": { "type": "integer", "minimum": 1, "maximum": 100, "description": "Most matching notes to return (default 20)." }
                },
                "required": ["query"]
            },
            "annotations": { "readOnlyHint": true }
        },
        {
            "name": "read_note",
            "description": "Read a note's full markdown text and its revision. Pass the revision to update_note or replace_in_note.",
            "inputSchema": {
                "type": "object",
                "properties": { "id": { "type": "string" } },
                "required": ["id"]
            },
            "annotations": { "readOnlyHint": true }
        },
        {
            "name": "create_note",
            "description": "Create a note. Its first line becomes its title, e.g. \"# Groceries\\n\\n- milk\".",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "text": { "type": "string", "description": "Markdown text of the note." },
                    "collection": { "type": "string", "description": "Collection (by name) to put the note in." }
                },
                "required": ["text"]
            },
            "annotations": { "readOnlyHint": false, "destructiveHint": false, "idempotentHint": false }
        },
        {
            "name": "append_to_note",
            "description": "Add text to the end of a note, on a new line. Needs no revision.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": { "type": "string" },
                    "text": { "type": "string" }
                },
                "required": ["id", "text"]
            },
            "annotations": { "readOnlyHint": false, "destructiveHint": false, "idempotentHint": false }
        },
        {
            "name": "replace_in_note",
            "description": "Replace one exact piece of a note's text. old_text must occur exactly once; include surrounding text to make it unique.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": { "type": "string" },
                    "old_text": { "type": "string", "description": "Exact text to replace, as read_note returned it." },
                    "new_text": { "type": "string" },
                    "revision": { "type": "string", "description": "Revision from read_note; when given, fails if the note changed since." }
                },
                "required": ["id", "old_text", "new_text"]
            },
            "annotations": { "readOnlyHint": false, "destructiveHint": true, "idempotentHint": false }
        },
        {
            "name": "update_note",
            "description": "Replace a note's whole text. Fails if the note changed since the given revision (from read_note).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": { "type": "string" },
                    "text": { "type": "string", "description": "The note's new markdown text." },
                    "revision": { "type": "string", "description": "Revision from read_note." }
                },
                "required": ["id", "text", "revision"]
            },
            "annotations": { "readOnlyHint": false, "destructiveHint": true, "idempotentHint": true }
        },
        {
            "name": "rename_note",
            "description": "Give a note a fixed title that later edits to its text do not change. An empty title makes it follow the first line again.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": { "type": "string" },
                    "title": { "type": "string" }
                },
                "required": ["id", "title"]
            },
            "annotations": { "readOnlyHint": false, "destructiveHint": false, "idempotentHint": true }
        },
        {
            "name": "create_collection",
            "description": "Create a collection (a named group of notes).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "name": { "type": "string" },
                    "description": { "type": "string" }
                },
                "required": ["name"]
            },
            "annotations": { "readOnlyHint": false, "destructiveHint": false, "idempotentHint": false }
        },
        {
            "name": "add_to_collection",
            "description": "Add a note to a collection (by name). A note can be in several collections.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": { "type": "string" },
                    "collection": { "type": "string" }
                },
                "required": ["id", "collection"]
            },
            "annotations": { "readOnlyHint": false, "destructiveHint": false, "idempotentHint": true }
        },
        {
            "name": "remove_from_collection",
            "description": "Take a note out of a collection (by name). The note itself stays.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": { "type": "string" },
                    "collection": { "type": "string" }
                },
                "required": ["id", "collection"]
            },
            "annotations": { "readOnlyHint": false, "destructiveHint": false, "idempotentHint": true }
        },
        {
            "name": "archive_note",
            "description": "Put a note away: move it into the Archive collection, out of its other collections. Its text and history are kept.",
            "inputSchema": {
                "type": "object",
                "properties": { "id": { "type": "string" } },
                "required": ["id"]
            },
            "annotations": { "readOnlyHint": false, "destructiveHint": false, "idempotentHint": true }
        }
    ]);
    if allow_delete {
        tools.as_array_mut().expect("tool list").push(json!({
            "name": "delete_note",
            "description": "Delete a note for good, with its history. Fails if the note changed since the given revision (from read_note). Prefer archive_note unless asked to delete.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": { "type": "string" },
                    "revision": { "type": "string", "description": "Revision from read_note." }
                },
                "required": ["id", "revision"]
            },
            "annotations": { "readOnlyHint": false, "destructiveHint": true, "idempotentHint": true }
        }));
    }
    tools
}

impl Notes {
    pub(crate) fn new(db: Db, defaults: NoteDefaults, allow_delete: bool) -> Self {
        Self {
            db,
            defaults,
            allow_delete,
        }
    }

    pub(crate) fn definitions(&self) -> Value {
        definitions(self.allow_delete)
    }

    /// Runs tool `name`; `None` when there is no such tool.
    pub(crate) fn call(&self, name: &str, args: &Value) -> Option<Result<Value, String>> {
        let args = args.as_object().cloned().unwrap_or_default();
        Some(match name {
            "list_notes" => self.list_notes(&args),
            "list_collections" => self.list_collections(),
            "search_notes" => self.search_notes(&args),
            "read_note" => self.read_note(&args),
            "create_note" => self.create_note(&args),
            "append_to_note" => self.append_to_note(&args),
            "replace_in_note" => self.replace_in_note(&args),
            "update_note" => self.update_note(&args),
            "rename_note" => self.rename_note(&args),
            "create_collection" => self.create_collection(&args),
            "add_to_collection" => self.add_to_collection(&args),
            "remove_from_collection" => self.remove_from_collection(&args),
            "archive_note" => self.archive_note(&args),
            "delete_note" if self.allow_delete => self.delete_note(&args),
            _ => return None,
        })
    }

    fn list_notes(&self, args: &Map<String, Value>) -> Result<Value, String> {
        let collection_id = self.collection_id(args)?;
        let limit = limit_arg(args, DEFAULT_LIST_LIMIT, MAX_LIST_LIMIT)?;
        let mut notes = self.db.list_notes_meta_filtered(collection_id.as_deref())?;
        notes.retain(|note| !is_file_note_id(&note.id));
        let truncated = notes.len() > limit;
        notes.truncate(limit);
        Ok(json!({
            "notes": notes.iter().map(summary_json).collect::<Vec<_>>(),
            "truncated": truncated,
        }))
    }

    fn list_collections(&self) -> Result<Value, String> {
        let counts = self.db.collection_note_counts()?;
        let collections = self
            .db
            .list_collections()?
            .into_iter()
            .map(|collection| {
                json!({
                    "name": collection.name,
                    "description": collection.description,
                    "notes": counts.per_collection.get(&collection.id).copied().unwrap_or(0),
                    "encrypted": collection.encrypted,
                })
            })
            .collect::<Vec<_>>();
        Ok(json!({ "collections": collections }))
    }

    fn search_notes(&self, args: &Map<String, Value>) -> Result<Value, String> {
        let query = string_arg(args, "query")?;
        let collection_id = self.collection_id(args)?;
        let limit = limit_arg(args, DEFAULT_SEARCH_LIMIT, 100)?;
        let results = self
            .db
            .search_notes_content_filtered(query, limit, collection_id.as_deref())?
            .into_iter()
            .filter(|hit| !is_file_note_id(&hit.id))
            .map(|hit| {
                json!({
                    "id": hit.id,
                    "title": hit.title,
                    "line": hit.line_number,
                    "snippet": hit.snippet,
                    "revision": hit.updated_at,
                })
            })
            .collect::<Vec<_>>();
        Ok(json!({ "results": results }))
    }

    fn read_note(&self, args: &Map<String, Value>) -> Result<Value, String> {
        let id = note_id_arg(args)?;
        let note = self.readable_note(id)?;
        let title = self
            .db
            .get_note_meta(id)?
            .map(|meta| meta.title)
            .unwrap_or_default();
        let collections = self.collection_names(id)?;
        Ok(json!({
            "id": note.id,
            "title": title,
            "revision": note.updated_at,
            "collections": collections,
            "text": note.body,
        }))
    }

    fn create_note(&self, args: &Map<String, Value>) -> Result<Value, String> {
        let text = string_arg(args, "text")?;
        let collection_id = self.collection_id(args)?;
        let password =
            app_core::config::resolve_default_note_encryption_password(&self.defaults.security)?;
        let id = Ulid::new().to_string();
        let created = self.db.create_note_with_context(
            &id,
            self.defaults.modules,
            password.as_deref(),
            collection_id.as_deref(),
        )?;
        let saved = if text.is_empty() {
            Ok(created.updated_at)
        } else {
            self.db
                .save_note_revision_if(&id, text, Some(&created.updated_at), None)
                .map(|revision| revision.updated_at)
        };
        let saved = saved.and_then(|revision| Ok((revision, self.title(&id)?)));
        // A note encrypted on creation stays closed to clients from now on:
        // the key it was created with is not kept.
        self.db.lock_note(&id);
        match saved {
            Ok((revision, title)) => Ok(json!({ "id": id, "title": title, "revision": revision })),
            Err(error) => {
                // Do not leave an empty note behind.
                let _ = self.db.delete_note(&id, password.as_deref());
                Err(error)
            }
        }
    }

    fn append_to_note(&self, args: &Map<String, Value>) -> Result<Value, String> {
        let id = note_id_arg(args)?;
        let text = string_arg(args, "text")?;
        self.readable_note(id)?;
        let note = self.db.append_note_body(id, text)?;
        Ok(json!({ "id": note.id, "revision": note.updated_at }))
    }

    fn replace_in_note(&self, args: &Map<String, Value>) -> Result<Value, String> {
        let id = note_id_arg(args)?;
        let old_text = string_arg(args, "old_text")?;
        let new_text = string_arg(args, "new_text")?;
        let note = self.readable_note(id)?;
        if let Some(revision) = optional_string_arg(args, "revision")? {
            if revision != note.updated_at {
                return Err(conflict_message(&note.updated_at));
            }
        }
        if old_text.is_empty() {
            return Err("old_text must not be empty".to_string());
        }
        let body = match note.body.matches(old_text).count() {
            0 => return Err("old_text was not found in the note".to_string()),
            1 => note.body.replacen(old_text, new_text, 1),
            count => {
                return Err(format!(
                    "old_text occurs {count} times; include more surrounding text so it occurs once"
                ))
            }
        };
        self.save(id, &body, &note.updated_at)
    }

    fn update_note(&self, args: &Map<String, Value>) -> Result<Value, String> {
        let id = note_id_arg(args)?;
        let text = string_arg(args, "text")?;
        let revision = string_arg(args, "revision")?;
        self.readable_note(id)?;
        self.save(id, text, revision)
    }

    fn rename_note(&self, args: &Map<String, Value>) -> Result<Value, String> {
        let id = note_id_arg(args)?;
        let title = string_arg(args, "title")?;
        self.readable_note(id)?;
        self.db.set_note_title(id, title)?;
        Ok(json!({ "id": id, "title": self.title(id)? }))
    }

    fn create_collection(&self, args: &Map<String, Value>) -> Result<Value, String> {
        let name = string_arg(args, "name")?;
        let description = optional_string_arg(args, "description")?.unwrap_or_default();
        let collection = self.db.create_collection(name, description)?;
        Ok(json!({ "name": collection.name, "description": collection.description }))
    }

    fn add_to_collection(&self, args: &Map<String, Value>) -> Result<Value, String> {
        let id = note_id_arg(args)?;
        self.readable_note(id)?;
        let collection_id = self.writable_collection_id(string_arg(args, "collection")?)?;
        self.db
            .add_notes_to_collection(&collection_id, &[id.to_string()])?;
        Ok(json!({ "id": id, "collections": self.collection_names(id)? }))
    }

    fn remove_from_collection(&self, args: &Map<String, Value>) -> Result<Value, String> {
        let id = note_id_arg(args)?;
        self.readable_note(id)?;
        let collection_id = self.writable_collection_id(string_arg(args, "collection")?)?;
        self.db
            .remove_notes_from_collection(&collection_id, &[id.to_string()])?;
        Ok(json!({ "id": id, "collections": self.collection_names(id)? }))
    }

    fn archive_note(&self, args: &Map<String, Value>) -> Result<Value, String> {
        let id = note_id_arg(args)?;
        self.readable_note(id)?;
        let archive_id = match self.db.get_collection_by_name(ARCHIVE_COLLECTION)? {
            Some(_) => self.writable_collection_id(ARCHIVE_COLLECTION)?,
            None => {
                self.db
                    .create_collection(ARCHIVE_COLLECTION, "Notes put away")?
                    .id
            }
        };
        self.db.set_note_collections(id, &[archive_id])?;
        Ok(json!({ "id": id, "collections": self.collection_names(id)? }))
    }

    fn delete_note(&self, args: &Map<String, Value>) -> Result<Value, String> {
        let id = note_id_arg(args)?;
        let revision = string_arg(args, "revision")?;
        let note = self.readable_note(id)?;
        if note.updated_at != revision {
            return Err(conflict_message(&note.updated_at));
        }
        self.db.delete_note(id, None)?;
        Ok(json!({ "id": id, "deleted": true }))
    }

    /// Stores `body` while the note is still at `revision`.
    fn save(&self, id: &str, body: &str, revision: &str) -> Result<Value, String> {
        match self
            .db
            .save_note_revision_if(id, body, Some(revision), None)
        {
            Ok(saved) => Ok(json!({ "id": saved.id, "revision": saved.updated_at })),
            Err(error) => match self.db.get_note_updated_at(id)? {
                Some(current) if current != revision => Err(conflict_message(&current)),
                _ => Err(error),
            },
        }
    }

    /// The stored note `id`, which must exist and not be encrypted. A key
    /// this process happens to hold does not count: encrypted notes are
    /// only ever opened in Slate.
    fn readable_note(&self, id: &str) -> Result<Note, String> {
        let note = self
            .db
            .get_note(id)?
            .ok_or_else(|| format!("note not found: {id}"))?;
        if note.access_mode != NoteAccessMode::None {
            return Err(format!(
                "note {id} is encrypted; it can only be opened in Slate"
            ));
        }
        Ok(note)
    }

    fn title(&self, id: &str) -> Result<String, String> {
        Ok(self
            .db
            .get_note_meta(id)?
            .map(|meta| meta.title)
            .unwrap_or_default())
    }

    fn collection_id(&self, args: &Map<String, Value>) -> Result<Option<String>, String> {
        let Some(name) = optional_string_arg(args, "collection")? else {
            return Ok(None);
        };
        match self.db.get_collection_by_name(name)? {
            Some(collection) => Ok(Some(collection.id)),
            None => Err(format!(
                "collection not found: {name} (list_collections shows the names)"
            )),
        }
    }

    /// Collection `name`, which must exist and not be encrypted: adding a
    /// note to an encrypted collection needs its key, which only Slate has.
    fn writable_collection_id(&self, name: &str) -> Result<String, String> {
        match self.db.get_collection_by_name(name)? {
            Some(collection) if collection.encrypted => Err(format!(
                "collection {name} is encrypted; change its notes in Slate"
            )),
            Some(collection) => Ok(collection.id),
            None => Err(format!(
                "collection not found: {name} (list_collections shows the names)"
            )),
        }
    }

    fn collection_names(&self, note_id: &str) -> Result<Vec<String>, String> {
        let ids = self.db.get_note_collection_ids(note_id)?;
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        Ok(self
            .db
            .list_collections()?
            .into_iter()
            .filter(|collection| ids.contains(&collection.id))
            .map(|collection| collection.name)
            .collect())
    }
}

fn summary_json(note: &NoteSummary) -> Value {
    json!({
        "id": note.id,
        "title": note.title,
        "revision": note.updated_at,
        "encrypted": note.access_mode != NoteAccessMode::None,
    })
}

fn conflict_message(current: &str) -> String {
    format!(
        "the note changed since that revision (it is now at {current}); read it again and redo the change"
    )
}

fn is_file_note_id(id: &str) -> bool {
    id.starts_with(MARKDOWN_NOTE_ID_PREFIX)
}

fn note_id_arg(args: &Map<String, Value>) -> Result<&str, String> {
    let id = string_arg(args, "id")?;
    if is_file_note_id(id) {
        return Err("file-backed notes are not available over MCP".to_string());
    }
    Ok(id)
}

fn string_arg<'a>(args: &'a Map<String, Value>, name: &str) -> Result<&'a str, String> {
    optional_string_arg(args, name)?.ok_or_else(|| format!("missing argument: {name}"))
}

fn optional_string_arg<'a>(
    args: &'a Map<String, Value>,
    name: &str,
) -> Result<Option<&'a str>, String> {
    match args.get(name) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => Ok(Some(value)),
        Some(_) => Err(format!("argument {name} must be a string")),
    }
}

fn limit_arg(args: &Map<String, Value>, default: usize, max: usize) -> Result<usize, String> {
    match args.get("limit") {
        None | Some(Value::Null) => Ok(default),
        Some(value) => value
            .as_u64()
            .filter(|limit| *limit >= 1)
            .map(|limit| (limit as usize).min(max))
            .ok_or_else(|| "argument limit must be a positive integer".to_string()),
    }
}
