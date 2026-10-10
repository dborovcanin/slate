# Architecture Refactor: Core-Owned Editing

Status: phases 1–3, 5 and 6 implemented, phase 4 partial (2026-10-10); see
"Follow-ups". Execution reference for moving editing semantics
out of `crates/tui` into the core crates. Ownership rules come from
`AGENTS.md` and `roadmap/plan.md` ("Ownership Rules"); the existing contract
is `roadmap/editor-engine-contract.md`.

## Goal

Make `crates/tui` what the architecture already says it is: an input adapter,
renderer and host for side effects. Behavior that changes text, cursor,
selection, undo, folds or calc state moves to `editor-core` (or `app-core`
where evaluation is involved) as deterministic functions with core tests.

Why now, independent of any future front end:

- The logic can only be tested through `TerminalApp` today; core functions can
  be tested on plain text and state.
- Hot paths (edit bookkeeping, calc remap, fold remap) become measurable in
  isolation.
- It continues work already done: table semantics (`editor_core::table`) and
  command execution (`editor_core::commands`, 2026-10-03).

## Non-goals

- No GUI and no generic "session" or controller layer up front. Shared
  orchestration is explicitly deferred: autosave, worker-result validation,
  cancellation and session lifecycle still live in `TerminalApp`. These
  phases improve editing ownership; they do not yet make those workflows
  reusable by another front end. A future extraction should share their
  correctness policy rather than copy it into each front end.
- The six phase commits preserve behavior. Separately committed, reproduced
  bugs from the follow-up audit are listed below. Replay fixtures and performance
  limits remain unchanged, with added regressions and coverage.
- No new dependencies.

## Baseline inventory before the refactor

`crates/tui/src` is about 36.4k lines (tests excluded). About 11.9k import
ratatui or crossterm; the other 24.5k do not. Import counts are descriptive,
not an ownership test: input routing, viewport geometry and dialog state
remain presentation concerns even without either import. Large files to
inspect for mixed ownership:

| File | Lines | Content |
| --- | ---: | --- |
| `terminal/app/editing.rs` | 5134 | 170 functions: text mutation, undo bookkeeping, word motions, paste, folding upkeep, calc scheduling and remap, autocomplete, wiki links, viewport calc |
| `terminal/app/command_search_switcher.rs` | 3502 | switcher, collections, content/web search, command bar, save/autosave, outside changes |
| `terminal/app/mod.rs` | 2046 | `TerminalApp` state, startup, main loop |
| `terminal/app/input_modes.rs` | 1020 | key dispatch per mode, date picker, image preview |
| `terminal/app/vim_actions.rs` | 996 | `apply_vim_actions` (540 lines) executing `VimIntent`s, registers, macros |
| `terminal/history.rs` | 704 | `LineHistory` undo/redo store; no imports, already pure |
| `terminal/markdown_view.rs` | 665 | hidden-marker ranges and reveal rules; depends only on `editor_core::markdown_tokens` |
| `terminal/app/table_helpers.rs` | 560 | cell lookup, formula masking, display row reformatting |
| `terminal/app/calc_helpers.rs` | 467 | calc glue, variable and cross-note completion |
| `terminal/app/reminder_helpers.rs` | 365 | reminder line mapping and persistence |

Recurring pattern in `editing.rs`: a primitive mutates `editor.lines`
directly (`insert_char`, `insert_newline`, `insert_paste`, `backspace`,
`delete_forward`), then calls bookkeeping (`splice_calc_line_metadata`,
`mark_edited_from_line_with_span`) with a `(start_line, old_span, new_span)`
tuple. That tuple is a useful invalidation summary, but not the complete edit
contract: exact pre-edit coordinates and transaction boundaries must survive
the extraction too.

## Inventory

Verdicts: **core** moves to a core crate, **host** stays in `crates/tui`,
**split** separates a pure decision (core) from execution (host).

