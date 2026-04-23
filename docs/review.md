# Slate Architecture Review

Scope: full-codebase architectural and performance review against `docs/plan.md`, the last 20 architecturally relevant commits, and the current state of `crates/editor-core`, `crates/app-core`, `src-tauri/`, and `src/`.

Methodology: read the plan, inspected the top of the commit log with stat, read the largest and most central files in every layer, and spot-checked integration seams (wasm boundary, IPC surface, TUI dispatch, calc scheduling, folding, tables, decorations).

---

## A. Executive Summary

### Top findings

1. **Plan direction is right; implementation is ~70% there.** Shared-core ownership of vim intents, text rules, folding, and calc planning is solid. What remains is not "decide what is shared" — it is "remove adapter-local duplicates of already-shared logic" and "fix the wasm boundary cost so the hot path stops paying for it."
2. **`editor-core/src/wasm.rs` is a 1812-line hand-marshaled JS bridge.** It is the single largest source of architectural friction: every VimIntent, every calc struct, every fold range requires a matching encoder on the Rust side and a matching decoder in `src/editor/wasm.ts` (1938 lines). This is not shared-core; it is a shared-core-plus-two-mirrors setup that grows with every feature.
3. **`src-tauri/src/terminal/app/mod.rs` defines one god struct (`TerminalApp`, ~80 fields, ~30 concerns).** Recent splits (commits `405144d`, `c81fa8b`) broke the 11,273-line `app.rs` into 10+ files but did not break the ownership boundary — every new file just `impl TerminalApp`s more methods. The struct itself has not been decomposed.
4. **TUI pays O(N) per keystroke on anything bigger than ~2000 lines.** `recompute_calc_full` (and the `defer_calc_state_after_edit` shortcut added to dodge it) rebuilds four parallel `Vec`s (`prev_line_hashes`, `prev_line_assignment_name`, `prev_line_has_assignment`, `prev_line_has_builtin_formula`) across the whole doc after every edit, even when the incremental planner already knows exactly which lines changed.
5. **Calc engine blocks the Tauri IPC thread.** `commands::calc::evaluate_note_context` is a sync Tauri command; `fend_core::evaluate_with_interrupt` is called with a hardcoded `NoInterrupt`. A large-note recalc blocks every other DB/IPC call (including `save_note`) for the duration.
6. **Storage is behind a single `Mutex<Connection>`.** This is an easy win (rusqlite supports a pool or `Arc<Mutex<>>` with more granular locking, or switch to `tokio-rusqlite` / `deadpool-sqlite`). Calc+save contention is visible on paper.
7. **Context snapshot pays for a full `String` clone plus full `Vec<String>` line materialization on every rule execution.** `EditorContextSnapshot { text: String, … }` + `ResolvedContext::new` → `parse_lines` → `Vec<String>` with `.to_string()` per line. At 100k lines that is ~100k allocations per wasm call on a hot path the plan explicitly flags.
8. **Tauri coupling is contained but not abstracted.** All `@tauri-apps` imports live in `src/main.ts`, `src/app.ts`, `src/api.ts`. No editor file depends on Tauri directly. That's excellent isolation — but there is no `BackendClient` interface, so a port to Qt means rewriting `api.ts` in place rather than swapping an implementation.
9. **The calc/table formula path has quiet duplication.** `table_expression_segment` (returns 0/1) and `table_expression_segments` (returns N) live side by side in `app-core/src/calc/engine.rs`, with essentially the same parsing. Same pattern appears between `editor-core/src/table.rs` (structural helpers) and `calc_plan::find_table_formula_segment(s)`.
10. **Calc widget + decoration rebuild is well-bounded, but viewport/scope detection in `calc-decoration.ts` scans the same line regions multiple times.** `lineHasCalcGlobalSyntax`, `rangeTouchesCalcExpression`, `visibleSpansTouchCalcSyntax`, `updateTouchesCalcExpression` all iterate changed/visible ranges with overlapping predicates; a single pass producing all flags would cut duplicate WASM calls.

### Architectural health

Overall: **solid bones, accumulating ergonomic debt.**

- Layering is correct: `editor-core` (pure, wasm+native) → `app-core` (native services) → `src-tauri` (platform glue + TUI + IPC) → `src/` (UI).
- Ownership rules in `docs/plan.md` are actually followed in the recent commits. The direction of travel is clear and good.
- The places that hurt are the seams (wasm marshaling, snapshot clones, TUI god struct), not the layering.

### Biggest risks

1. **Responsiveness on large notes.** 20k-line notes are described as a target, 100k+ as a stretch. The current full-doc hash + parallel-vector rebuild on every edit will not scale past ~50k without visible lag.
2. **TUI/UI semantic drift.** The plan calls for a parity suite; golden replay tests exist for vim and markdown. Calc, folding, and command semantics do not have cross-adapter parity coverage. Every new edit command is currently implemented ~twice (TS in `vim.ts` + Rust in `terminal/app/vim_actions.rs`) with parity enforced by eyeballing.
3. **Framework replacement cost.** Moderate, not high. The IPC surface (`api.ts` ↔ `commands/*`) is 29 invoke calls; a `BackendClient` abstraction is a weekend. The harder part is that `app.ts` mixes IPC, session, UI-shell and keyboard work in 1006 lines; that would need a port regardless of framework.

### Biggest opportunities

1. **Replace hand-rolled wasm marshaling with `serde-wasm-bindgen` (or `tsify`).** Estimated removal: 1200–1400 lines across `wasm.rs` + `wasm.ts`. Payoff: new intents/structs become zero-cost from a plumbing perspective.
2. **Store `lines: Vec<String>` (or `Rope`) + derived indices once in `editor-core`, not "text: String + reparse on every call".** Makes every rule cheap to call and the 100k-line edit path tractable.
3. **Decompose `TerminalApp`.** Extract `EditorModel` (lines, cursor, selection, undo), `RenderState` (fence_checkpoints, draw_buf, last_drawn_rows), `OverlayState` (switcher, date picker, command bar), `CalcRuntime` (debounce, viewport prefetch), `FoldRuntime`. Existing files become the impls for the focused struct instead of methods on the god struct.
4. **Make calc async off the Tauri thread.** `fend_core::Interrupt` already supports it; the blocker is the sync Tauri command.

---

## B. Alignment with `docs/plan.md`

### Intended architecture (per plan)

- Layers: `editor-core` (Rust, pure) → UI adapter (Tauri + CodeMirror) → TUI adapter → backend/runtime services.
- Shared-core owns anything that mutates document text, cursor/selection, vim, calc, or fold semantics.
- Adapters keep only: rendering, viewport, host side effects, and measured-hot-path local copies.
- Hot-path policy: no per-keystroke wasm round-trips for insert mode, batched shared semantics where possible.
- Incremental over full recomputation.

### Actual architecture

