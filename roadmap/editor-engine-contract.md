# EditorEngine Contract

This document defines the canonical shared-core contract for editor semantics.

## Ownership

- `crates/editor-core/src/engine.rs` is the canonical entrypoint for:
  - command normalization and resolution
  - command suggestions
  - note-security command parsing
  - vim key stepping (`VimState` + `VimContext` -> `VimStep`)
  - module-command planning (`CommandId` + current module state -> deterministic plan)
- `crates/editor-core/src/folding.rs` owns fold-range computation plus incremental line-edit mapping/rebuild decisions.
- The terminal app must call this contract for semantic decisions.
- The terminal app remains responsible for rendering, UI state, persistence side effects, and I/O.

## Current input/output contracts

- Buffer contract (`editor_core::buffer`):
  - `prepare_text_change`: input is the current lines, a byte-offset `TextChange`
    and current document byte length; output is replacement lines plus
    `ExactTextEdit` (pre-edit line/byte coordinates, boundary-line byte lengths
    and inserted line breaks) and `EditDelta` (line-span invalidation summary).
  - Record host metadata before calling `apply_text_change_in_place`, which
    consumes the prepared change and mutates only the affected lines. The
    buffer must stay unchanged between preparation and application.
  - Change offsets must be UTF-8 boundaries; offsets beyond the document
    length retain the existing end clamping. The host still applies compound
    changes in descending offset order, records each change before mutation
    and retains the existing undo/calc transaction boundary.
  - Offset helpers use bytes and count inter-line newlines. They do not build
    a joined copy of the document.
- Primitive buffer contract (`editor_core::buffer::primitives`):
  - Input: lines, `BufferCursor` (line and character column), and
    `PrimitiveEdit` (character/text insertion, newline, Backspace or Delete).
  - `prepare_primitive_edit` returns an allocation-free plan with exact byte
    metadata, a line-span summary and the resulting character cursor, or
    `None` for an ignored input or document-boundary no-op.
  - The host records structural reminder metadata before consuming the plan
    with `apply_primitive_edit`. The buffer must stay unchanged between these
    calls. Insertion text is borrowed; mutation retains the existing in-place
    insert, remove, split and join paths.
  - Table-specific handling runs first. Calc/history bookkeeping, autoformat
    and transaction boundaries remain in the host. Literal text insertion
    stays within one stored line; newline and paste are distinct operations.
- Paste contract (`editor_core::buffer::paste`):
  - Normalize CRLF and CR to LF before selecting table-cell or plain paste.
  - Plain preparation reports exact pre-edit coordinates, line spans and the
    resulting character cursor. It borrows normalized parts and retains the
    existing current-line clone; application keeps the per-line insert path.
  - Table-cell preparation uses the cached core table planner. The host
    records the old/new block mapping before core applies the replacement
    and resolves the character cursor.
  - Core decides whether CSV/TSV can become a table and whether it belongs
    on a blank line or below existing text. The host supplies cached fence
    state only after parsing succeeds; opening and closing fences count as
    code. Table-disabled notes and existing table rows retain their guards.
  - Hosts route exact line/block edits to reminder mapping before mutation
    and retain the existing history/calc transaction after mutation. Prepared
    edits must be applied to the unchanged buffer they were prepared against.
- Command contract:
  - input: `CommandMode`, `raw_input`
  - output: canonical command definition (or none), suggestions, normalized input
- Vim contract:
  - input: `VimState`, `VimKey`, `VimContext`
  - output: `VimStep` (`next state`, `actions`, `handled`)
- Module command contract:
  - input: `CommandId`, `ModuleState`
  - output: `ModuleCommandPlan` (`changed`, `next`, `message`)
- Markdown rule contract:
  - input: `ResolvedContext` + rule options
  - output: optional `EditOperation`
- Fold incremental contract:
  - input: existing fold ranges + line-edit deltas
  - output: mapped fold ranges plus rebuild-needed decision
- Calc scope/trigger contract:
  - input: changed-line slices + previous-change metadata + eval context flags + calc feature mask (`math_enabled`, `table_enabled`, `variables_enabled`)
  - output: deterministic eval-window decision (`eval_from`, `eval_to`, dependency flags, `can_use_partial`)
- Calc trailer-refresh eligibility contract:
  - input: previous/current line identity hash, previous result presence, selection guard
  - output: deterministic eligibility decision for commit-style trailer refresh

## Determinism requirements

- For identical inputs, output must be identical.
- Contract methods must avoid side effects.
- Runtime side effects (db writes, clipboard, notifications) happen outside this contract.

## Replay fixtures

Behavior is frozen by fixture-backed tests in `crates/editor-core/tests/golden_replay.rs`:

- `golden/vim_replay.json`
- `golden/module_command_replay.json`
- `golden/command_replay.json`

Terminal behavior is frozen by replay tests in `crates/tui/src/terminal/app/tests/replay.rs`, which drive the terminal app with key sequences and compare against expected snapshots in:

- `crates/tui/src/terminal/tests/golden/vim_replay.json`
- `crates/tui/src/terminal/tests/golden/markdown_replay.json`
- `crates/tui/src/terminal/tests/golden/calc_replay.json`