| Area | Current location | Verdict | Target |
| --- | --- | --- | --- |
| Offset and change application | `editing.rs`: `map_offset_through_changes`, `document_text_len`, `line_and_byte_for_offset`, `apply_text_change_in_place`, the change part of `apply_edit_operation` | core | `editor_core::buffer` (new) |
| Text primitives | `insert_char`, `insert_text`, `insert_newline`, `insert_paste`, `backspace`, `delete_forward`, `delete_word_backward` | split | line mutation + cursor + exact edit description and `EditDelta` summary in `editor_core::buffer`; bookkeeping execution stays host |
| Paste semantics | `try_insert_table_cell_multiline_paste`, `pasted_table`, `try_paste_as_table` | core | `editor_core::table` / `table_import` |
| Undo store | `terminal/history.rs` (`LineHistory`, `LineDelta`) | core | `editor_core::history` |
| Undo coalescing and action stack | `record_text_history`, `push_undo_action`, `UndoAction` (text vs reminder) | split | deterministic grouping, redo truncation and action ordering in core; elapsed time, transaction boundaries and reminder payloads are host inputs |
| Word motions | `move_cursor_left_word`, `move_cursor_right_word`, `move_cursor_word_in_table` | core | `editor_core::motions` (new) or `vim_actions` |
| Vim intent execution | `vim_actions.rs`: `apply_vim_actions`, `apply_visual_selection_action` | split | `VimIntent -> EditOperation + register/selection delta` in `editor_core::vim_actions` (plan.md Commands/Vim action point 1); macros, clipboard and status stay host |
| Post-edit calc planning | `mark_edited_from_line_with_span`, `can_skip_calc_recompute`, `should_defer_calc_recompute`, `try_remap_calc_results_after_structural_edit`, `run_calc_recompute` (454 lines) | split | pure `plan_after_edit` in `editor_core::calc_plan` returning an action; host executes, schedules and evaluates through `app-core` |
| Fold upkeep | `recompute_folding_if_needed` (235 lines), `try_incremental_fold_remap`, `fence_state_before_line` | split | remap/rebuild decisions in `editor_core::folding`; collapse state stays host (per contract) |
| Fold view map | `rebuild_fold_view_map`, `real_line_for_virtual`, `current_virtual_line` | later | motions depend on it; revisit after vim execution moves |
| Completion semantics | `calc_helpers.rs`: `extract_variable_completion_prefix`, `variable_completion_candidates`, `build_variable_suggestions`, `extract_cross_note_completion_prefix`; `editing.rs`: `variable_autocomplete_state`, `parse_wiki_link_query`, `filtered_wiki_link_suggestions` | split | prefix/candidate logic core; popups host |
| In-note search matching | `text_utils.rs`: `case_insensitive_matches`; `recompute_search` | core | `editor_core::search` (new) |
| Markdown display rules | `terminal/markdown_view.rs` | later | pure already; move when another consumer or tests need it |
| Table display reformatting | `table_helpers.rs`: `reformat_table_row_impl` and friends | later | presentation of `TableBlockLayout`; evaluate after phase 4 |
| Reminder line mapping | `reminder_helpers.rs`: `line_change`, `fates`, `move_lines` | later | builds on `app_core::reminders::block_line_fates`; preserve exact pre-edit coordinates and block mappings, not just `EditDelta` |
| Switcher, collections, content/web search, command bar, dialogs, pickers, help, browser | `command_search_switcher.rs`, `input_modes.rs`, `browser.rs` | host | UI state |
| Save, autosave, outside changes, workers (scripts, currency, prewarm, viewport calc preparation) | various | host | side effects and threads |
| Calc scheduling (debounce, idle ticks, `key_depth`) | `editing.rs` | host | timing is host policy |
| Clipboard, image import/preview, export, backup, IMAP | various | host | I/O |

`editor-core` does not depend on `app-core`, so calc evaluation stays a host
call; only the decision of what to evaluate moves. No new crate is needed for
any phase below.

## Seam: exact edits and `EditDelta`

