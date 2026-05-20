# Architecture and Performance Review — Deferred Work

Working list of architecture/performance items that survived the most recent review pass. Items kept across reviews are noted; items removed have been completed and merged into `roadmap/plan.md`.

Recently shipped (this pass) and dropped from the table:

- TUI wiki-link cache eviction was O(n) `min_by_key` on every insert past the cap. Both `wiki_link_render_cache` (2048 cap) and `wiki_link_line_render_cache` (1024 cap) now use a parallel `VecDeque<String>` insertion-order queue for O(1) FIFO eviction.
- `InlineTokenCache` (`terminal/render.rs`, 4096-entry cap) and `table_formula_segment_cache` (`terminal/app/rendering.rs`, 2048-entry cap) converted to the same VecDeque eviction pattern. Dropped the now-unused `tick`-based LRU bookkeeping from `InlineTokenCache`.
- TUI autosave path used to clone the entire document via `joined_text_cached()` on every save. `save_with_options` now uses `joined_text_cache.take().unwrap_or_else(...)` to move the owned String into the save call and reinsert it after; zero copies on a warm cache.
- TUI switcher fuzzy-match recomputation allocated a fresh `Vec<(usize, i32)>` and called `sort_by` on every keystroke. It now reuses a `switcher_score_scratch` field and uses `sort_unstable_by`.
- TUI command completion options were emitted in `COMMAND_DEFINITIONS` insertion order; they are now sorted alphabetically before truncation so the menu doesn't reorder by registration history.
- Calc gating for large notes was over-restrictive: `note_math_module_enabled` / `note_table_module_enabled` / `note_variables_module_enabled` all AND-ed with `!large_note_reduced_features()`, so 100k-line notes with explicit calc syntax silently dropped results. The math/table/variables gates now follow raw `modules.*` (matching the runtime `calc_viewport_only` configuration); only the style gate keeps the large-note cutoff because rendering is the legitimate motivation for it.

---

## 1. TUI deduplication vertical slice (carry-over)

**Problem**
`src-tauri/src/terminal/app/editing.rs` (4018 lines), `vim_actions.rs` (1386 lines), and `command_search_switcher.rs` (2791 lines) still re-implement semantics that already live in `editor-core`. Confirmed duplications as of this pass:

- `terminal/app/vim_actions.rs:353-487` — `find_word_object_bounds`, `find_pipe_object_bounds`, `apply_word_text_object`, `apply_pipe_text_object`. Parallel implementations of word/pipe text-object selection. Shared core already has the canonical version at `crates/editor-core/src/vim_actions.rs:1089-1190` reachable via `execute_word_text_object` / `execute_pipe_text_object`, which the UI path calls through `wasm_execute_vim_action`.
- `terminal/app/editing.rs:383-518` — `rescan_calc_flags`, `update_calc_flags_incremental`, `rebuild_calc_line_metadata`, `refresh_calc_line_metadata_at`, `splice_calc_line_metadata`. These duplicate `calc_plan::detect_calc_signal_flags_with_mask`, `contains_variable_assignment_with_mask`, `contains_builtin_formula_with_mask`, and `line_metadata_for_lines_with_mask`. The TUI versions add incremental bookkeeping that core does not, so the migration is more than a one-liner — but the per-keystroke logic should live in core with the TUI calling through.

Every duplicated primitive is a latent divergence bug: a fix to core semantics has to be manually replicated in the TUI, and the TUI's version will quietly drift.

**Approach**
Pick one primitive per sitting as a vertical slice. Recommended starting point: word text objects, because they are self-contained and have clear test coverage in editor-core.

1. Add a WASM-less entry point in `editor-core` for the text-object operation if one doesn't exist (some already do via `wasm_execute_vim_action`).
2. Route the TUI call through it — pass the line text in, get back the (start, end) byte range.
3. Delete the TUI-local impl.
4. Run the parity test suite (`terminal/app/tests/parity.rs`) to confirm identical behavior.
5. Repeat for pipe objects, then calc metadata helpers.

**Risk**
The TUI calls these synchronously on every keypress, so the core fn must not regress in hot-path cost. Verify with `cargo bench` or a manual timing check after each slice. The `apply_*` wrappers also manage TUI register/cursor/clipboard state, so the call site needs to keep that orchestration on the TUI side while delegating only the pure bounds/diff calculation to core.

---

## 2. Delta-based decoration rebuild in the web frontend (carry-over, partially mitigated)

**Problem**
`src/editor/markdown-decoration.ts` (2327 lines) and `src/editor/calc-decoration.ts` (2026 lines) still rebuild their CodeMirror `RangeSet` over the entire visible viewport for every transaction that signals a structural change. The previous review noted this was true on every keystroke; the situation has improved but is not solved.

Current state (verified `markdown-decoration.ts:2033-2187`, `calc-decoration.ts:846-904`):

