# In-Progress Features

## Reactive Cross-Note Calculations

Extends the inline calc and variables system across note boundaries. Values defined in one note can be referenced and consumed in another; when the source changes, dependent expressions update reactively.

### Syntax

```
[[SHORTID]].var_name
```

Uses the existing 8-char wiki-link short ID. Variable name follows the same rules as note-local vars (case-insensitive, spaces normalized, must contain at least one letter/underscore).

Example:
```
# Budget note
monthly_income := 5000

# Goals note
[[A1B2C3D4]].monthly_income * 12
```

### What is done

**`crates/app-core/src/calc/engine.rs`**
- `ExternVar` struct — carries resolved values from other notes into an evaluation
- `CrossNoteRef` struct — records cross-note refs found in a note's lines
- `extern_vars: Vec<ExternVar>` field on `NoteEvaluationOptions`
- `cross_note_refs: Vec<CrossNoteRef>` and `variable_values: FxHashMap<String, f64>` fields on `NoteEvaluationResult`
- `scan_cross_note_refs(lines) -> Vec<CrossNoteRef>` — regex scan for `[[SHORTID]].var_name` patterns (public)
- `preprocess_line_cross_note` — substitutes resolved extern vars into a line before eval; unresolved refs leave the line unevaluated
- `VariableResolver::numeric_values()` — extracts resolved variable values as f64 for export
- 5 unit tests covering: ref scanning, successful substitution, unresolved ref silence, mixed local+cross-note, variable_values export

**`crates/app-core/src/cross_note.rs`** (new)
- `CrossNoteVarIndex` — the central reactive data structure:
  - `exports`: short_id → (var_normalized → f64) — what each note exports
  - `export_entries`: short_id → Vec<VariableIndexEntry> — for autocomplete
  - `deps`: note_id → Set<short_id> — which notes this note depends on
  - `short_id_to_note_id`: reverse lookup
  - `update_exports(short_id, entries, values) -> bool` — updates exports, returns true if values changed
  - `update_deps(note_id, refs)` — updates dependency graph from scan results
  - `extern_vars_for(note_id) -> Vec<ExternVar>` — builds the extern_vars input for a note's next eval
  - `exports_for_short_id(short_id) -> &[VariableIndexEntry]` — for autocomplete queries

**`crates/app-core/src/lib.rs`**
- `cross_note_var_index: Mutex<CrossNoteVarIndex>` added to `AppCore`
- `cross_note_exports_for_autocomplete(short_id)` — returns exported variable entries for a given note short_id

### Terminal integration

**`crates/app-core/src/lib.rs`**
- `cross_note_var_index` changed from `Mutex<CrossNoteVarIndex>` to `Arc<Mutex<CrossNoteVarIndex>>`
- Added `cross_note_var_index_arc()` accessor to clone the Arc

**`crates/tui/src/terminal/app/calc_helpers.rs`**
- `tui_note_short_id(note_id)` helper — extracts 8-char short ID, returns `""` for `mdfile:` notes
- `compute_calc_data_for_note(...)` — full-note eval that reads extern vars from the shared index before eval and updates exports/deps after; used for whole-document recomputes
- `extract_cross_note_completion_prefix(line, cursor_col)` — detects `[[SHORTID]].partial` before the cursor, returns `(short_id, from_col, partial_query)` using a lazy-compiled Regex

**`crates/tui/src/terminal/app/mod.rs`**
- Added `cross_note_var_index: Arc<Mutex<CrossNoteVarIndex>>` field to `TerminalApp`
- `run_terminal_session` and `new_with_startup_metrics` accept and store the Arc
- `std::sync::{Arc, Mutex}` and `app_core::cross_note::CrossNoteVarIndex` imported

**`crates/tui/src/terminal/app/editing.rs`**
- Stale full-recompute path (`if self.calc.stale { ... }`) uses `compute_calc_data_for_note` instead of `compute_calc_data` so the shared index is updated on every full eval
- `variable_autocomplete_state` checks for `[[SHORTID]].partial` prefix first; on match, queries the index directly and returns cross-note variable suggestions using the existing popup infrastructure

**`crates/tui/src/lib.rs`**
- `run_terminal_session` receives `core.cross_note_var_index_arc()`

**Loading values on open (2026-09-29):** the first calc pass after opening a note, and viewport evaluation of large notes, now load linked notes' values first, so `[[SHORTID]].var` lines show values without being edited. Large notes do this on the background calc preparation.

### Deferred

**Module gating**: cross-note refs are gated implicitly by `variables_enabled`. A dedicated `cross_note` module flag is deferred until the feature is stable.