Every core edit primitive reports the lines it touched, replacing today's
`(start_line, old_span, new_span)` tuples with an invalidation summary:

```rust
pub struct EditDelta {
    pub start_line: usize,
    pub old_span: usize,
    pub new_span: usize,
}
```

`EditDelta` is not the sole edit notification. The buffer contract must also
preserve the information consumers cannot recover after mutation:

- Exact text edits carry pre-edit start/end line and byte columns, old
  boundary-line lengths and the number of inserted line breaks. Preserve
  the information currently passed to `note_line_edit` / `LineEdit` without
  making `editor-core` depend on `app-core`.
- Structured block replacements retain the old-line-to-new-line mapping
  needed by line-attached marks. Summary spans cannot distinguish a moved
  line from a deleted one.
- Compound `EditOperation`s report each applied change in application order,
  with its coordinate space explicit. Do not replace them with only one
  bounding span. Today `apply_edit_operation` sorts changes by descending
  `from` in pre-operation coordinates and applies them back to front; the
  `changes.clone()` and sort happen only on this multi-change path.
- Transaction boundaries distinguish one user action from its constituent
  mutations, including typing followed by autoformat. Consumers may update
  metadata per change while history and calc retain their existing grouping.

Core functions operate in place on `&mut Vec<String>` plus cursor, so no
whole-document copies are added on the typing path. Capture required metadata
before changing the buffer. Use the existing single-edit fast path; a
compound-edit description must not force a new allocation for every key.
`TerminalApp` keeps one `after_edit` path (today
`mark_edited_from_line_with_span`) that routes exact edits and transaction
information to history/reminders, and span summaries to calc metadata,
folds and render caches. Preserve existing call ordering during extraction.

## Phases

Each PR extracts one responsibility and is behavior neutral, with tests moved
or added in the core crate. Aim for fewer than 300 substantive changed lines,
excluding tests and mechanical relocation. An unchanged module move may
exceed that size only with explicit approval; keep it separate from contract
adaptation or other logic changes so it remains reviewable.

### Phase 1: buffer primitives

1. Create `editor_core::buffer` with exact edit reporting, `EditDelta`, offset
   helpers and `apply_text_change_in_place`; `apply_edit_operation` calls it.
2. Move line mutation for `insert_char`/`insert_text`/`insert_newline`/
   `backspace`/`delete_forward` behind core functions returning cursor and
   exact edit information plus `EditDelta`.
3. Move `insert_paste` and the table paste decisions.

Done when terminal wrappers for extracted primitives no longer splice
`editor.lines` themselves, every mutation preserves exact edit reporting and
transaction grouping, and core tests plus
terminal integration tests cover splits at column zero versus mid-line,
joins, block replacements and multiple replacements in one operation.

### Phase 2: undo store and policy

1. Move `terminal/history.rs` to `editor_core::history` unchanged, with its
   tests; `COALESCE_ANCHOR_MAX_LINES` moves with it.
2. Make `record_edit_span` take an `EditDelta`.
3. Move deterministic undo grouping and action ordering into core. The host
   supplies elapsed time, edit/insert-session boundaries and opaque reminder
   payloads; it executes persistence side effects. Core decides whether to
   coalesce, truncate redo or remove a merged step that undid itself, keeping
   text and reminder actions in order.

Done when the host no longer independently decides undo grouping or maintains
the semantic action order. Cover Vim insert sessions across pauses, typing
coalescing boundaries, redo truncation, self-cancelling merged edits and
text/reminder interleaving. Moving only `LineHistory` is an intermediate step,
not completion of undo extraction.

### Phase 3: word motions

1. Move `move_cursor_left_word`, `move_cursor_right_word` and
   `delete_word_backward` to core functions over a line and column.
2. Fold-aware stepping across lines takes the visible-line neighbors as
   input instead of reading `TerminalApp`.

Done when the terminal word motions only wrap core functions, and core tests
cover punctuation and whitespace classes, line edges, folded neighbors and
table cells.