| Plan item | Current state | Gap |
|---|---|---|
| Shared-core vim intents | Yes — `editor-core::vim`, `vim_actions`, `VimIntent` enum with native+wasm consumers | Dispatch table in UI (`src/editor/vim.ts`) still has ~1200 lines of adapter-side logic that could be thinner |
| Shared-core text rules | Yes — `text_rules.rs` run_doc_change/enter/tab/table_* called by both adapters via wasm+native | `markdown-editing.ts` (738 lines) still has trigger heuristics and `lineMightTriggerDocChangeRules`, `shouldDeferTableAutoformatForSpace` duplicated vs TUI logic in `editing.rs` |
| Shared-core calc planning | Yes — `editor-core::calc_plan` owns window/scope decisions, formula detection, trailer refresh plan | TUI and UI both reimplement per-line-metadata tracking around the shared planner (prev_line_has_assignment, etc.); this should move into a shared `CalcLineIndex` struct |
| Shared-core folding | Partially — structure + range mapping are shared; TUI and UI still independently call `recompute_folding`/`rebuildFoldView` | No parity tests for fold state across adapters (plan explicitly flags this) |
| Host-owned commands | Classified via `EditorEngine::classify_command_dispatch` → `CommandDispatchKind::Host*` | Good boundary; cleanly implemented |
| Hot-path policy (no per-keystroke wasm round-trips) | Partial — `wasm_run_markdown_transaction_batch` exists and is used for batched rules | Individual wasm calls for `classify_line`, `find_inline_tokens`, `tokenize_code_line`, `find_calc_segment`, etc. still happen per visible line in `markdown-decoration.ts` viewport builds |
| Incremental vs full recomputation | UI path is incremental (`plan_incremental_calc`, `map_ranges_through_line_edits`) | TUI `recompute_calc_full` rebuilds all 4 parallel state vectors every time; `recompute_folding` rebuilds fold structure fully on most edits |

### Mismatches and drift

- **Plan: "Keep one canonical intent mapping source and avoid frontend numeric coupling."** Currently the mapping lives in three synchronized places: `VimIntent` enum in `editor-core/src/vim.rs`, `intent_to_id`/`intent_from_id` in `editor-core/src/wasm.rs`, and `VIM_INTENT` in `src/editor/wasm.ts`. Adding an intent requires updating all three plus the TS `VimIntentMap`.
- **Plan: "Remove unsupported-intent panic paths from parity simulation."** Still open per plan; I did not verify the parity simulator directly but the plan itself carries this as TODO.
- **Plan: "Tighten invalidation/remap behavior for large-note edits to avoid stale calc state."** Partially done: UI has `remapCalcResultsForDocChange` with eval-key rescue, TUI has `defer_calc_state_after_edit` which clears to `None` for `LARGE_DOC_CALC_DEFER_LINES=20000`. The TUI approach is a shortcut, not a solution — above 20k the note gets no calc at all until the idle tick reschedules.
- **Plan: "Expand parity from vim key replay to command/rule/folding/calc semantic parity suites."** Golden fixtures exist only for vim and markdown. Calc and folding have no parity corpus.
- **Plan: "Architecture guardrails in CI."** Not visible in the tree (no CI config in the checked-out files I scanned). Golden replay tests exist as local test files only.

### Where the plan is unclear or contradictory

- The plan's "hot path policy" and "Shared-core first" principles point in opposite directions for rendering. The current code resolves this by keeping `markdown-decoration.ts` (1207 lines) UI-only — which is correct — but the plan does not explicitly exempt rendering tokens from the shared-core default, so the boundary is implicit rather than documented.
- "Performance and responsiveness before convenience abstractions" is fine, but the code has accepted a lot of convenience (hand-marshaling, parallel vectors, snapshot clones). The plan does not give a quantitative bar for "responsiveness."

---

## C. Recent Architecture Changes (last 20 commits)

### Summary

The commit log is almost entirely consolidation work: moving editing logic into `editor-core`, deleting redundant TS modules, and splitting the TUI's monolithic `app.rs`. The direction is consistent with the plan.

| Commit | Impact |
|---|---|
| `6b11296` Shared calc window + TUI dirty-buffer update | + Moved calc eval window decisioning to shared (`calc_plan.rs`). + TUI now re-renders on dirty buffer. Net-positive alignment. |
| `934c39e` Remove full doc string for calc / async evals | Removed full-doc string clone in calc (`crates/app-core/src/calc/engine.rs` +40 lines), partial async support. |
| `8a3ae2d` Remove unused code | **-867 LOC**: deleted `src/editor/core/sum.ts`, `src/editor/ex-commands.*`, dead `editor-core/src/wasm.rs` helpers (178 lines). Pure hygiene win. |
| `c81fa8b` Split app.rs further | Split monolithic test file into per-domain files (`calc_table.rs`, `vim.rs`, `layout_folding.rs`, `command_security_switcher.rs`). Source file structure, not semantics. |
| `405144d` Split app to modules | Broke `src-tauri/src/terminal/app.rs` (11,273 LOC) into 10 files. **Structural move only — `TerminalApp` struct is still defined in `mod.rs` with ~80 fields; every other file `impl`s onto it.** |
| `25ee927` Update remaining Vim gaps | +3,315 LOC in `editor-core` (new `math_commands.rs`, `vim_actions.rs`). Shared-core grew. |
| `9f30dd3` Improve UI and TUI architecture alignment | +818 LOC consolidation. Shared wasm surface expanded for vim parity. |
| `ae34451` Use char-based pasting and share it with UI | +917 LOC. Shared pasting semantics; TUI and UI converge on one code path. |
| `21762f2` Implement parity tests | +491 LOC. Golden replay introduced. |
| `601f1c1` / `a6eb119` / `1d79a72` / `9a74d00` / `7c905e0` | Sequential plan-item implementations (items 1–9). These are the bulk of the shared-core consolidation. |
| `93a101f` Fix table regression | Tactical fix in `text_rules.rs` + `markdown-editing.ts`. |

### What improved

- Shared-core surface now covers vim intents, math commands, text rules, calc planning, folding structure, markdown token analysis, and command catalog. This is the core thesis of the plan and it lands.
- Parity infrastructure exists (`golden_replay.rs`, `vim_parity_replay.json`, `markdown_parity_replay.json`). Extending it is a matter of adding fixtures, not building plumbing.
- Dead code was actively pruned, not just shadowed (commit `8a3ae2d`). Healthy.
- `editor-core/src/wasm.rs` got a batching entry point (`wasm_run_markdown_transaction_batch`) to amortize boundary crossings. Right idea.
- Charwise paste became a shared semantics rather than adapter-local (commit `ae34451`) — plan item.

### What regressed or remains half-done

- `editor-core/src/wasm.rs` grew from ~450 lines to 1812 lines over this window. The growth is because every new shared function needs a hand-written marshaling shim. This is ergonomic debt compounding at the rate of feature delivery.
- `TerminalApp` structure is **untouched** semantically by the recent split. The file split was a readability improvement, not a decomposition. The god struct is the real architectural issue and it is now harder to see because the fields are in `mod.rs` and the methods are in 10 other files.
- `calc_plan.rs` went from ~500 lines to 1620 with every plan item. The planner and the marshaling layer are growing faster than anything else in the core.
- No commit in the last 20 touched the `EditorContextSnapshot { text: String }` allocation pattern, even though the plan explicitly flags "hot path" concerns.
- The plan items are done sequentially (1, 2, 4, 5, 6, 7, 8, 9) but item 3 and the "cross-subsystem architecture action plan" items (parity in calc+folding, CI guardrails, feature-flag policy) are not visible.

