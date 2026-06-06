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
- **(was item 4) WASM analyze hot path moved off `serde_wasm_bindgen` to a flat encoding.** `wasm_markdown_analyze_lines` previously reflected every line's info struct + every inline/code token into a JS object across the FFI boundary (one property write per field, interned string keys), then the TS `markdownAnalyzeLines` rebuilt a second object per line. It now packs all numerics into a single `Int32Array` (one boundary copy) plus a deduped fence-lang string table, and the TS decoder (`decodeMarkdownAnalyzeFlat`) rebuilds the same `MarkdownAnalyzeResult` object shape in one pass — so both decoration plugins (which call this per viewport scan) benefit and the consumer (`buildMarkdownDecorationsForSpans`) is unchanged. Stable token-type ids live in `markdown_tokens.rs` (`InlineTokenType::id` / `CodeTokenType::id`) mirrored by `INLINE_TOKEN_TYPE_BY_ID` / `CODE_TOKEN_TYPE_BY_ID` in `wasm.ts`, locked by tests on both sides (`token_type_ids_are_contiguous_and_match_as_str_order`, `markdown-analyze.test.ts`). Residual: the per-line cold exports (`wasm_markdown_classify_line`, `wasm_markdown_find_inline_tokens`, `wasm_markdown_tokenize_code_line`) still use serde; left as-is since they are not per-viewport-per-dispatch.
- **(was item 5) Undo span fast-path gate lowered from 30k to the coalesce cap.** `prefer_span_history_fast_path` (`terminal/app/editing.rs`) gated the O(changed-lines) `record_edit_span` path behind `>= LARGE_NOTE_LIGHTWEIGHT_FOLD_LINES` (30_001). Below that, every keystroke ran `record_edit` → `build_history_entry`, an O(distance-from-doc-ends) full-string line diff. But `record_edit` only coalesces while the doc fits within `COALESCE_ANCHOR_MAX_LINES` (5_000) — above that its coalesce anchor is `None` and it already emits one undo entry per edit. The gate now fires at `> COALESCE_ANCHOR_MAX_LINES`, so 5k–30k-line notes take the span path: identical undo granularity, O(changed lines) instead of O(doc) per keystroke. Removed the now-unused `LARGE_NOTE_LIGHTWEIGHT_FOLD_LINES` const; added a boundary regression test (`history_record_edit_span_roundtrips_just_above_coalesce_cap`). ≤5k notes keep `record_edit` (coalescing intact, diff bounded to ≤5k lines).

This pass added new findings, shipped the undo span-gate fix and the WASM analyze flat encoding (both above), and refreshed line numbers on items 1–3.

---

## 1. TUI deduplication vertical slice (carry-over)

**Problem**
`src-tauri/src/terminal/app/editing.rs` (4227 lines), `vim_actions.rs` (1441 lines), and `command_search_switcher.rs` (2834 lines) still re-implement semantics that already live in `editor-core`. Confirmed duplications as of this pass:

- ~~word + pipe text objects~~ **(done this pass — corrected understanding)** — `apply_vim_actions` already routes every `supports_intent` intent (which includes all word/pipe object intents) through the shared core `execute_vim_action_with_target` first, then `continue`s; the TUI-local `find_word_object_bounds`, `find_pipe_object_bounds`, `apply_word_text_object`, `apply_pipe_text_object` lived only in the fallback match reached when core returns `None`. So this was **dead/vestigial duplicate logic, not a live divergence** (an earlier note here mis-called it a parity bug; in production both front ends already use core, which trims inner whitespace on `ci|`). Removed the 4 fns and collapsed the 8 fallback arms into one documented no-op arm (kept for match exhaustiveness; reachable only on the core-`None` edge case, where a no-op matches the UI). Full workspace + parity green.
- ~~remaining vestigial fallback arms~~ **(done this pass)** — verified in `execute_vim_action_with_target` that every `supports_intent` intent returns `Some(...)` **except `PasteAfter`** (which returns `Option` and can fall through with an empty register — and its TUI arm additionally pulls from the system clipboard, so it is genuinely live and was kept). All other `supports_intent` fallback arms are therefore unreachable. Removed the dead impls for `DeleteLine`, `YankLine`, `Delete/YankToLineStart/End`, `DeleteChar`, `DeleteWordForward/Backward`, `YankWordForward/Backward`, and consolidated all 37 dead `supports_intent` intents into one exhaustive no-op arm (no wildcard, so a new intent still forces a compile error). Deleted the 5 now-orphaned helpers (`slice_current_line_cols`, `delete_current_line_cols`, `line_col_lt`, `slice_cols_range`, `delete_cols_range`) and their imports. Net **−316 lines** in `vim_actions.rs`; full workspace + parity green.
- ~~calc metadata helpers~~ **(done this pass — corrected understanding)** — the pure primitives (`detect_calc_signal_flags_with_mask`, `line_metadata_with_mask`, `line_metadata_for_lines_with_mask`) already lived in core and the TUI already called them; the thin wrappers (`rescan_calc_flags`, `rebuild_calc_line_metadata`, `refresh_calc_line_metadata_at`) just store core results into `self.calc`. The only genuinely TUI-resident *semantics* were the two incremental algorithms — `update_calc_flags_incremental` (false→true flag merge over the edited line(s)) and `splice_calc_line_metadata` (recompute metadata only for replaced lines). Note this was **not cross-frontend dedup** (the web `calc-incremental.ts` solves a different problem and has no counterpart; single consumer today) — but per the "keep front ends thin / calculation behavior lives in core" rule, the incremental algorithms were extracted to `calc_plan::merge_incremental_signal_flags` and `calc_plan::splice_line_metadata` (pure, unit-tested), with the TUI keeping `self.calc` state ownership + the empty/rebuild-fallback orchestration. Behavior unchanged (full workspace + calc_table incremental tests green).

Every duplicated primitive is a latent divergence bug: a fix to core semantics has to be manually replicated in the TUI, and the TUI's version will quietly drift.

**Status:** the `vim_actions.rs` half of item 1 is now done — word/pipe text objects, the calc-metadata incremental algorithms, and the full vestigial-fallback set in `apply_vim_actions` are all resolved (net ~−380 lines across the file this pass). The remaining `editing.rs` / `command_search_switcher.rs` surfaces (4227 / 2834 lines) have not been audited for further duplication; treat that as a fresh pass, not a continuation.

**Caveat learned this pass:** two of the three text-object/calc bullets turned out to be *not* live cross-frontend divergences — the shared semantics already lived in core. Before treating an item-1 entry as a dedup, confirm there is actually a second consumer (or that the TUI logic is genuinely unreachable); otherwise the work is either dead-code removal or an architecture-cleanliness move (front-end-thin), not parity dedup.

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
Keystroke decoration latency on large documents (>5k lines) drops from O(viewport size) to O(changed lines), which for typical single-line edits is O(1) in practice. The analyze flat encoding shipped this pass (see "Recently shipped") already cut the per-line boundary cost; sharing one analysis pass between the two plugins removes the second scan entirely. Note the rebuild is viewport-bounded today (deferred margins of 8 lines markdown / 80 lines calc), not document-bounded, so the absolute win is capped by screen height — useful, but smaller than a doc-scaled gain. Closes the gap that the immediate-`map` fast path opened.

---

## 3. `TerminalApp` state decomposition (carry-over from `plan.md`)

**Problem**
`TerminalApp` (`src-tauri/src/terminal/app/mod.rs`) was a ~121-field flat monolith covering: editor model, switcher, collection switcher, content search, wiki-link autocomplete, variable autocomplete, vim register/macros, calc cache + scheduling, search, undo history, folding, clipboard watch, render palette, date picker, command bar, perf trace state. Any per-subsystem change touches the same monolith, blocking subsystem-scoped diffs and making lifetime/borrow constraints conflate unrelated state.

This was already listed in `roadmap/plan.md`'s Next Sprint Checklist as `TerminalApp state decomposition (EditorModel, CalcRuntime, OverlayState, RenderState, FoldRuntime)`.