- The immediate path for in-viewport edits already uses `this.decorations.map(update.changes)` + a debounced `scheduleDeferredRefresh` — typing latency no longer pays the full O(viewport) cost each keystroke.
- `safeBuild` (full viewport scan via `buildMarkdownDecorationsForSpans` / `buildCalcDecorations`) still runs unconditionally on: `varsChanged`, `viewportChanged`, `selectionSet` that intersects checklist/transparent-markdown reveal, table-row edits (`updateTouchesTableRows`), and every deferred refresh after typing settles.
- Two separate decoration builders still iterate the same spans per dispatch (markdown decorations, calc decorations). Each safe-build rebuilds independently.
- `calc-incremental.ts::planIncrementalCalc` already computes the changed line range for calc evaluation; the same delta is not reused for decoration rebuild.

**Approach**

1. Reuse the changed-line range computed by `calc-incremental.ts` (or compute the equivalent from `update.changes.iterChangedRanges`) so the deferred markdown rebuild can target only changed lines + their fence/table context, then splice the result into the previous `RangeSet` via `existing.map(update.changes)` + a small `RangeSetBuilder` for the changed slice.
2. Eliminate the second `RangeSetBuilder` pass for table widgets in markdown — fold table widget placement into the same single-pass builder that processes inline tokens.
3. Coordinate markdown and calc decoration update cycles so they share one viewport scan per dispatch instead of running two independent O(viewport) iterations on every `safeBuild`.

**Constraint**
CodeMirror requires decorations to be added in document order within a `RangeSetBuilder`. Track changed ranges by start position and process in order. Keep the current debounced full safeBuild as a fallback for viewport-change-only events where no line content changed.

**Expected gain**
Keystroke decoration latency on large documents (>5k lines) drops from O(viewport size) to O(changed lines), which for typical single-line edits is O(1) in practice. Closes the gap that the immediate-`map` fast path opened.

---

## 3. `TerminalApp` state decomposition (carry-over from `plan.md`)

**Problem**
`src-tauri/src/terminal/app/mod.rs:375` declares `TerminalApp` with ~120+ flat fields covering: editor model, switcher, collection switcher, content search, wiki-link autocomplete, variable autocomplete, vim register/macros, calc cache + scheduling, search, undo history, folding, clipboard watch, render palette, date picker, command bar, perf trace state. Any per-subsystem change touches the same monolith, blocking subsystem-scoped diffs and making lifetime/borrow constraints conflate unrelated state.

This was already listed in `roadmap/plan.md`'s Next Sprint Checklist as `TerminalApp state decomposition (EditorModel, CalcRuntime, OverlayState, RenderState, FoldRuntime)`. It remains unstarted.

**Approach**
Decompose by subsystem in small slices. Suggested first slice: pull the cache + scheduling state (`wiki_link_render_cache`, `wiki_link_render_cache_order`, `wiki_link_line_render_cache`, `wiki_link_line_render_cache_order`, `table_formula_segment_cache`, `joined_text_cache`, `switcher_score_scratch`) into a `RenderCaches` substruct. That's the lowest-risk slice because the caches have only render-time read sites and don't cross subsystem boundaries.

**Risk**
Borrow checker friction — many call sites hold `&mut self` across multiple substructs. Plan each slice to keep field access patterns ergonomic; back out if a slice forces awkward `&mut self.x` + `&mut self.y` partial-borrow gymnastics that the current flat struct hides.

---

## 4. `joined_text_cached()` still clones in non-autosave callers (lower-priority follow-up)

**Problem**
`command_search_switcher.rs:1876-1883` — `joined_text_cached()` always returns an owned `String` and seeds `joined_text_cache` with a clone. The autosave path was rewritten to use `joined_text_cache.take().unwrap_or_else(...)` (take-and-restore) and no longer pays the clone. Two callers still do: `build_snapshot` (line 1886) and the `Export` HostCommandPlan branch (line 1621). Both allocate a full document copy each invocation.

`build_snapshot` runs on every vim/command pipeline dispatch that goes through `execute_vim_action_with_target` against the full doc (the scoped-window branch already borrows from the cache without cloning, see `vim_actions.rs:185-196`). For large notes outside the scoped window, every such dispatch is an O(doc) allocation.

**Approach**
Either:

- Change `joined_text_cached()` to return `&str` borrowed from the cache, populating it from `&mut self` first. Callers that need an owned `String` (Export) can `.to_string()` explicitly. This makes the cache-hit case zero-copy for the common `build_snapshot` path.
- Or apply the same take-and-restore pattern to `build_snapshot` so it moves the owned String into the snapshot and returns it on drop. More work, but keeps the API uniform.

**Expected gain**
Removes one O(doc) clone per command dispatch on notes where the scoped-window snapshot isn't used. Lower priority than the autosave fix because it's not per-keystroke, but it lands on every `:` command and every vim action that doesn't qualify for the scoped window.

---

## Notes

- Items 1, 2, 3 are carried over from prior reviews and remain accurate; line numbers updated to current files.
- Item 4 is a new finding surfaced by following the same clone pattern fixed in the autosave path.
- Large-note tier policy (`adaptive large-note mode`, regression gates at `30k/100k/200k/400k`) lives in `roadmap/plan.md` Next Sprint Checklist and is not duplicated here.
- All execution backlog for measurement/operations references stays in `roadmap/performance.md`, `roadmap/perf-tracing.md`, `roadmap/perf-multirow-table.md` per the ownership note in `plan.md`.