### Dead-end / partial refactors

- `editor-core::sum` still exists (102 lines) but `src/editor/core/sum.ts` was deleted. The Rust sum.rs is still used by `math_commands.rs`, so not actually dead; the naming is confusing (sum vs math_commands vs commands in src-tauri).
- `src-tauri/src/editor_core/mod.rs` is a pure re-export shim over the `editor_core` crate ("so existing callers don't need path changes"). It's a transitional artifact — paths could just change.

---

## D. Subsystem Architecture Review

### D.1 Shared editor core (`crates/editor-core`)

**Shape.** Pure library, no native-only deps. Modules: `types`, `context`, `operations`, `command_catalog`, `command_history`, `engine`, `vim`, `vim_actions`, `text_rules`, `math_commands`, `sum`, `substitute`, `format`, `folding`, `table`, `markdown_tokens`, `calc_plan`, `wasm` (cfg=wasm32).

**Strengths.**
- Clear separation: parsing/rules/planning are data-in → data-out, no side effects. `context::ResolvedContext` is the standard input shape.
- Intent-based vim: `vim::step` returns `VimStep { state, actions: Vec<VimAction>, handled }` — declarative; actions executed separately by caller. Good.
- `command_catalog::COMMAND_DEFINITIONS` is a single static table (41 entries) for both UI and TUI. Correct.
- Deterministic hashing (`hash_line` via `DefaultHasher`) gives the calc planner a stable identity for lines across edits.

**Weaknesses.**

1. **`EditorContextSnapshot` is a heavy value.**
   ```rust
   pub struct EditorContextSnapshot {
       pub text: String,              // full doc copy
       pub selection: SelectionSnapshot,
       pub changed_range: Option<TextRange>,
   }
   ```
   `ResolvedContext::new(snapshot)` → `parse_lines(&snapshot.text)` → `Vec<String>` per line. `ResolvedContext::from_parts(&str, …)` avoids the outer clone but still materializes `lines: Vec<String>` with `text[start..idx].to_string()` in `parse_lines`. For a 100k-line doc this is 100k allocations per call, on a hot path (`run_doc_change_rules`, `run_enter_rules`, `run_tab_rules` all consume this).

2. **`markdown_tokens::analyze_lines(&[String], …)` takes owned lines.** It's called from `folding::build_fold_ranges_with_options` with the full owned `&[String]` slice — fine. But the TUI and UI both have to own their lines as `Vec<String>`. If the source is a `Rope` or a `doc.line(n).text` iterator, you pay to materialize.

3. **Wasm boundary cost dominates `wasm.rs` size.** 
   - `edit_operation_to_js`, `calc_segment_to_js`, `calc_refresh_plan_to_js`, `markdown_line_info_to_js`, and 30+ other hand-written encoders.
   - `wasm_vim_intent_id_map` is 253 lines of `set_prop(&out, "MOVE_LEFT", JsValue::from_f64(intent_to_id(VimIntent::MoveLeft) as f64))` entries.
   - `intent_to_id`/`intent_from_id` are inverse match statements with 50 arms each — adding an intent requires 4+ edits, 2 of which are in the same file.
   - **Recommendation:** `serde-wasm-bindgen` or `tsify` with `#[wasm_bindgen]` + `#[derive(Tsify)]`. Removes most of the file. Keeps exactly the same wire format.

4. **`text_rules.rs` (2152 LOC) is the biggest module.** It owns 7 public entry points (`run_doc_change_rules`, `run_enter_rules`, `run_tab_rules`, `run_table_cell_navigation_rules`, `run_table_pipe_insert_column_rule`, `run_table_header_delete_column_rule`, `run_table_boundary_edit_rules`) plus a zoo of helpers. Responsibilities conflated:
   - list marker parsing/incrementing (`parse_ordered_marker_segments`, `increment_ordered_marker`, `indent_ordered_marker`)
   - list line inspection (`parse_list_line_parts`, `parse_checklist_after_prefix`)
   - actual rule dispatch
   
   **Recommendation:** split into `text_rules/list.rs`, `text_rules/checklist.rs`, `text_rules/table.rs`, `text_rules/dispatch.rs`. Mechanical refactor.

5. **`calc_plan.rs` (1620 LOC) is the second biggest** and mixes formula detection (`find_table_formula_segment`, `find_single_calc_table_cell`, `find_list_calc_segment`), planner (`plan_incremental_calc`, `decide_eval_scope`, `decide_eval_window`), refresh plan (`compute_calc_refresh`), and hashing utilities (`hash_line`, `hash_lines`). Good candidate for a submodule split.

**Extensibility.**
- Adding a new command: add to `CommandId`, to `COMMAND_DEFINITIONS`, plumb through `classify_command_dispatch` + `plan_host_command` if host-owned, implement in `math_commands.rs`/`commands.rs` if core. Pattern is clean.
- Adding a new VimIntent: enum + `vim.rs::step` + `vim_actions.rs::execute_vim_action` + wasm `intent_to_id`/`intent_from_id` + TS `VIM_INTENT`. Too many places; the wasm surface is the noisy part.
- Adding a new decoration/widget: UI-only. Fine as-is.
- Adding fold kinds: `FoldKind` enum + `FoldBuildOptions` + `fold_structural_signature` + `as_str`. Localized.

**Testability.** Good. Modules expose pure functions; tests are in-file `#[cfg(test)] mod tests`. Golden replay via `tests/golden_replay.rs` is excellent for regression proofing.

### D.2 Tables

**Model.** Two sources of truth:
- Structural: `crates/editor-core/src/table.rs` — pipe positions, cell spans, delimiter parsing, row normalization, row formatting.
- Formula: `crates/editor-core/src/calc_plan.rs::find_table_formula_segment(s)` + `crates/app-core/src/calc/engine.rs::table_expression_segment(s)` + `evaluate_table_formula*`.

**Problems.**

1. **Duplicated segment finders.** `calc_plan::find_table_formula_segment` (editor-core) and `engine::table_expression_segment` (app-core) parse the same pipe-separated cells with overlapping rules (`has_calc_signal`, `looks_like_assignment`, `parse_builtin_formula`). The split exists because `editor-core` is wasm-pure and `app-core` uses `fend-core`, but the *parsing* logic is duplicated rather than shared.

2. **`table_expression_segment` (0 or 1) vs `table_expression_segments` (all formula cells).** Both exist in `engine.rs`. The former is legacy single-cell behavior; the latter is the multi-cell path added in `6b11296`. Same in `calc_plan::find_table_formula_segment(s)`. One of each pair is legacy.

3. **No structured Table model.** The table "exists" only as a line range where each line starts and ends with `|`. Operations are per-line string rewrites + re-normalization. This works for simple cases; it blocks the plan's future items: "Allow multiple rows in table cell", "Support multiple formulas in row/column contexts", "Enrich table with `(1,2)` access". A `TableBlock { header, delimiter, rows: Vec<TableRow { cells: Vec<TableCell> }>, align: Vec<TableAlign> }` would remove the re-parsing-per-keystroke cost and make multi-line cells representable.