### Phase 4: vim intent execution (partial)

Split `apply_vim_actions` by intent group (motions, operators, text objects,
visual selection, paste/registers). Core returns `EditOperation` plus
register and selection deltas; the host applies them and keeps macros,
system clipboard and status messages. Extend
`crates/tui/src/terminal/tests/golden/vim_replay.json` before each group
moves.

Done when each intent group executes through core, with its replay fixtures
added before the move, and the host keeps only macros, system clipboard and
status messages.

Phase 4 keeps line-buffer visual plans separate from byte-offset operator plans:
selected text, replacement spans, register modes and resulting cursors are core
owned without joining large notes. Horizontal prose movement, linewise paste
line preparation and fallback register preparation also execute in core, and
the host uses the core register type directly. Not done: linewise and charwise
paste and open-line insertion still mutate the buffer in the host, and
`insert_entry_column` / `paste_after_column` only compute cursor columns for
intents the host still dispatches. Folded-line
lookup, screen movement and markdown display-boundary exits remain presentation
adapters; clipboard/image import, macro replay, reminder attachment, undo I/O,
and status are host effects. The existing operator/text-object plans stay scoped.
Unicode visual deletion, reverse multiline selection and counted EOF paste
fixtures were added and passed before the move.

### Phase 5: post-edit planning (complete)

1. Add `editor_core::calc_plan::plan_after_edit(delta, flags) -> CalcAfterEdit`
   covering skip, defer, remap-only, recompute-range, schedule-idle and
   viewport-refresh; `mark_edited_from_line_with_span` executes the result.
2. Split `recompute_folding_if_needed` the same way into an
   `editor_core::folding` decision and host application.

Done when `mark_edited_from_line_with_span` and the fold upkeep only carry out
core decisions, and the PR records `perf-check` large-note p50/p95 before and
after.

Phase 5's calc dispatch uses cached flags and line counts; structural remap
eligibility reads borrowed metadata and affected lines. Fold upkeep moved with
its existing incremental cache updates and deferred-rescan rules. Scheduling,
worker results, persistence and viewport application stay in the terminal.

### Phase 6: completion and search (complete)

Move completion prefix and candidate functions, wiki-link query parsing and
filtering, and in-note search matching to core. Popups, selection state and
DB lookups stay in the host.

Done when completion and search logic is tested in core and the terminal keeps
only popup state and DB lookups.

### Later, only when needed

Fold view map, markdown display rules, table display reformatting and
reminder line mapping. They are presentation-derived or already pure; move
them when a second consumer or a test needs them, not before.

## Rules for every step

- Move code before changing it; a move PR contains no behavior changes.
- Keep in-place mutation; no extra clones or allocations on key paths.
- `cargo test --workspace`, `cargo clippy --workspace --all-targets`,
  `cargo fmt --all --check`.
- Golden replays (`crates/editor-core/tests/golden_replay.rs`,
  `crates/tui/src/terminal/app/tests/replay.rs`) pass unchanged.
- Add focused core and terminal integration coverage for exact edit mapping
  and transaction grouping before replacing the current notification paths.
- Run `perf-check` against `perf/baselines/large_note.json` for any step that
  touches editing, calc or folding; p50/p95 must not regress.
- Update `roadmap/editor-engine-contract.md` when a new contract (buffer,
  history, post-edit plan) becomes canonical.

## Risks

- **Performance.** The post-edit path carries measured tuning (span splices,
  viewport-only calc, deferred fold rescans). Phase 5 is a split, not a
  rewrite, and needs before/after perf runs.
- **Hidden coupling.** Edits also invalidate the joined-text cache, table
  formula segment cache, title refresh and reminder positions. All of it must
  keep flowing through the single `after_edit` path.
- **Churn against feature work.** Run phases between features; each phase is
  independently shippable and can pause.

## Progress

