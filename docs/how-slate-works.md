# How Slate Works

This document explains Slate's runtime model and the core editing flow.

## Runtime model

Slate is a terminal application (`slate`). Editing semantics live in `crates/editor-core`:

- command parsing/suggestions
- vim intent stepping
- markdown/list/table editing rules
- calc/variable semantics
- fold range behavior

The terminal app (`crates/tui`) is responsible for:

- key/input capture
- rendering
- host side effects (save, export, clipboard, notifications)

## Data model and storage

Slate stores notes in SQLite with WAL mode.

- note ids are ULID-like identifiers
- per-note module state is persisted with each note
- title is derived from the first non-empty line (denormalized for search/switcher) until it is pinned by a rename (`title_pinned`); encrypting a note pins its title
- an encrypted note's text and history are sealed with a random per-note key; `wrapped_key` holds that key sealed by the note's password (PBKDF2 with `encryption_salt`) or by the key of the encrypted collection in `key_collection_id`, whose own key is stored in `collection_keys` sealed by the collection's password (see `crates/app-core/src/storage/encryption.rs`)
- members of an encrypted collection stay encrypted: joining seals a plain note under the collection's key, and `Db::decrypt_note` and `Db::encrypt_note` refuse a member (decrypting, or moving it off the collection's key); leaving keeps the collection's key until the collection is decrypted
- schema changes run once per database, in order, recorded in `PRAGMA user_version`

Markdown file notes are also supported:

- `slate path/to/file.md`

Edits sync back to the source markdown file.

## Editing lifecycle

At a high level:

1. Load active note text and modules.
2. User edits text.
3. The terminal app applies editor-core edits/rules where required.
4. Calc/variables refresh derived results.
5. Wiki-link and image resolution refreshes derived inline display state.
6. Autosave or explicit save writes note content.

Save behavior:

- `autosave = true`: idle debounce + flush on transitions
- `autosave = false`: explicit write commands persist (`:w`, `:wq`, or `Ctrl+S`); leaving a note with unsaved changes is refused once (`can_leave_note`)
- save errors never close the session: they are shown in the status line and the buffer stays dirty

## Command lifecycle

Command input is normalized and resolved in the editor core.

1. Raw command string comes from command bar.
2. The editor core resolves command id and dispatch type.
3. Dispatch goes one of two paths:
- core command execution (pure text operations)
- host command execution (save/export/date/reminder/module/clipboard/fold/security side effects)
4. The terminal app applies resulting operations or side effects and updates status.

## Vim lifecycle

Vim behavior is split into two deterministic stages:

1. `vim::step` converts key input + current vim state into actions/intents.
2. The terminal app applies actions with core execution where available, then updates cursor/selection/mode.

## Calc and variable flow

When math modules are enabled, Slate evaluates expressions from note content and renders ghost results.

- assignment syntax: `name := expression`
- table formula syntax: `:=expression`
- variable scope: note-local
- variable resolution: case-insensitive
- module gates:
  - `math` is master gate
  - `variables` gates variable assignment/reference/autocomplete
  - `table` gates table formula handling

`Tab` behavior:

- if variable completion is active, accept completion
- otherwise, apply available calc ghost result

## Wiki-link flow

Wiki-link syntax is parsed in the editor core and resolved through note-source services.

- syntax supports `[[id]]`, `[[id#heading]]`, `[[id|title]]`, `[[id#heading|title]]`, where `id` is the target note's full ID (`markdown_tokens::is_note_link_id`)
- parser/tokenization and cursor-hit detection are editor-core owned
- note/headings lookup is performed through note-source services
- links render collapsed when the cursor is outside and show full raw source while editing inside the link
- navigation: `Ctrl+]` and vim `gd`

## Export flow

Export is command-planned in the editor core and executed by the terminal app.

- `export pdf <path>`
- `export md [path]`
- `export txt [path]`

If `md/txt` path is omitted, Slate exports to clipboard.

## Architecture boundaries

Slate prioritizes core-owned semantics and a thin terminal layer.

- Editing semantics belong in the editor core.
- Rendering and I/O belong in the terminal app.
- Performance-sensitive hot paths may remain terminal-local if measured.

For module-level boundaries, see [Architecture](./architecture.md).