4. **Performance.** `table::table_pipe_positions` does `line.match_indices('|').collect::<Vec<usize>>()` per call. `find_table_formula_segment` scans pipes from scratch. UI has a `FORMULA_SEGMENT_CACHE` (512 entries, line-text keyed) that amortizes; TUI has no equivalent — hot-render paths reparse the active table line every frame.

5. **Table normalization churn.** Every space typed inside a table cell potentially triggers `run_table_cell_navigation_rules` / `run_table_boundary_edit_rules` / `run_doc_change_rules` which re-emit the full normalized block. For a 50-row table each keystroke emits a ~50-row rewrite. Correct, but expensive on edit latency.

6. **Formula evaluation (`evaluate_table_formula*`)** uses `working_lines: Vec<String>` lazy-cloned from `lines`. Good. But the substitution writes back into the cloned vector and re-evaluates — for a table with N formula cells this is O(N²) string rewrites + N × fend calls.

**Integration.** Cursor/selection-aware table navigation (`tableCellAtColumn` in UI, `table_cell_info_at_char` in TUI) is implemented *twice*, once in `src/editor/markdown-editing.ts` and once in `src-tauri/src/terminal/app/table_helpers.rs`. Same logic, different languages. Should move into `editor-core::table`.

**Recommendations.**

- Extract a shared `TableModel` in `editor-core::table` that: (a) parses once, (b) exposes cell spans, (c) serializes on demand. Both calc and rule paths consume it.
- Cache the parsed table per `(line_hash → TableModel)` in core; share between UI and TUI.
- Move the multi-cell formula path into `calc_plan` as a pure planner and keep `engine.rs` responsible only for fend invocation. The planner/parser split is the cleaner cut.

### D.3 Calc engine

**Structure.**
- Semantics/planning: `editor-core::calc_plan` (pure, portable, ~1620 LOC).
- Evaluation: `app-core::calc::engine::CalcEngine` (native, `fend-core`, ~2087 LOC).
- UI scheduler: `src/editor/calc-decoration.ts` + `src/editor/calc-incremental.ts` + `src/editor/calc-line-utils.ts`.
- TUI scheduler: `src-tauri/src/terminal/app/calc_helpers.rs` + `src-tauri/src/terminal/calc_cache.rs` + inline orchestration in `editing.rs`.

**Strengths.**
- **Clean plan/exec split.** `editor-core::calc_plan` decides *what and where to evaluate*; `app-core::calc::CalcEngine` evaluates. Portable planner is a great pattern.
- **Incremental window** (`decide_eval_window`, `plan_incremental_calc_from_hashes`) is well thought out; handles variable dependencies, builtin formula chains, and per-line assignment name tracking.
- **Commit-mark trailer refresh** (`compute_calc_refresh`, `should_attempt_calc_trailer_refresh`) is a clever pattern: the user types `= 4`, the engine re-rewrites it transparently when the upstream expression drifts, without clobbering cursor or uncommitted edits.
- **Variable regex cache** with key-based deduplication (`VARIABLE_REGEX_CACHE`) avoids rebuilding the alternation regex on every eval when names stabilize.

**Weaknesses.**

1. **`fend_core::Interrupt` is hardcoded to `NoInterrupt`.** `static NO_INTERRUPT: NoInterrupt = NoInterrupt` is passed to every `evaluate_with_interrupt` call. This means:
   - Long-running calcs cannot be cancelled.
   - A slow expression (fend recursion / big unit conversion) blocks until it finishes.
   - The Tauri IPC thread is held for the whole eval.
   
   **Fix:** thread a `Arc<AtomicBool>` through, flip it from a debounced outer scheduler; measured cost is one atomic load per interrupt check.

2. **Sync Tauri commands for calc.** `commands::calc::evaluate_note_context(_delta)` is `#[tauri::command]` without `async`. Tauri handles it on a blocking thread pool, but the `AppCore` state is a `State<'_, AppCore>` shared reference and the underlying engine/DB are `Mutex`-locked. A large-note calc blocks `save_note`, `list_notes_meta`, and every other command for its duration. **Fix:** make calc async and move fend off the IPC thread; use channels or `tokio::task::spawn_blocking`.

3. **Global `Mutex<Connection>` (`app-core::storage::sqlite::Db::conn: Mutex<Connection>`).** Every command path locks it, including `save_note` and calc cache operations. `note_line_cache: Mutex<HashMap<String, Vec<String>>>` is a second serialization point. **Fix:** Use `r2d2` or `deadpool-sqlite` and open read-mostly connections separately; SQLite in WAL mode supports concurrent readers.

4. **Two parallel table formula code paths.** `evaluate_table_formula` orchestrates multi-cell rewriting; `table_expression_segment(s)` parses. Hard to follow; changes to one can desync from the other.

5. **`collect_variable_definitions` does one full-document scan per eval.** For docs with no assignments this costs O(N) to produce an empty `HashMap`. The `prev_line_has_assignment` vector in TUI already has the answer — pass it in or short-circuit on `contains_variable_assignment(&lines)` first. (The front-ends do gate on `cached_has_variable_assignment`, but the engine still does the scan when they pass `variables_enabled=true` on a lineset with no assignments.)

6. **Dead path `evaluate_lines`.** `CalcEngine::evaluate_lines` is still exposed and used by `commands::calc::evaluate_lines`. Nothing in the main flow calls it (`evaluate_note_context(_delta)` is the real path). Candidate for removal.

7. **`format_number` rounds at `1e-9` tolerance then prints `{:.10}` with trailing-zero trim.** Works, but allocates twice for every number. Not hot enough to optimize unless calc becomes per-keystroke.

**Extensibility.** Adding a builtin formula: `parse_builtin_formula` + `FormulaOp`/`FormulaScope` + `collect_table_formula_terms`. Local. Adding a new calc trigger (e.g., list-aggregate): `extract_line_expression` plus planner signal. Reasonable.

**Large-doc performance.** The incremental planner + assignment dependency tracking is the right design. The issue is the code path around it: `recompute_calc_full` in TUI spends most of its time rebuilding the four parallel vectors, not evaluating. A single `LineMetadata` struct per line, maintained incrementally, would eliminate the rebuild.

### D.4 UI (`src/editor/*`, CodeMirror)

**Architecture.** CodeMirror 6 host. Extensions compose features:
- `calcExtensions` — calc decoration, widgets, trailer refresh, incremental scheduling.
- `markdownRichTextExtensions` — token decorations, hidden tokens, code highlight.
- `markdownEditingExtensions` — rules (enter/tab/doc change), table navigation.
- `foldingExtensions` — fold structure + fold gutter + commands.
- `variableAutocompleteExtensions` — popup, completion.
- `notifyExtensions` — reminders, decorations.
- `vimModeExtension` — vim dispatcher.
- `commandModeExtension` — `:command` bar.
- `editorSearchExtensions` — `/` search.

**Mounting.** `src/editor/editor.ts::mountEditor` builds extensions, creates `EditorState`, instantiates `EditorView`. `reconfigureEditor` creates a new `EditorState` with the same doc, preserving selection.

