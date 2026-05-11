# How Slate Works

This document explains Slate's runtime model and the core editing flow.

## Runtime model

Slate has two user-facing frontends that share semantic logic:

- GUI app (Tauri + CodeMirror)
- TUI app (terminal runtime)

Both use shared logic in `crates/editor-core` for:

- command parsing/suggestions
- vim intent stepping
- markdown/list/table editing rules
- calc/variable semantics
- fold range behavior

Frontends are responsible for:

- key/input capture
- rendering
- host side effects (save, export, clipboard, notifications, dialogs)

## Data model and storage

Slate stores notes in SQLite with WAL mode.

- note ids are ULID-like identifiers
- per-note module state is persisted with each note
- title is derived from the first non-empty line (denormalized for search/switcher)

Markdown file notes are also supported:

- GUI: `slate path/to/file.md`
- TUI: `slate --terminal path/to/file.md`

Edits sync back to the source markdown file.

## Editing lifecycle

At a high level:

1. Load active note text and modules.
2. User edits text.
3. Frontend applies shared-core edits/rules where required.
4. Calc/variables refresh derived results.
5. Wiki-link and image resolution refreshes derived inline display state.
6. Autosave or explicit save writes note content.

Save behavior:

- `autosave = true`: idle debounce + flush on transitions
- `autosave = false`: explicit write commands persist (`:w`, `:wq`, or non-vim save shortcuts)

## Command lifecycle

Command input is normalized and resolved in shared core.

1. Raw command string comes from command bar.
2. Shared core resolves command id and dispatch type.
3. Dispatch goes one of two paths:
- core command execution (pure text operations)
- host command execution (save/export/date/reminder/module/clipboard/fold/security side effects)
4. Frontend applies resulting operations or side effects and updates status.

This keeps command semantics aligned between GUI and TUI while allowing frontend-specific I/O.

## Vim lifecycle

Vim behavior is split into two deterministic stages:

1. `vim::step` converts key input + current vim state into actions/intents.
2. Frontend applies actions with shared execution where available, then updates cursor/selection/mode.

Both GUI and TUI consume this shared stepping contract.

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

Wiki-link syntax is parsed in shared core and resolved in frontend/runtime services.

- syntax supports `[[shortid]]`, `[[shortid#heading]]`, `[[shortid|title]]`, `[[shortid#heading|title]]`
- parser/tokenization and cursor-hit detection are shared-core owned
- note/headings lookup is performed through note-source services
- GUI and TUI render collapsed display for out-of-cursor links and keep full raw source while editing inside the link
- navigation:
  - GUI: Ctrl/Cmd+click and vim `gd`
  - TUI: `Ctrl+]` and vim `gd`

## Export flow

Export is command-planned in shared core and executed in host runtime.

- `export pdf <path>`
- `export md [path]`
- `export txt [path]`

If `md/txt` path is omitted, Slate exports to clipboard (GUI and TUI).

## Architecture boundaries

Slate prioritizes shared semantics and thin frontends.

- Editing semantics belong in shared core.
- Rendering and I/O belong in frontend/runtime.
- Performance-sensitive hot paths may remain adapter-local if needed.

For module-level boundaries, see [Architecture](./architecture.md).
