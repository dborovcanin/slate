# Architecture and Performance Review — Deferred Work

Working list of architecture/performance items that survived the most recent review pass. Items kept across reviews are noted; items removed have been completed and merged into `roadmap/plan.md`.

Recently shipped (prior pass) and dropped from the table:

- TUI wiki-link cache eviction was O(n) `min_by_key` on every insert past the cap. Both `wiki_link_render_cache` (2048 cap) and `wiki_link_line_render_cache` (1024 cap) now use a parallel `VecDeque<String>` insertion-order queue for O(1) FIFO eviction.
- `InlineTokenCache` (`terminal/render.rs`, 4096-entry cap) and `table_formula_segment_cache` (`terminal/app/rendering.rs`, 2048-entry cap) converted to the same VecDeque eviction pattern. Dropped the now-unused `tick`-based LRU bookkeeping from `InlineTokenCache`.
- `joined_text_cached()` replaced with `joined_text_cached_ref() -> &str`; `build_snapshot` calls `.to_owned()` once explicitly. Cache-hit path is now zero-copy.
- TUI autosave path used to clone the entire document via `joined_text_cached()` on every save. `save_with_options` now uses `joined_text_cache.take().unwrap_or_else(...)` to move the owned String into the save call and reinsert it after; zero copies on a warm cache.
- TUI switcher fuzzy-match recomputation allocated a fresh `Vec<(usize, i32)>` and called `sort_by` on every keystroke. It now reuses a `switcher_score_scratch` field and uses `sort_unstable_by`.
- TUI command completion options were emitted in `COMMAND_DEFINITIONS` insertion order; they are now sorted alphabetically before truncation so the menu doesn't reorder by registration history.
- Calc gating for large notes was over-restrictive: `note_math_module_enabled` / `note_table_module_enabled` / `note_variables_module_enabled` all AND-ed with `!large_note_reduced_features()`, so 100k-line notes with explicit calc syntax silently dropped results. The math/table/variables gates now follow raw `modules.*` (matching the runtime `calc_viewport_only` configuration); only the style gate keeps the large-note cutoff because rendering is the legitimate motivation for it.

This pass added items 4–6 (new findings) and refreshed line numbers on items 1–3.

---

## 1. TUI deduplication vertical slice (carry-over)

**Problem**
`src-tauri/src/terminal/app/editing.rs` (4227 lines), `vim_actions.rs` (1441 lines), and `command_search_switcher.rs` (2834 lines) still re-implement semantics that already live in `editor-core`. Confirmed duplications as of this pass:

- `terminal/app/vim_actions.rs:364-538` — `find_word_object_bounds`, `find_pipe_object_bounds`, `apply_word_text_object`, `apply_pipe_text_object`. Parallel implementations of word/pipe text-object selection. Shared core already has the canonical version at `crates/editor-core/src/vim_actions.rs:665-723` (`execute_word_text_object` / `execute_pipe_text_object`, with bounds helpers at `1180-1296`), reachable from the UI path via `wasm_execute_vim_action`.
- `terminal/app/editing.rs:387-540` — `rescan_calc_flags`, `update_calc_flags_incremental`, `rebuild_calc_line_metadata`, `refresh_calc_line_metadata_at`, `splice_calc_line_metadata`. These duplicate `calc_plan::detect_calc_signal_flags_with_mask` (`calc_plan.rs:757`), `contains_variable_assignment_with_mask` (`:730`), `contains_builtin_formula_with_mask` (`:747`), and `line_metadata_for_lines_with_mask` (`:81`). The TUI versions add incremental bookkeeping that core does not (single-line revalidation, line-count-delta splice), so the migration is more than a one-liner — but the per-keystroke signal-detection logic should live in core with the TUI calling through.

Every duplicated primitive is a latent divergence bug: a fix to core semantics has to be manually replicated in the TUI, and the TUI's version will quietly drift.

**Approach**
Pick one primitive per sitting as a vertical slice. Recommended starting point: word text objects, because they are self-contained and have clear test coverage in editor-core.