**Strengths.**
- **Correct use of CM 6 primitives.** `StateField`, `StateEffect`, `ViewPlugin`, `Decoration.mark/widget/replace`, `RangeSetBuilder`. Decorations are viewport-scoped.
- **Commit-mark pattern for calc trailer refresh.** `CommitMark extends RangeValue` with `StateField<RangeSet<CommitMark>>`. Move-through-doc-changes semantics handled correctly via `RangeSet.map`. Well done.
- **`setEditorContent` large-doc path.** Uses `view.setState` when doc exceeds 200,000 chars to avoid gigantic change transactions.
- **`api.ts` is the single Tauri entry point.** 29 `invoke` calls, nothing else. Editor files depend on it for *types* but never for IPC.

**Weaknesses.**

1. **`calc-decoration.ts` is a 1549-line responsibility dumpster.** Contains: `StateEffect` and `StateField` definitions, commit-mark management, `CalcResultWidget`, `FormulaCellWidget`, calc result remapping across doc changes, variable index remapping, per-cell result remapping, incremental planner glue, eval-window computation, trailer refresh computation, multi-range scan helpers (`lineHasCalcGlobalSyntax`, `visibleSpansTouchCalcSyntax`, etc.), the `calcDecorationsPlugin`, plus the top-level `calcExtensions` factory. **Split into:** `calc-state.ts` (effects/fields), `calc-widgets.ts` (widget classes), `calc-remap.ts` (doc-change remapping), `calc-scheduler.ts` (eval scheduling + windowing), `calc-decoration.ts` (the ViewPlugin only).

2. **`vim.ts` (1190 lines)** largely duplicates logic that `editor-core::vim_actions` already implements. The plan explicitly says "Keep high-frequency UI movement/insert interactions local when wasm boundary cost is measurable" — but a lot of what's in `vim.ts` is not movement/insert hot path (e.g., `shouldExecuteSharedVimAction` has 20-case match). The dispatcher itself is fine; the 500+ lines of private helpers are candidates for migration or for sharing with TUI's `vim_actions.rs`.

3. **`markdown-decoration.ts` (1207 lines)** does its own viewport scanning + fence checkpoints. Parallel to TUI's `fence_checkpoints` in `src-tauri/src/terminal/app/editing.rs`. Both reimplement the same idea with subtly different constants (`FENCE_CHECKPOINT_INTERVAL = 256` in TUI vs UI). Potential drift.

4. **Module-level mutable singletons in multiple files.** `editor.ts`: `view`, `saveTimer`, `mountedExtensions`, `localDirty`, `saveInFlight`, `suppressProgrammaticDocSync`, `currentFormatOnSave`, `backendDetached`, `tableModuleEnabled`. These make the editor non-reentrant and hard to test. Refactor into a class or a factory-returned controller.

5. **`app.ts` (1006 lines)** mixes too much: note session lifecycle, backend-change listening, module persistence, switcher open, save dialog, font cycling, keyboard shortcuts. Should be split.

6. **Repeated scans in `calc-decoration.ts`.** `rangeTouchesCalcGlobalSyntax`, `rangeTouchesCalcExpression`, `visibleSpansTouchCalcSyntax`, `updateTouchesCalcGlobalSyntax`, `updateTouchesCalcExpression` — all walk changed/visible ranges with near-overlapping predicates. Each crosses the wasm boundary (`calcLineUsesAssignmentPrefix`, `findCalcSegment`, `builtinFormulaLabels`) per line. A single pass that reports all three flags would reduce wasm boundary calls significantly.

7. **`setEditorContent` large-doc threshold of 200,000 chars** is too low for a "notes app with large docs" claim. A 200KB note is ~5,000 lines of prose. The threshold seems to guard against a specific CodeMirror slowdown on giant transactions — would be worth measuring current CM behavior and raising, or using `view.setState` unconditionally for programmatic `setEditorContent` calls.

**Performance.**
- Decoration builds are viewport-scoped, which is correct.
- Wasm calls are not batched beyond `wasm_run_markdown_transaction_batch`. Markdown token analysis, inline marker ranges, and code tokenization are separate calls per line per build.
- The `FORMULA_SEGMENT_CACHE` (512 entries) is a good amortization. `inlineMarkerRangeCache` (1024 entries, `markdown-decoration.ts`) similar. These are per-process; good.

### D.5 TUI (`src-tauri/src/terminal/*`)

**Architecture.** `run_terminal_session` → `TerminalApp::new_with_startup_metrics` → `app.run(db)` loop: `draw → read_key → handle_key → maybe_autosave`.

The split committed in `405144d` produced:
- `app/mod.rs` (825 LOC) — struct def + top-level methods + `run_terminal_session`.
- `app/editing.rs` (2013 LOC) — cursor motion, calc recompute, folding, undo/redo.
- `app/rendering.rs` (1027 LOC) — `draw()`, row diff, status composition.
- `app/command_search_switcher.rs` (1344 LOC) — command bar, search, switcher, date picker dispatch.
- `app/vim_actions.rs` (1114 LOC) — vim intent execution.
- `app/input_modes.rs` (560 LOC) — key dispatch.
- `app/calc_helpers.rs` (172 LOC) — `compute_calc_data`, trailer refresh.
- `app/table_helpers.rs` (107 LOC) — table cursor/info.
- `app/reminder_helpers.rs` (202 LOC) — reminders.
- `app/tests*.rs` — golden replay + per-domain tests.

**Strengths.**
- Fence-state checkpointing (`FENCE_CHECKPOINT_INTERVAL = 256`, `fence_state_before_line`, `invalidate_fence_checkpoints_from_line`) is the right shape: O(sqrt(N)) scan ceiling per draw instead of O(N).
- Frame-row diffing (`last_drawn_rows`, `split_frame_rows`) avoids redrawing unchanged rows.
- Async-style deferral: `rescan_pending` for folds, `calc_recompute_pending` for calc, `reminders_dirty` — idle-tick driven. Good for typing latency.
- Startup metrics (`TerminalStartupMetrics`) give real observability.
- Terminal-side calc viewport-only mode (`CALC_VIEWPORT_ONLY_MIN_LINES = 2000`) — sensible escape valve for large notes.

**Weaknesses.**

1. **`TerminalApp` god struct.** Fields cross almost every concern in the system:
   - Document model: `active_note`, `lines`, `cursor_line`, `cursor_col`, `scroll_line`, `scroll_col`, `dirty`.
   - Mode: `mode`, `command_bar_from_normal`.
   - UI overlays: `switcher_*` (8 fields), `date_*` (9 fields), `command_input`, `command_completion`, `search_*` (6 fields), `variable_autocomplete_popup`.
   - Clipboard: `clipboard`, `last_clipboard_backend`, `clipboard_watch_*` (3 fields).
   - Vim: `vim_state`, `selection_anchor`, `command_selection*` (2 fields).
   - Calc: `calc` (CalcCache), `calc_recompute_pending`, `calc_viewport_only`, `calc_last_view_eval_range`.
   - Reminders: `reminder_ghosts`, `reminders_dirty`, `last_reminder_check`.
   - Folding: `folds`.
   - Render: `render_palette`, `fence_checkpoints`, `fence_checkpoints_valid_through`, `draw_buf`, `last_drawn_rows`, `last_drawn_rows_dim`, `last_cursor_*` (3 fields).
   - Config flags: `format_on_save`, `markdown_autoformat`, `checklist_auto_reorder`, `variable_autocomplete_min_chars`, `date_format`, `date_time_format`.
   - History: `history`.
   - Quit: `quit`, `force_quit`, `status`.

   ~80 fields, ~30 concerns. Any method can touch any field. Testing is borough-wide.

