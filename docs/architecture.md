# Architecture

This document describes Slate architecture boundaries and ownership.

## High-level layout

Slate is a terminal application built from three crates:

- terminal app (`crates/tui/`, package `slate`): input, rendering, and host side effects
- editor core (`crates/editor-core/`): editing semantics
- app core (`crates/app-core/`): storage, note sources, calc evaluation, config

## Layer ownership

### Editor core (`editor-core`)

Owns semantic decisions and deterministic behavior:

- command normalization/resolution/suggestions
- command dispatch classification and host plan generation
- vim key stepping and intents
- markdown/list/table transformations
- fold semantics
- calc/variable evaluation planning

### Terminal app (`tui`)

Owns input adapters, rendering, and runtime side effects:

- key handling and UX flows
- cursor and viewport rendering
- overlays (switcher, pickers, popups) and status line
- clipboard I/O, notifications
- export and backup execution (`src/commands/`)
- IMAP sync (`src/imap.rs`, `imap` feature)

### App core (`app-core`)

Owns:

- SQLite schema and data access
- note and reminder persistence
- note source abstraction (db notes and file notes)
- calc engine and cross-note variable index
- config loading

## Command architecture

Command behavior is split into:

- core commands: pure edit operations returned from the editor core
- host commands: side-effect plans executed by the terminal app (`date`, `notify`, `module`, `export`, `write`, `quit`, etc.)

This split keeps parsing and meaning in the core while side effects stay in the host.

## Vim architecture

Vim behavior path:

1. terminal app sends key + current vim state/context
2. editor core returns `VimStep` actions
3. terminal app applies actions and updates render state

## Performance policy

Slate prioritizes:

1. correct architecture ownership
2. responsive editing at large note sizes
3. incremental updates over full-document recomputation
4. rendering from cached derived state

Terminal-local behavior is acceptable only for rendering, host side effects, or measured hot paths.

## Testing strategy

- replay fixtures in `editor-core` (vim, commands, modules)
- replay suites in the terminal app (vim, markdown, calc) comparing terminal behavior against a reference simulator
- mode-aware command suggestion and command dispatch tests

## Related files

- `crates/editor-core/src/engine.rs`
- `crates/editor-core/src/command_catalog.rs`
- `crates/editor-core/src/vim.rs`
- `crates/app-core/src/storage/`
- `crates/tui/src/terminal/app/`
- `crates/tui/src/lib.rs` (CLI entry and mode selection)