| Phase | Status | Notes |
| --- | --- | --- |
| 1. Buffer primitives | Complete | Offset helpers, text changes, typing and plain/table paste delegate to core |
| 2. Undo store and policy | Complete | Core owns span recording, grouping, redo truncation, self-cancelling text markers and text/reminder action order; host supplies time/session boundaries and applies effects |
| 3. Word motions | Complete | Core owns word motions and backward deletion; host supplies lazy visible neighbors and retains edit bookkeeping |
| 4. Vim intent execution | Partial | Visual selection, operator/text-object plans, registers and cursor placement are core-owned; paste and open-line buffer edits remain in the host |
| 5. Post-edit planning | Complete | `plan_after_edit`, `plan_result_remap` and fold upkeep decisions are core-owned; fold upkeep still locates edits from the cursor (see "Follow-ups") |
| 6. Completion and search | Complete | Prefixes, candidates, wiki-link queries and search matching are core-owned; applying a picked completion still edits in the host |

Track progress by which semantic decisions have a canonical core owner,
which terminal paths delegate to it, and which core regression tests cover
the contract. Record remaining host-owned correctness policy explicitly.
File sizes and import counts may describe the codebase, but reducing them is
not an acceptance criterion.

### Host-owned correctness policy

Policy that stays in `TerminalApp` after these phases. A future shared
orchestration layer should take these over rather than copy them per front
end.

| Policy | Where | Notes |
| --- | --- | --- |
| Autosave and save revision checks | `command_search_switcher.rs`: `save_with_options`, `start_background_autosave`, `poll_background_save` | optimistic concurrency against the stored revision |
| Outside-change reload | `maybe_take_outside_change`, `reload_active_note` | |
| Script results discarded unless `note_id` and `text_generation` still match | `scripts.rs`: `poll_script_result` | edits or note switches during a run drop the result |
| Currency results applied globally | `currency.rs`: `apply_currency_result` | no note check; identical rates skipped; one fetch at a time |
| Viewport calc preparation installed only for the same note | `editing.rs`: `install_viewport_calc_preparation` | changed text caught by rehashing in the calc cache and the reset `cross_note_refs_generation`; cross-note index writes fenced by `epoch()` |
| Script and currency cancellation | `scripts.rs`, `currency.rs` | process-group termination lives in `app_core::scripts` |
| Clock for undo coalescing | `mark_edited_from_line_with_span` | elapsed time supplied to core; grouping decisions are core-owned |
| Calc scheduling (debounce, idle ticks, `key_depth`) | `editing.rs` | timing only; decisions move in phase 5 |

## Phase 5 performance comparison (2026-10-10)

Release `large_note_perf`, same machine and unchanged latency limits; values are
p50/p95 milliseconds for key handling plus repaint. Both runs passed every gate.
400k lines remain report-only. These samples show preservation, not a guaranteed
speedup across machines or runs.

| Action | Lines | Before | After |
|---|---:|---:|---:|
| Enter | 30k | 0.80 / 1.86 | 0.68 / 1.69 |
| Open line | 30k | 0.89 / 0.92 | 0.81 / 0.83 |
| Paste | 30k | 6.01 / 6.37 | 5.09 / 5.41 |
| Enter | 100k | 3.27 / 6.99 | 2.85 / 6.98 |
| Open line | 100k | 3.83 / 4.06 | 3.40 / 3.91 |
| Paste | 100k | 10.25 / 10.40 | 9.84 / 10.10 |

Phase 6 moves variable/cross-note prefix parsing, candidate selection, table
helper precedence, wiki query validation/filtering and character-range search
matching into core. Popup visibility, current search match and DB/background
export loading remain host state/effects. Existing popup integration and replay
tests continue to exercise the terminal calls; core tests cover Unicode ranges,
qualified names, suffix candidates, disabled modules and suggestion order.

## Follow-up audit

After the six extraction phases, the requested architecture/correctness pass:

- Tracks new history entries and eviction explicitly, so bounded text history
  stays in chronological order with reminder actions. Small-note and large-note
  span recording are covered through undo and redo.