2. **Parallel per-line vectors.** `CalcCache` holds:
   ```
   prev_line_hashes: Vec<u64>
   prev_line_assignment_name: Vec<Option<String>>
   prev_line_has_assignment: Vec<bool>
   prev_line_has_builtin_formula: Vec<bool>
   results: Vec<Option<String>>
   cell_results: Vec<Vec<(usize, String)>>
   ```
   Plus `FoldingState::line_has_structure: Vec<bool>` lives alongside. Plus `lines: Vec<String>` itself. That's 7 parallel `Vec`s of length `lines.len()`. Each edit potentially rebuilds all of them.

3. **`recompute_calc_full` rebuilds 4 vectors per call.** Lines 887–904 in `editing.rs`:
   ```rust
   self.calc.prev_line_assignment_name = self.lines.iter().map(|line| ... assignment_name(line)).collect();
   self.calc.prev_line_has_assignment = self.lines.iter().map(|line| ... contains_assignment_operator(line)).collect();
   self.calc.prev_line_has_builtin_formula = self.lines.iter().map(|line| ... contains_builtin_formula(...)).collect();
   ```
   Each is O(N) per edit where N = total lines. The planner only needs the changed range to be updated; the rest is unchanged. **Fix:** incremental update alongside the line mutation.

4. **`recompute_folding` is mostly O(N).** `editing.rs` has the fast path for "no active folds → update one entry + rebuild view map", but the fallback is `build_fold_ranges(&self.lines)` over the whole doc. For 100k lines this is `markdown_tokens::analyze_lines` over the whole doc. The `FoldLineEdit` / `map_ranges_through_line_edits` in `editor-core::folding` is designed for incremental updates but the TUI does not use that path.

5. **Rendering hot path allocates heavily.** In `draw()` per line:
   - `line_text.to_string()` once (`rendered_line`).
   - For formula rows: `String::with_capacity(...)` + multiple `push_str`.
   - `format!(...)` for trailer parts, markers.
   - `ctx.render_line_window_with_reminder_cursor(...)` returns a `String`.
   
   At ~60 FPS on a 100-row viewport with formula rows this is not free. Profile candidates: reuse buffers across rows; replace `format!` with write-to-buffer.

6. **`split_frame_rows` re-parses ANSI goto sequences.** The frame is generated with goto sequences, then re-parsed to split into rows for diff. A structured row-by-row builder would avoid the re-parse.

7. **Parallel implementations with UI.** `table_cell_info_at_char`, `move_cursor_left_word`/`move_cursor_right_word`, `line_has_fold_structure` all have TS equivalents. Drift risk.

**Calc orchestration in TUI** is more complex than UI's because it lacks CodeMirror's transaction model. It works but reads like a state machine with many guards (`should_defer_calc_recompute`, `can_skip_calc_recompute`, `calc_viewport_only`, `skip_initial_calc`). **Extract a `CalcOrchestrator` struct** that owns the decision tree; the TUI becomes a caller.

### D.6 Platform/framework separation

**What is Tauri-specific.**
- `src-tauri/src/lib.rs::run_gui` — `tauri::Builder`, `invoke_handler`, `generate_handler!`, `State<'_, AppCore>`, `AppHandle::emit`.
- `src-tauri/src/commands/*.rs` — every file uses `#[tauri::command]` and `State<'_, AppCore>` / `AppHandle`.
- `src-tauri/src/ipc/server.rs` — Tauri-specific setup.
- `src/main.ts` — `@tauri-apps/api/window`.
- `src/app.ts` — `@tauri-apps/plugin-dialog`, `@tauri-apps/api/window`, `@tauri-apps/api/event`.
- `src/api.ts` — `@tauri-apps/api/core::invoke`.

**What is portable.**
- `crates/editor-core` — fully portable.
- `crates/app-core` — portable (directories, rusqlite, fend-core, aes-gcm, pbkdf2). No Tauri.
- `src/editor/*` — depends on CodeMirror + `api.ts` types. No Tauri imports.
- `src-tauri/src/terminal/*` — no Tauri imports; uses `app-core` directly.
- `src-tauri/src/editor_core/*` — re-export shim over `editor_core` crate.

**Assessment.** Good containment. Framework replacement cost:
- **Rewrite `api.ts`.** 321 lines, single file, no deep coupling.
- **Rewrite `app.ts`** or refactor it to consume a platform-abstraction interface. Currently it reaches into Tauri directly in ~5 places.
- **Rewrite `src-tauri/src/lib.rs::run_gui`** and every `#[tauri::command]` in `commands/*.rs`. Switch to e.g. Qt would mean hosting a QWebEngine + QWebChannel bridge mirroring the invoke surface, or moving to a pure-Rust UI (iced, egui) and reimplementing the web layer.

**Gaps to portability.**
1. **No `BackendClient` interface.** `api.ts` calls `invoke<T>(...)` inline 29 times. Define an interface:
   ```typescript
   export interface BackendClient {
     getOrCreateNote(): Promise<Note>;
     saveNote(id: string, body: string): Promise<Note>;
     // …
   }
   ```
   Implementations: `TauriBackend`, `HttpBackend`, `MockBackend`. `api.ts` becomes a Tauri-specific file; the rest of UI consumes the interface.
2. **`NOTE_CHANGED_EVENT` listener is inline in `app.ts`.** Should be part of the `BackendClient` subscription API.
3. **`src-tauri/src/editor_core/mod.rs` is a shim.** Delete it, change callers to import `editor_core::*` directly. No behavior change.
4. **`tauri::State<'_, AppCore>` is fine as-is** — the abstraction layer is `AppCore`, Tauri just owns the reference. Any other runtime can manage the same `AppCore` with `Arc<AppCore>`.

