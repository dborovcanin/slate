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

**`src-tauri/src/commands/calc.rs`**
- `note_short_id(note_id)` helper — extracts 8-char short ID from DB note IDs, returns `""` for `mdfile:` notes
- `evaluate_note_context_delta` now goes through `CrossNoteVarIndex`: registers note, snapshots extern vars before `spawn_blocking`, updates exports and deps after
- `get_cross_note_vars(short_id)` Tauri command — returns exported variable entries for a note; backed by `cross_note_exports_for_autocomplete`

**`src/api.ts`**
- `getCrossNoteVars(shortId)` IPC wrapper

**`src/editor/variable-autocomplete.ts`**
- `crossNoteCompletionSource` — async CodeMirror completion source; detects `[[SHORTID]].partial` before the cursor, calls `getCrossNoteVars`, suggests variable names with `[[SHORTID]]` as the detail label; stale-doc guard after await
- `variableAutocompleteExtensions` updated to include both sources in the `override` array; cross-note source always active (even when local variable autocomplete is disabled)

### What remains (TUI)

**TUI reactive updates**
- `calc_cache.rs`: call `AppCore.evaluate_note_with_cross_refs` instead of `CalcEngine::evaluate_note_context` so the shared index is updated on TUI edits too
- Cross-note staleness on TUI is lazy: next keypress on a dependent note picks up fresh values automatically (same model as UI)

**TUI autocomplete**
- TUI variable popup (`src-tauri/src/terminal/`): detect `[[SHORTID]].` prefix on the current line
- On match: call `AppCore.cross_note_exports_for_autocomplete(short_id)` and render the existing popup with those entries

**Module gating**
- Cross-note refs are currently gated implicitly by `variables_enabled`
- A dedicated `cross_note` module flag is deferred until the feature is stable