- Converts fixed-padding table byte bounds to character columns before backward
  word deletion; Unicode in preceding cells no longer leaves a partial word.
- Keeps horizontal left movement within a line constant-time, without a new
  full-line character count.
- Initializes an empty buffer before newline application and reports zero
  old lines for empty-buffer primitive/paste edits.
- Corrects visual-plan deltas for unchanged yanks and the retained empty line
  after whole-document deletion. Changed visual selections use the history span
  fast path; empty no-op selections retain their previous history behavior.

The startup probe source and app-core startup/config paths are unchanged against
`main`. The unified checker exceeded its April startup baseline on this machine;
a clean `main` worktree also exceeded it. Limits were not increased to hide the
failure. Final validation results and the remaining startup qualification gate
are recorded with the completion report below.

## Completion checks (2026-10-10)

- Workspace tests: 1,294 passed, six ignored benchmarks; full doctest pass.
- `cargo fmt --all -- --check` and diff whitespace checks passed.
- `cargo clippy --workspace --all-targets --locked` passed with existing warnings;
  new completion/search re-exports were moved before the shim's test module.
- Final unified performance check: table and large-note gates passed. Startup
  remained above the existing baseline on both this branch and clean `main`.
  Five clean-main runs had median config load 0.850 ms (limit 0.255 ms) and
  app-core open 2.215 ms (limit 0.742 ms), using the same active config/database.
- Startup qualification is deferred at the user's request. Performance limits
  were kept unchanged. Review can proceed; this is not a claim that every merge
  qualification gate is green.
- Live terminal: a tmux run of a debug build (Vim mode, isolated config and
  data directories) covering typing, visual delete, `o`, Ctrl-W, yank/paste,
  undo/redo and calc results produced the same screens as `main`, with the
  prepared-plan debug assertions active.

## Follow-ups

Found in the pre-merge review. None blocks the merge; each needs its own
change with core tests.

- **Fold upkeep from `EditDelta`.** `folding::upkeep::plan_fold_upkeep` infers
  a one-line insert or delete at the cursor line. Pass the edit's `EditDelta`
  instead, so edits away from the cursor (undo and redo, a mouse paste or
  multiple cursors in another front end) map folds without a rescan. It also
  clones the current line three times per keystroke.
- **Host-only text edits.** These still mutate `editor.lines` in the terminal
  crate without a core plan, and most record history through the
  whole-document diff (`mark_edited`):
  - applying a variable autocomplete pick (`apply_variable_autocomplete_pick`)
  - applying a calc result with Tab (`apply_calc_tab`)
  - calc trailer refresh in `run_calc_recompute`
  - wiki-link selection and heading-suffix removal
  - removing an empty table continuation row
    (`prune_empty_table_continuation_row_at_cursor`)
  - Vim linewise and charwise paste, and open-line insertion (phase 4)
- **Vim paste performance.** Linewise paste inserts and clones one line at a
  time, which is O(pasted lines × note lines); use one splice.
- **Search allocation.** `search::find_matches` lowercases a copy of every line
  on each query change; reuse one buffer.
- **Document state.** The after-edit pipeline (calc plan execution, fold view
  map, cache invalidation) and the fold structure/text caches still live in
  `TerminalApp`. A core document-state type (lines, cursor, history, undo
  policy, fold caches, calc metadata) whose `apply` returns effects would let
  another front end reuse it.
- **Pre-existing bug, also on `main`.** Typing `one`/`two`, then `o` with
  `three`/`four`, `gg V d`, `u`, Ctrl-R, `u` and `x` in Vim mode joins the
  first two lines (`onetwo`) instead of deleting a character. The restored
  cursor is probably left past the line end; undo should clamp it as Normal
  mode does.
- **Display model.** `terminal/markdown_view.rs` and table display
  reformatting remain terminal code; a second front end needs them as a
  shared styled-line model.