1. Add a WASM-less entry point in `editor-core` for the text-object operation if one doesn't exist (some already do via `wasm_execute_vim_action`).
2. Route the TUI call through it — pass the line text in, get back the (start, end) byte range.
3. Delete the TUI-local impl.
4. Run the parity test suite (`terminal/app/tests/parity.rs`) to confirm identical behavior.
5. Repeat for pipe objects, then calc metadata helpers.

**Risk**
The TUI calls these synchronously on every keypress, so the core fn must not regress in hot-path cost. Verify with `cargo bench` or a manual timing check after each slice. The `apply_*` wrappers also manage TUI register/cursor/clipboard state, so the call site needs to keep that orchestration on the TUI side while delegating only the pure bounds/diff calculation to core. The incremental calc bookkeeping (item above) must keep its single-line / splice fast paths — do not regress to a full `detect_calc_signal_flags` rescan per keystroke when delegating.

---

## 2. Delta-based decoration rebuild in the web frontend (carry-over, partially mitigated)

**Problem**
`src/editor/markdown-decoration.ts` (2396 lines) and `src/editor/calc-decoration.ts` (2030 lines) still rebuild their CodeMirror `RangeSet` over the entire visible viewport for every transaction that signals a structural change. The previous review noted this was true on every keystroke; the situation has improved but is not solved.

Current state (verified `markdown-decoration.ts:2130-2243`, `calc-decoration.ts:846-933`):

- The immediate path for in-viewport edits already uses `this.decorations.map(update.changes)` (`markdown-decoration.ts:2242`, `calc-decoration.ts:890`) + a debounced `scheduleDeferredRefresh` — typing latency no longer pays the full O(viewport) cost each keystroke.
- `safeBuild` (full viewport scan via `buildMarkdownDecorationsForSpans` / `buildCalcDecorations`) still runs unconditionally on: `varsChanged`, `viewportChanged` (`markdown-decoration.ts:2184`), `selectionSet` that intersects checklist/transparent-markdown reveal, table-row edits (`updateTouchesTableRows`), and every deferred refresh after typing settles.
- **Two separate `ViewPlugin`s still scan the same viewport independently per dispatch.** The markdown plugin calls `markdownAnalyzeLines` (`markdown-decoration.ts:1239`) over the visible spans; the calc plugin runs `buildCalcDecorationsForSpans` (`calc-decoration.ts:518`) over an overlapping span set and re-derives line structure that the markdown analysis already computed. Each `safeBuild` is an independent O(viewport) pass; they are not coordinated to share one scan.
- `calc-incremental.ts::planIncrementalCalc` already computes the changed line range for calc evaluation; the same delta is not reused for decoration rebuild.

**Approach**

1. Reuse the changed-line range computed by `calc-incremental.ts` (or compute the equivalent from `update.changes.iterChangedRanges`) so the deferred markdown rebuild can target only changed lines + their fence/table context, then splice the result into the previous `RangeSet` via `existing.map(update.changes)` + a small `RangeSetBuilder` for the changed slice.
2. Eliminate the second `RangeSetBuilder` pass for table widgets in markdown — fold table widget placement into the same single-pass builder that processes inline tokens.
3. Coordinate markdown and calc decoration update cycles so they share one viewport scan (and one `markdownAnalyzeLines` result) per dispatch instead of running two independent O(viewport) iterations on every `safeBuild`.

**Constraint**
CodeMirror requires decorations to be added in document order within a `RangeSetBuilder`. Track changed ranges by start position and process in order. Keep the current debounced full safeBuild as a fallback for viewport-change-only events where no line content changed.

**Expected gain**
Keystroke decoration latency on large documents (>5k lines) drops from O(viewport size) to O(changed lines), which for typical single-line edits is O(1) in practice. Sharing the analysis pass between the two plugins roughly halves the per-dispatch wasm-boundary + scan cost (see item 4). Closes the gap that the immediate-`map` fast path opened.

---

## 3. `TerminalApp` state decomposition (carry-over from `plan.md`)

**Problem**
`src-tauri/src/terminal/app/mod.rs:394` declares `TerminalApp` with **121 flat fields** covering: editor model, switcher, collection switcher, content search, wiki-link autocomplete, variable autocomplete, vim register/macros, calc cache + scheduling, search, undo history, folding, clipboard watch, render palette, date picker, command bar, perf trace state. Any per-subsystem change touches the same monolith, blocking subsystem-scoped diffs and making lifetime/borrow constraints conflate unrelated state.

