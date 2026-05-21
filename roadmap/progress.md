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
  - `dependents_of(short_id) -> Vec<String>` — which note_ids to re-eval after a change
  - `exports_for_short_id(short_id) -> &[VariableIndexEntry]` — for autocomplete queries

**`crates/app-core/src/lib.rs`**
- `cross_note_var_index: Mutex<CrossNoteVarIndex>` added to `AppCore`
- `evaluate_note_with_cross_refs(note_id, short_id, lines, options)` — drop-in replacement for `calc_engine.evaluate_note_context` that handles extern var injection and index updates in one call
- `cross_note_exports_for_autocomplete(short_id)` — returns exported variable entries for a given note short_id; ready for an IPC command to serve the UI autocomplete

### What remains (hard phase — frontend wiring)

**Propagation loop** (`src-tauri/src/`)
- After `evaluate_note_with_cross_refs` returns with `variable_values` changed, call `cross_note_var_index.dependents_of(short_id)` to get dependent note_ids
- For each dependent: re-evaluate using `evaluate_note_with_cross_refs` and push the updated results to the relevant frontend
- Must handle topological order to avoid stale intermediate values; detect cycles and break them (no output for cycle participants, consistent with existing intra-note behavior)
- Throttle: only propagate when `update_exports` returns `true` (value-level gating already implemented)

**IPC command for autocomplete**
- Add a Tauri command `get_cross_note_vars(short_id: String) -> Vec<VariableIndexEntry>` backed by `cross_note_exports_for_autocomplete`
- UI: detect `[[<8-char>]].` prefix in variable autocomplete source, call the new command, present results

**UI autocomplete (CodeMirror)**
- Extend `variable-autocomplete.ts` with a second completion source that triggers on the `[[SHORTID]].` prefix
- Fetch `get_cross_note_vars` on trigger; suggest exported variable names with `[[SHORTID]].var_name` as the `apply` value
- Needs: `variableIndexField` equivalent for cross-note entries, or an async IPC call in the completion source

**TUI autocomplete**
- Extend TUI variable popup (`src-tauri/src/terminal/`) with the same trigger pattern
- `calc_cache.rs`: on `[[SHORTID]].` prefix, look up `AppCore.cross_note_exports_for_autocomplete(short_id)` and render the popup

**TUI reactive updates**
- `calc_cache.rs` and TUI event loop need to handle incoming "note stale" signals when a dependency note changes
- Current TUI re-evaluates on keypress; cross-note staleness requires a background signal path

**Note short_id resolution**
- `evaluate_note_with_cross_refs` currently takes `short_id` as a caller-supplied parameter
- For DB notes the short_id is the first 8 chars of the ULID note_id; callers need to extract and pass it
- For markdown file notes (no ULID), cross-note refs pointing at them are not supported in this design; pass empty string to skip export indexing

**Module gating**
- Cross-note refs are currently gated implicitly by `variables_enabled`
- A dedicated `cross_note` module flag (in `EditorModulesConfig` + TOML config + UI settings) would let users opt out per note; defer until feature is stable