**Progress (this pass): first slice done — `RenderCaches`.** Pulled the seven render-time cache fields (`wiki_link_render_cache` + `_order`, `wiki_link_line_render_cache` + `_order`, `table_formula_segment_cache` + `_order`, `table_display_col_width_cache`) into a `#[derive(Default)] struct RenderCaches`, accessed as `self.render_caches.*` (33 call sites across `rendering.rs`/`command_search_switcher.rs`/`editing.rs`). No borrow friction — these are render-path-only with disjoint field access. `joined_text_cache` and `switcher_score_scratch` were **deliberately excluded** from this slice: the former is read/written on the edit/save hot paths alongside `self.lines` (real partial-borrow risk), the latter belongs to the switcher subsystem, not rendering.

**Remaining approach**
Continue by subsystem in small slices (the `plan.md` targets: `EditorModel`, `CalcRuntime`, `OverlayState`, `FoldRuntime`). For each, check that call sites don't hold `&mut self.<substruct>` across a `&self` method call; back out a slice if it forces awkward partial-borrow gymnastics the flat struct hides. `joined_text_cache` is a good canary for that risk — defer it until an `EditorModel` slice can own `lines` + `joined_text_cache` together.

---

## 4. TUI document model is a flat `Vec<String>` (new — architectural observation, monitor)

**Observation**
The TUI's canonical document model is `lines: Vec<String>` (`mod.rs:396`) with a derived `joined_text_cache`. Mid-document structural edits (Enter / join / delete-line) are `Vec::insert` / `Vec::remove` = O(n) pointer memmove of the line vector, and several subsystems are built around the same line-vector assumption (undo prefix/suffix diff, calc `line_metadata` splice, fence checkpoints, folding maps). The web frontend uses CodeMirror's rope, so the two front ends have fundamentally different document representations — acceptable, but it means "large notes stay fast" is enforced by **tiered feature reduction** (`LARGE_DOC_CALC_DEFER_LINES = 20_000`, `LARGE_NOTE_FULL_FEATURE_LINE_LIMIT = 30_000`, reduced-undo at 30_001) rather than by a sublinear data structure.

This is most likely a deliberate, accepted tradeoff: the line-vector keeps every per-line subsystem simple, and the tier thresholds cap the O(n) costs. No action is proposed now — but it is the structural reason the undo span fast path and item 1's calc-splice logic exist, and it is the ceiling the `30k/100k/200k/400k` regression gates in `plan.md` are defending.

**If a tier gate regresses**, the options in priority order are: (a) widen the span/incremental fast paths so they cover all sizes (item 1, and the now-shipped undo span path) before touching the data model; (b) only if line-vector memmove itself shows up in profiles, consider a gap-buffer-of-lines or rope-of-lines for the TUI model. Option (b) is a large refactor touching every subsystem listed above and should not be undertaken speculatively.

---

## Notes

- Items 1, 2, 3 are carried over from prior reviews and remain accurate; line numbers and file sizes updated to current files.
- Item 4 (flat `Vec<String>` doc model) is a watch item, not a task. Two findings shipped this pass — the undo span gate and the WASM analyze flat encoding — see "Recently shipped".
- `TerminalApp` is now 121 flat fields (item 3); the count is up from the prior review, reinforcing the decomposition case.
- Remaining open work: item 1 (TUI dedup), item 2 (decoration delta rebuild / shared scan), item 3 (`TerminalApp` decomposition). Item 2's remaining value is the shared single scan across the two plugins; the per-line marshal cost it depended on is already addressed.
- Large-note tier policy (`adaptive large-note mode`, regression gates at `30k/100k/200k/400k`) lives in `roadmap/plan.md` Next Sprint Checklist and is not duplicated here; item 4 records the structural reason those gates exist.
- All execution backlog for measurement/operations references stays in `roadmap/performance.md`, `roadmap/perf-tracing.md`, `roadmap/perf-multirow-table.md` per the ownership note in `plan.md`.