This was already listed in `roadmap/plan.md`'s Next Sprint Checklist as `TerminalApp state decomposition (EditorModel, CalcRuntime, OverlayState, RenderState, FoldRuntime)`. It remains unstarted.

**Approach**
Decompose by subsystem in small slices. Suggested first slice: pull the cache + scheduling state (`wiki_link_render_cache`, `wiki_link_render_cache_order`, `wiki_link_line_render_cache`, `wiki_link_line_render_cache_order`, `table_formula_segment_cache`, `table_formula_segment_cache_order`, `table_display_col_width_cache`, `joined_text_cache`, `switcher_score_scratch`) into a `RenderCaches` substruct. That's the lowest-risk slice because the caches have only render-time read sites and don't cross subsystem boundaries.

**Risk**
Borrow checker friction — many call sites hold `&mut self` across multiple substructs. Plan each slice to keep field access patterns ergonomic; back out if a slice forces awkward `&mut self.x` + `&mut self.y` partial-borrow gymnastics that the current flat struct hides.

---

## 4. WASM boundary double-marshalling on the hot decoration path (new)

**Problem**
Every `editor-core` WASM export returns its result through `serde_wasm_bindgen::to_value` (`crates/editor-core/src/wasm.rs:115`, used by all 51 exports), and the TypeScript side then **re-parses and re-validates every field** of that value into a typed object. On the hottest path — `markdownAnalyzeLines` — this happens for the whole viewport on every `safeBuild`:

- Rust side: `wasm_markdown_analyze_lines` serializes a nested `Vec<AnalyzedLine>` (each line = `info` struct + `inlineTokens[]` + `codeTokens[]`) via serde reflection into JS objects.
- JS side (`src/editor/wasm.ts:1517-1526`): every returned line is walked again — `asMarkdownLineInfo(rawLine.info)`, `asMarkdownInlineTokens(...)`, `asMarkdownCodeTokens(...)` — allocating a second typed object per line and per token.

So each analyzed line is built three times: native struct → serde JS object → re-validated JS object. For a viewport scan this is the dominant per-dispatch boundary cost, and item 2 currently runs it twice (markdown + calc plugins) per structural change.

The offset-conversion paths were already optimized (`batchUtf16ToUtf8` / `batchUtf8ToUtf16` do a single walk instead of N slices), which shows the boundary cost is on the radar — but the analyze/token *payload* marshalling was not addressed.

**Approach**
1. Land item 2's shared-scan first so the analyze payload is produced once per dispatch, not twice.
2. For the analyze hot path, replace the serde object graph with a flat, index-addressable encoding (typed arrays / a single packed `Uint32Array` of `[lineFlags, headingLevel, tokenStart, tokenEnd, tokenType, ...]` plus a string side-table) that the JS side can consume without per-field re-validation. This removes both the serde reflection cost in Rust and the `as*` re-allocation in JS.
3. If a full flat-buffer rewrite is too large, the cheaper intermediate win is to drop the JS-side re-validation for the trusted analyze payload (the wire shape is produced by our own Rust, so the defensive `as*` coercion is redundant on the hot path) and keep it only for untrusted/optional fields.

**Risk**
The `as*` coercions guard against `undefined`/shape drift; removing them trades safety for speed, so gate the flat encoding behind the existing decoration profiler samples (`markdown.decorations.safeBuild` already records `analyzeLinesMs`) and confirm no correctness regression on the parity/decoration tests. Keep the serde path for the cold/low-frequency exports — this only pays off where the call is per-viewport-per-dispatch.

**Expected gain**
Removes one of three allocations per analyzed line and the serde reflection pass; combined with item 2's single-scan coordination, the per-keystroke (deferred) decoration boundary cost on large viewports drops substantially. Also shrinks the JS validation code on the hot path.

---

## 5. Undo span fast-path gated behind the 30k-line threshold (new)

