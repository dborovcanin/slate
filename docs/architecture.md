# Architecture

This document describes Slate architecture boundaries and ownership.

## High-level layout

Slate consists of:

- GUI frontend (`src/`): CodeMirror-based Tauri UI
- TUI frontend (`src-tauri/src/terminal/`): terminal editor runtime
- shared editor core (`crates/editor-core/`): semantic engine
- backend/runtime core (`crates/app-core/` + `src-tauri/src/commands/`): storage and host services

## Layer ownership

### Shared core (`editor-core`)

Owns semantic decisions and deterministic behavior:

- command normalization/resolution/suggestions
- command dispatch classification and host plan generation
- vim key stepping and intents
- markdown/list/table transformations
- fold semantics
- calc/variable semantics and evaluation planning

### Frontends (GUI/TUI)

Own rendering/input adapters and runtime side effects:

- key handling and UX flows
- cursor and viewport rendering
- dialogs, toasts, status overlays
- clipboard I/O, notifications
- persistence execution
- export execution

### Backend storage/runtime (`app-core` and tauri commands)

Owns:

- SQLite schema and data access
- note and reminder persistence
- note source abstraction (db notes and file notes)
- IMAP integration and sidecar behavior

## Command architecture

Command behavior is split into:

- core commands: pure edit operations returned from shared engine
- host commands: side-effect plans executed by frontend/runtime (`date`, `notify`, `module`, `export`, `write`, `quit`, etc.)

This split keeps parsing and meaning shared, while allowing platform-specific execution.

## Vim architecture

Vim behavior path:

1. frontend sends key + current vim state/context
2. shared engine returns `VimStep` actions
3. frontend applies actions and updates render state

This preserves semantic parity while keeping frontend-specific rendering concerns local.

## Performance policy

Slate prioritizes:

1. correct architecture ownership
2. responsive editing at large note sizes
3. incremental updates over full-document recomputation
4. shared deterministic semantics across GUI and TUI

Hot-path local behavior is acceptable where boundary crossing would regress UX.

## Parity strategy

Parity is enforced through tests and shared contracts:

- replay fixtures in shared core
- cross-frontend parity tests in terminal suite
- mode-aware command suggestion and command dispatch tests

## Related files

- `crates/editor-core/src/engine.rs`
- `crates/editor-core/src/command_catalog.rs`
- `crates/editor-core/src/vim.rs`
- `crates/app-core/src/storage/`
- `src-tauri/src/terminal/app/`
- `src/editor/`
