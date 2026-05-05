# Architecture Review — Deferred Work

Two projects from the May 2026 full-codebase review that are too large to do in one sitting but represent the highest long-term leverage.

---

## 4. TUI deduplication vertical slice

**Problem**  
`src-tauri/src/terminal/app/editing.rs` (3566 lines), `vim_actions.rs` (1177 lines), and `command_search_switcher.rs` (1970 lines) re-implement semantics that already live in `editor-core`. The most concrete duplication:

- `terminal/app/vim_actions.rs:287-459` — `find_word_object_bounds`, `find_pipe_object_bounds`, `apply_word_text_object`, `apply_pipe_text_object`. Parallel implementations of word/pipe text-object selection. The same logic exists in `crates/editor-core/src/vim_actions.rs` and should be the single source of truth.
- `terminal/app/editing.rs:303-435` — `rescan_calc_flags`, `update_calc_flags_incremental`, `rebuild_calc_line_metadata`, `refresh_calc_line_metadata_at`. These duplicate calc-state invalidation that `calc_plan::detect_calc_signal_flags_with_mask` / `line_metadata_for_lines_with_mask` already handle.

Every duplicated primitive is a latent divergence bug: a fix to core semantics has to be manually replicated in the TUI, and the TUI's version will quietly drift.

**Approach**  
Pick one primitive per sitting as a vertical slice. Recommended starting point: word text objects, because they are self-contained and have clear test coverage in editor-core.

1. Add a WASM-less entry point in `editor-core` for the text-object operation if one doesn't exist (some already do via `wasm_execute_vim_action`).
2. Route the TUI call through it — pass the line text in, get back the (start, end) byte range.
3. Delete the TUI-local impl.
4. Run the parity test suite (`terminal/app/tests/parity.rs`) to confirm identical behavior.
5. Repeat for pipe objects, then calc metadata helpers.

**Risk**  
The TUI calls these synchronously on every keypress, so the core fn must not regress in hot-path cost. Verify with `cargo bench` or a manual timing check after each slice.

---

## 5. Delta-based decoration rebuild in the web frontend

**Problem**  
Both `src/editor/markdown-decoration.ts` and `src/editor/calc-decoration.ts` rebuild their CodeMirror `RangeSet` over the entire visible viewport on every document change. Key locations:

- `markdown-decoration.ts:3527-3668` (`update()`) — calls `safeBuild()` on every `docChanged` or `viewportChanged`, rebuilding all decorations for visible + margin spans.
- `markdown-decoration.ts:2753-2785` and `calc-decoration.ts:633-665` (`buildCalcDecorations`) — `RangeSetBuilder` scans every visible line from scratch; O(viewport) per keystroke, not O(delta).
- Two separate builders iterate the same spans per dispatch (markdown decorations at `:2612`, table widgets at `:2793`, calc at `calc-decoration.ts:379`).

The correct fix is already modelled in `src/editor/calc-incremental.ts` — determine which lines actually changed, re-evaluate only those, and splice the result into the prior `RangeSet` using `RangeSet.map(changes)` for unchanged regions.

**Approach**

1. Extend the incremental model from `calc-incremental.ts` to markdown decorations. On each transaction, compute the changed line range from `update.changes.iterChangedRanges`. Re-tokenize only those lines via `wasm_markdown_analyze_lines` (already batched). Merge with the prior decoration set using `existing.map(update.changes)` + targeted `RangeSetBuilder` for changed lines only.
2. Eliminate the second `RangeSetBuilder` pass for table widgets — fold table widget placement into the same single-pass builder that processes inline tokens.
3. Fuse markdown and calc decoration updates into a single `ViewPlugin` pass or coordinate their update cycles so they share one viewport scan per dispatch instead of two independent O(viewport) iterations.

**Constraint**  
This requires careful handling of CodeMirror's decoration ordering invariant (decorations must be added in document order within a `RangeSetBuilder`). Track changed ranges by start position and process in order. The deferred `setTimeout`-based rebuild in `markdown-decoration.ts:3586` can be kept as a fallback for viewport-change-only events where no line content changed.

**Expected gain**  
Keystroke decoration latency on large documents (>5k lines) drops from O(viewport size) to O(changed lines), which for typical single-line edits is O(1) in practice.