**Problem**
`record_history_after_edit` (`src-tauri/src/terminal/app/editing.rs:300-345`) only takes the O(changed-lines) `record_edit_span` path when `prefer_span_history_fast_path()` is true, which is `self.lines.len() >= LARGE_NOTE_LIGHTWEIGHT_FOLD_LINES` = **30_001 lines** (`mod.rs:77`). Below that threshold — i.e. for the overwhelming majority of real notes — it falls through to `record_edit`, which calls `build_history_entry` (`terminal/history.rs:326`). That function recomputes the changed region from scratch via `shared_prefix_lines_len` + `shared_suffix_lines_len`, an O(distance-from-document-ends) line-by-line `String` comparison on **every keystroke**.

The span information (`start_line`, `old_line_span`, `new_line_span`) is already available at the call site and is simply discarded under 30k lines. For a 25k-line note edited in the middle, that is ~12k line comparisons per keystroke to rediscover a range the caller already knew.

**Approach**
Use the span fast path whenever `history_span` is `Some`, regardless of line count — the caller already has the exact changed range, so `record_edit_span` is strictly cheaper and equally correct. Keep `record_edit` (full diff) only for the `history_span == None` callers where no span is known.

**Risk / caveat**
`record_edit` and `record_edit_span` differ in coalescing behavior: `record_edit` runs the `coalesce_anchor` merge logic (`history.rs:113-137`) while `record_edit_span` does not. Making the span path the default changes undo-granularity for sub-30k notes (rapid typing may produce more, finer undo entries). Decide whether to (a) port the coalesce-anchor merge into `record_edit_span`, or (b) accept the granularity change. Validate against the undo/redo tests before flipping the gate. Secondary note: `record_edit` clones the full snapshot for `coalesce_anchor` when `len <= COALESCE_ANCHOR_MAX_LINES` (5_000) on each non-coalesced edit — bounded, but it disappears for free if the span path becomes default.

---

## 6. TUI document model is a flat `Vec<String>` (new — architectural observation, monitor)

**Observation**
The TUI's canonical document model is `lines: Vec<String>` (`mod.rs:396`) with a derived `joined_text_cache`. Mid-document structural edits (Enter / join / delete-line) are `Vec::insert` / `Vec::remove` = O(n) pointer memmove of the line vector, and several subsystems are built around the same line-vector assumption (undo prefix/suffix diff in item 5, calc `line_metadata` splice, fence checkpoints, folding maps). The web frontend uses CodeMirror's rope, so the two front ends have fundamentally different document representations — acceptable, but it means "large notes stay fast" is enforced by **tiered feature reduction** (`LARGE_DOC_CALC_DEFER_LINES = 20_000`, `LARGE_NOTE_FULL_FEATURE_LINE_LIMIT = 30_000`, reduced-undo / lightweight-fold at 30_001) rather than by a sublinear data structure.

This is most likely a deliberate, accepted tradeoff: the line-vector keeps every per-line subsystem simple, and the tier thresholds cap the O(n) costs. No action is proposed now — but it is the structural reason items 5 and 1's calc-splice logic exist, and it is the ceiling the `30k/100k/200k/400k` regression gates in `plan.md` are defending.

**If a tier gate regresses**, the options in priority order are: (a) widen the span/incremental fast paths so they cover all sizes (items 1, 5) before touching the data model; (b) only if line-vector memmove itself shows up in profiles, consider a gap-buffer-of-lines or rope-of-lines for the TUI model. Option (b) is a large refactor touching every subsystem listed above and should not be undertaken speculatively.

---

## Notes

- Items 1, 2, 3 are carried over from prior reviews and remain accurate; line numbers and file sizes updated to current files.
- Items 4, 5, 6 are new this pass. 4 and 5 are concrete, bounded-risk wins; 6 is a watch item, not a task.
- `TerminalApp` is now 121 flat fields (item 3); the count is up from the prior review, reinforcing the decomposition case.
- Large-note tier policy (`adaptive large-note mode`, regression gates at `30k/100k/200k/400k`) lives in `roadmap/plan.md` Next Sprint Checklist and is not duplicated here; item 6 records the structural reason those gates exist.
- All execution backlog for measurement/operations references stays in `roadmap/performance.md`, `roadmap/perf-tracing.md`, `roadmap/perf-multirow-table.md` per the ownership note in `plan.md`.
