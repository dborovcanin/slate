# EditorEngine Contract

This document defines the canonical shared-core contract for editor semantics.

## Ownership

- `crates/editor-core/src/engine.rs` is the canonical entrypoint for:
  - command normalization and resolution
  - command suggestions
  - note-security command parsing
  - vim key stepping (`VimState` + `VimContext` -> `VimStep`)
  - module-command planning (`CommandId` + current module state -> deterministic plan)
- `crates/editor-core/src/wasm.rs` exposes batched markdown transaction execution for UI hot-path text rules.
- `crates/editor-core/src/folding.rs` owns fold-range computation plus incremental line-edit mapping/rebuild decisions.
- Frontends (Tauri UI and TUI) must call this contract for semantic decisions.
- Frontends remain responsible for rendering, UI state, persistence side effects, and I/O.

## Current input/output contracts

- Command contract:
  - input: `CommandMode`, `raw_input`
  - output: canonical command definition (or none), suggestions, normalized input
- Vim contract:
  - input: `VimState`, `VimKey`, `VimContext`
  - output: `VimStep` (`next state`, `actions`, `handled`)
- Module command contract:
  - input: `CommandId`, `ModuleState`
  - output: `ModuleCommandPlan` (`changed`, `next`, `message`)
- Markdown transaction contract:
  - input: `EditorContextSnapshot` + ordered markdown transaction requests
  - output: first matching `{ kind, operation }` edit operation
- Fold incremental contract:
  - input: existing fold ranges + line-edit deltas
  - output: mapped fold ranges plus rebuild-needed decision

## Determinism requirements

- For identical inputs, output must be identical across GUI and TUI.
- Contract methods must avoid side effects.
- Runtime side effects (db writes, clipboard, notifications) happen outside this contract.

## Replay fixtures

Behavior is frozen by fixture-backed tests in `crates/editor-core/tests/golden_replay.rs`:

- `golden/vim_replay.json`
- `golden/module_command_replay.json`
- `golden/command_replay.json`