**Specifically for Qt:** the rational move is keeping the web layer (Qt's `QWebEngine` hosts it) and implementing the invoke surface via `QWebChannel`. The existing `BackendClient` abstraction would make this a matter of implementing one class. If moving to native Qt, the whole `src/` rewrite is unavoidable regardless of abstraction — the value of the abstraction is in `editor-core` + `app-core`, which already compile for any target.

---

## E. Performance Review

### Bottlenecks found

| # | Location | Severity | Why it matters | Likely root cause |
|---|----------|----------|----------------|-------------------|
| 1 | `TerminalApp::recompute_calc_full` — 4× O(N) vector rebuilds per edit | High | Keystroke latency scales linearly with total lines on any doc with assignments/formulas | Vector rebuild instead of incremental update |
| 2 | `ResolvedContext::from_parts` → `parse_lines` → `Vec<String>` per call | High | Every shared-core rule call re-allocates N strings | `ParsedLines::lines` stores owned `String`s, parse copies substrings |
| 3 | `EditorContextSnapshot { text: String }` clones the whole doc at wasm boundary | High | `text.to_string()` at 3 call sites in `wasm.rs` — every snapshot-taking rule pays | Snapshot is `Serialize`-able but carries owned text |
| 4 | `commands::calc::evaluate_note_context_delta` is sync Tauri command | High | Blocks the IPC thread for any concurrent command | No async wrapper around fend |
| 5 | `fend_core::Interrupt = NoInterrupt` | High | Long calcs cannot be cancelled, no timeout | Hardcoded |
| 6 | Single `Mutex<Connection>` in `app-core::storage::sqlite::Db` | Medium | All DB ops serialize; calc cache contends with save | No pool |
| 7 | Rendering full frame then row-diff via ANSI parse | Medium | Per-frame cost ~O(viewport_height × style_ops); ANSI re-parse adds ~2x | Frame-first architecture |
| 8 | TUI `recompute_folding` full rebuild on most edits | Medium | `analyze_lines` over N lines per edit | Fast path (no collapsed folds) exists but many real edits fall back to full rebuild |
| 9 | Wasm marshaling via `Reflect.set` per field | Medium | Every calc/folding/vim call pays O(fields) JS property sets | Hand-rolled encoders |
| 10 | UI calc scope detection runs 3+ line-scan passes per update | Medium | Each pass crosses the wasm boundary per line | Distinct predicates not fused |
| 11 | `table_pipe_positions` reparses every call | Low | Cheap per line, but called from every render + every rule | No per-line cache in TUI |
| 12 | `note_line_cache: Mutex<HashMap<String, Vec<String>>>` in `app-core` | Low | Accumulates over process lifetime, contends with calc | No eviction, no per-note unique lock |
| 13 | `collect_variable_definitions` full-scan per eval | Low | Redundant when the outer layer already knows `has_variable_assignment` | No parameter to skip |
| 14 | 7 parallel per-line `Vec`s in TUI state | Low | Memory and cache-miss churn on large docs | Missing `LineMetadata` struct |
| 15 | `FORMULA_SEGMENT_CACHE` 512 entries (UI) / no cache (TUI) | Low | TUI reparses formula segments per render frame | Asymmetric amortization |

### Estimated impact

Rough, uninstrumented:
- Fix (1) + (2): **5–20× faster keystroke on 50k-line notes with calc**. This is the headline win.
- Fix (3) + (9): **~30% smaller wasm bridge, 2–3× fewer allocations per call** on hot paths that snapshot.
- Fix (4) + (5) + (6): **unblock IPC during calc, enable cancellation**, no direct latency number but unblocks the app during long calcs.
- Fix (8) + (10): **2–4× faster TUI draw loop on large docs** with folds or calc.

---

## F. Recommended Improvements

### Short-term (1–2 weeks)

1. **Replace hand-marshaled wasm boundary with `serde-wasm-bindgen`.** Remove `wasm.rs` encoders (800+ lines gone) and `wasm.ts` decoders (mirror savings). No wire-format change if done carefully.
2. **Make calc Tauri commands async.** `#[tauri::command] async fn evaluate_note_context(…)` + move fend to `spawn_blocking`. Unblocks IPC.
3. **Wire `fend_core::Interrupt` to a cancellable flag.** Flip on subsequent edits; calc aborts cleanly.
4. **Replace `Mutex<Connection>` with a read/write pool.** `deadpool-sqlite` or `r2d2`. WAL mode already enabled.
5. **Stop rebuilding 4 parallel vectors in `recompute_calc_full`.** Maintain them incrementally alongside `lines`.
6. **Delete `src-tauri/src/editor_core/mod.rs` shim** and rename callers to use `editor_core::*` directly. Pure cleanup.
7. **Consolidate `calc-decoration.ts` viewport/change scans into one pass.** Single pass emits all 3 flags.
8. **Delete dead path `evaluate_lines` in both `CalcEngine` and `commands/calc.rs`** if truly unused.

### Medium-term (2–6 weeks)

1. **Decompose `TerminalApp`.** Split into `EditorModel`, `ViewportState`, `OverlayState`, `CalcRuntime`, `FoldRuntime`, `RenderCache`. Each with its own `impl`. Methods move from god-struct to focused structs; public API of `TerminalApp` becomes orchestration.
2. **Share `LineMetadata` between TUI and UI.** One struct per line: `{ hash, assignment_name, has_assignment, has_builtin_formula, has_fold_structure }`. Updated incrementally. Lives in `editor-core::calc_plan` or a new `editor-core::line_index`.
3. **Extract a shared `TableModel` in `editor-core::table`.** Parse once per line (or per block), expose cell spans + formula segments + normalization. Kill duplicate parsers in `calc_plan` and `app-core::calc::engine`.
4. **Introduce a `BackendClient` interface in UI.** Wrap `invoke` calls; inject into `app.ts`. One implementation today (`TauriBackend`), groundwork for portability.
5. **Split `calc-decoration.ts` into 4 files** (state, widgets, remap, scheduler).
6. **Fold parity suite + calc parity suite.** Golden fixtures as for vim/markdown.
7. **Reduce `EditorContextSnapshot` footprint.** Change API to pass `(text: &str, selection, changed_range)` tuples; avoid `text: String` in the snapshot type entirely.
8. **TUI fold incremental path.** Use `editor-core::folding::map_ranges_through_line_edits` as the primary path, fall back to full rebuild only on structural edits.

### Long-term (quarter+)

1. **Rope-backed document model in `editor-core`.** Replace `Vec<String>` materialization with a `Rope` (e.g., `ropey` crate) stored per document. Both UI and TUI wrap it. Eliminates line-materialization cost on large docs.
2. **Full shared-core execution path for high-frequency vim commands that are not hot insert-mode.** The plan's "migrate non-hot-path" item. Goal: `vim.ts` and `terminal/app/vim_actions.rs` shrink to adapter glue.
3. **Full editor state manageable outside the framework.** Ship an `EditorSession` in `editor-core` that both CM and the TUI consume as their single source of truth; CM becomes a view layer, the TUI becomes a renderer. This is a significant rewrite but is the clean endpoint of the plan.
4. **Pluggable evaluation backend.** Abstract `CalcEvaluator` trait with a fend-backed default implementation. Opens the door to Wasm-side calc for offline UI (no Tauri round-trip) or alternative engines.
5. **Structured `Table` data type** with multi-row cells (plan backlog item) and column formula aggregation. Required for the future table roadmap.

---

## G. Prioritized Action List

| # | Problem | Recommended change | Expected benefit | Risk / Complexity |
|---|---------|---------------------|------------------|-------------------|
| A1 | Hand-written wasm marshaling in `wasm.rs` + mirror in `wasm.ts` (~3300 LoC combined) | Adopt `serde-wasm-bindgen` or `tsify` | Delete ~1500 LoC; new features add zero plumbing | Low: wire-compatible if done deliberately |
| A2 | `TerminalApp` god struct (~80 fields) | Decompose into focused sub-structs; move methods accordingly | Fewer cross-cutting bugs, easier testing | Medium: large mechanical refactor; coordinate with current splits |
| A3 | `recompute_calc_full` rebuilds 4 parallel `Vec`s O(N) per edit | Maintain `LineMetadata` incrementally | 5–20× faster keystroke on large calc docs | Medium: invariant changes; need tests for every edit path |
| A4 | `EditorContextSnapshot` clones full doc; `parse_lines` clones per line | Switch to `&str` + on-demand line view (Rope or `[&str]`) | ~30% less allocation on hot path; unblocks larger docs | Medium: API change across shared-core |
| A5 | Tauri calc commands are sync; fend has no interrupt | `async` wrappers, `spawn_blocking`, `Interrupt` with `AtomicBool` | Unblock IPC during calc; cancellation on new edits | Low: localized |
| A6 | Single `Mutex<Connection>` | Connection pool (deadpool-sqlite) | Save/list/calc stop contending | Low-medium: small migration |
| A7 | No `BackendClient` interface in UI | Introduce one; keep Tauri impl default | Framework portability; mock-friendly tests | Low: mechanical |
| A8 | `calc-decoration.ts` 1549 LoC mixes 6 concerns | Split into 4 files (state/widgets/remap/scheduler) | Ownership clarity; shrinks review area | Low |
| A9 | Parallel table segment/formula parsers in `calc_plan` vs `app-core::calc::engine` | Share one parser in `editor-core::calc_plan` | One place to fix bugs | Low-medium |
| A10 | TUI `recompute_folding` full rebuild on most edits | Use `map_ranges_through_line_edits` as primary path | 2–4× faster TUI draw on large docs with folds | Medium: current code has subtle invariants |
| A11 | Wasm `VimIntent` numeric mapping in three files | Serde-based enum transport; drop numeric IDs | Adding intents requires editing one place | Low once A1 is done |
| A12 | `text_rules.rs` (2152 LoC) and `calc_plan.rs` (1620 LoC) are monolithic | Split into submodules by concern | Easier navigation, smaller review diffs | Low: pure file moves |
| A13 | UI calc scope detection triple-scans viewport/changes | Single-pass flags | Fewer wasm round-trips per update | Low |
| A14 | No calc or fold parity suite | Add golden fixtures mirroring vim/markdown | Prevent adapter drift | Low |
| A15 | Rendering re-parses ANSI goto sequences to split rows | Emit structured per-row frames | Lower draw-loop cost | Medium |
| A16 | Tauri-specific mix in `src/app.ts` (1006 LoC) | Extract `note-session`, `backend-sync`, `keyboard-shortcuts` modules | Maintainability, port cost | Medium |
| A17 | Dead `CalcEngine::evaluate_lines` + exposed Tauri command | Delete | Smaller surface | Trivial |
| A18 | `src-tauri/src/editor_core/mod.rs` re-export shim | Delete; update imports | Less indirection | Trivial |
| A19 | Encrypted note sessions have no timeout | Add TTL eviction to `NoteAccessService` | Security posture | Low |
| A20 | `note_line_cache` has no eviction | LRU or size-bounded | Memory growth over long sessions | Low |

---

## Top 10 Architecture Issues

1. **Hand-marshaled wasm boundary** (`wasm.rs` + `wasm.ts` ≈ 3700 LoC combined, growing).
2. **`TerminalApp` god struct** (~80 fields, ~30 concerns, untouched by the recent file split).
3. **`EditorContextSnapshot` full-doc clone** on every rule invocation.
4. **Parallel per-line `Vec`s in TUI state** (7 deep) instead of one `LineMetadata`.
5. **Duplicated table parsers** in `editor-core::calc_plan` and `app-core::calc::engine`.
6. **No `BackendClient` abstraction** between UI and Tauri IPC.
7. **`text_rules.rs` + `calc_plan.rs` monoliths** (~3700 LoC of mixed concerns).
8. **UI–TUI semantic duplicates** (`vim.ts` ↔ `terminal/app/vim_actions.rs`, table navigation, fence checkpoints).
9. **`calc-decoration.ts` owns 6 concerns in one 1549-LoC file.**
10. **No parity suite for calc or folding** — drift is only caught post-hoc.

## Top 10 Performance Issues

1. **`recompute_calc_full` rebuilds 4 parallel O(N) `Vec`s** per edit.
2. **`ResolvedContext` line materialization** (`Vec<String>` allocations per call).
3. **Sync Tauri calc commands** blocking the IPC thread.
4. **Hardcoded `NoInterrupt`** prevents cancellation.
5. **Single `Mutex<Connection>`** serializes all DB ops.
6. **TUI `recompute_folding` full rebuild** path chosen on most edits.
7. **Multi-pass calc scope detection** in UI crossing wasm per line per pass.
8. **TUI render: full frame + ANSI re-parse diff** per redraw.
9. **`EditorContextSnapshot.text: String`** clone on wasm boundary per call.
10. **Table parsing repeated** per render in TUI (no per-line cache).

## Top 10 Recommended Next Steps

1. Land **serde-based wasm marshaling** (A1, A11) — biggest lever per LoC removed.
2. Make **calc async + interruptible** (A5) — unblocks IPC thread, foundation for cancellation.
3. Introduce **`LineMetadata`** in shared core (A3) — turns O(N) per-edit rebuilds into O(Δ).
4. Decompose **`TerminalApp`** (A2) — pays off every future TUI change.
5. Replace **`Mutex<Connection>` with a pool** (A6) — low effort, removes save/calc contention.
6. Introduce **`BackendClient` interface** (A7) — puts UI portability on rails.
7. Split **`calc-decoration.ts` into 4 files** (A8) — makes the calc pipeline comprehensible.
8. Consolidate **table parsing into one `TableModel`** (A9) — prerequisite for the multi-row-cell roadmap item.
9. Add **calc + folding parity suites** (A14) — mandatory before further shared-core migrations.
10. Switch **TUI folding to the incremental remap path** (A10) — the code already exists in core.

---

## Appendix: Evidence Map

- Plan: `docs/plan.md` — lines 47–161 (subsystem plans), 131–154 (cross-subsystem actions).
- Editor core: `crates/editor-core/src/{lib.rs,engine.rs,vim.rs,vim_actions.rs,text_rules.rs,calc_plan.rs,folding.rs,table.rs,markdown_tokens.rs,context.rs,types.rs,wasm.rs}`.
- App core: `crates/app-core/src/{lib.rs,calc/engine.rs,storage/sqlite.rs,config/mod.rs}`.
- TUI: `src-tauri/src/terminal/{adapter.rs,folding.rs,calc_cache.rs,app/{mod.rs,editing.rs,rendering.rs,command_search_switcher.rs,vim_actions.rs,calc_helpers.rs,table_helpers.rs}}`.
- Tauri commands: `src-tauri/src/commands/{notes.rs,calc.rs}`, `src-tauri/src/lib.rs`.
- UI: `src/{api.ts,app.ts}`, `src/editor/{editor.ts,wasm.ts,calc-decoration.ts,markdown-decoration.ts,markdown-editing.ts,vim.ts,folding.ts}`.
- Recent commits reviewed: `6b11296`, `934c39e`, `8a3ae2d`, `30936b6`, `c81fa8b`, `b8665fe`, `405144d`, `a12d0c2`, `770edab`, `25ee927`, `9f30dd3`, `ae34451`, `21762f2`, `601f1c1`, `a6eb119`, `1d79a72`, `9a74d00`, `7c905e0`, `6d64473`, `93a101f`.
