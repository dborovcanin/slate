# Architecture Refactor: Core-Owned Editing

Status: phases 1–3, 5 and 6 implemented, phase 4 partial; phases 7 (shared
note session) and 8 (shared display model) planned (2026-10-10). See
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

- No GUI in this plan. A second front end is a separate project; this plan
  only makes the shared parts ready for it.
- No speculative framework. Phases 1–6 deliberately deferred shared
  orchestration until editing semantics were core-owned. Phase 7 now
  extracts it from code the terminal already runs, one responsibility at a
  time, rather than designing a controller layer up front.
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
| Markdown display rules | `terminal/markdown_view.rs` | phase 8 | pure already; part of the shared display model |
| Table display reformatting | `table_helpers.rs`: `reformat_table_row_impl` and friends | phase 8 | presentation of `TableBlockLayout`; part of the shared display model |
| Reminder line mapping | `reminder_helpers.rs`: `line_change`, `fates`, `move_lines` | phase 7 | builds on `app_core::reminders::block_line_fates`; preserve exact pre-edit coordinates and block mappings, not just `EditDelta` |
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

### Phase 7: shared note session (planned)

**Why, and why now.** Phases 1–6 deferred a session layer: moving editing
semantics first kept it from wrapping `TerminalApp`'s mix of policy and
scheduling. That is done, and a second front end is now a near-term goal.
What remains in `TerminalApp` is largely correctness policy, not only
timing (see "Host-owned correctness policy"): save revision checks, stale
worker results, exact edit recording before mutation, and the order of
history, reminder, calc and fold updates after an edit. A second copy of
those rules would drift and fail as data loss or stale results, so they
need one owner that both front ends call. Neither core crate can own it:
calc upkeep needs `editor-core` planning and `app-core` evaluation, and
neither depends on the other.

**Shape.** A new crate (`crates/session`, name provisional) depending on
`editor-core` and `app-core`. It owns one open note's state and the rules
for changing it, and it never runs work itself:

- Owns: note identity, access mode and stored revision; text generation and
  dirty state; `LineHistory` and
  `UndoPolicy`; reminder marks; fold structure caches; calc results, line
  metadata, dependency index and prepared calc context.
- Takes: core edit plans and commands, results of jobs it requested, and
  the current time as an input. It holds no clock.
- The document (lines, cursor, selection anchor, text generation, joined
  text) is a `Document` value defined by the crate and passed to session
  calls as `&mut Document`. Front ends read it freely and move the cursor;
  only the crate changes its text. Keeping it a separate value, rather than
  a field behind the session, avoids rewriting several hundred
  `self.editor.*` call sites and lets a GPUI entity hold both side by side.
- Returns effects: which lines to repaint, a status message, and jobs for
  the front end to run (calc preparation and index builds, autosave with the
  revision it expects, cross-note export loads). Jobs carry the identity and
  versions relevant to their type; result acceptance follows the rules below,
  rather than one universal note/text/epoch check.
- Does not own: viewport and scroll, wrapping, fold collapse state and view
  map, popups, dialogs, key mapping, clipboard, worker threads, timers and
  rendering. The terminal keeps its event loop and threads; another front
  end runs the same jobs on its own executor.
- Beside the note session, the crate holds the application-wide rate
  service: the refresh coordination now in `terminal/app/currency.rs` (one
  fetch at a time, cancellation on exit, applying a result and invalidating
  calc state). It follows the same rules: it returns the fetch job and the
  invalidation effects, and the installed rates themselves stay in
  `app_core::currency`.

**Result acceptance.** Keep acknowledgement of completed side effects separate
from installing results into the current buffer:

- Script edits require the same note, session lifetime, text generation and
  editable access state. Switching away and back must still invalidate the
  old result.
- Calc preparation and index/export results validate the note/session and
  relevant text, dependency and currency versions before installation or
  shared-index publication. Preserve existing revalidation where a result can
  be reused safely; each job type specifies its required checks.
- A successful save acknowledges the persisted snapshot even when more typing
  occurred while it ran. Advance the stored revision only if its expected
  revision still matches, and clear dirty state only if the saved snapshot is
  still current. Preserve reminder-generation acknowledgement and keep newer
  text/reminder edits unsaved. Drain or reconcile pending saves before switch,
  reload and close; never discard an acknowledgement merely because the text
  generation changed, or roll back a newer stored revision.
- Currency refresh is application-wide and survives note switches. Apply it
  once through the shared rate service, skip identical rates, and invalidate
  affected sessions and cross-note calculation state when rates change.
  Cancellation and overlapping-refresh policy belong to that global job,
  rather than the lifetime of one note session.

**Steps.** "Phase 7 implementation plan" below lists them as one commit
each: prerequisites (undo cursor fix, fold upkeep from `EditDelta`, core plans
for host-only edits), the crate and its `Document`, session state, one edit
pipeline, calc state and jobs, fold structure, note lifecycle and saves,
script results and the rate service, then a headless-host test and a final
bug and performance round.

Done when `TerminalApp` holds a session and keeps only input mapping,
rendering, timers and job execution; each row of "Host-owned correctness
policy" is moved into the session or the rate service, or is pure scheduling;
and session tests drive edits, jobs and stale results without a terminal.
Cover typing during autosave, save completion after a newer revision,
switch-away/back script rejection, global currency refresh across note
switches, and reminder positions through joins, block replacements and
undo/redo.

### Phase 8: shared display model (planned)

Move the presentation rules both front ends need into a shared,
framework-independent model: which markdown markers are hidden or revealed
for a cursor (`terminal/markdown_view.rs`), table display reformatting, and
the spans for calc results and variable highlights. Output is styled lines
with semantic styles (heading, hidden marker, calc result, variable), not
colors or cells; each front end maps them to its own theme and geometry.
Each transformed line also carries explicit source-to-display and
display-to-source mappings, with coordinate units stated. Hidden markers,
substituted formula values, table padding and generated ghost spans retain
their source provenance. Define whether a position in generated text maps to
an owning source span or is non-editable, including the cursor affinity at
hidden boundaries. The front end maps display positions to terminal cells or
pixels for cursor placement, selection and hit testing.

Keep derived lines cached and update only affected lines or table blocks.
Cache validity includes text changes, cursor-dependent reveal state, module
state and relevant calc/variable generations; cursor movement must not cause
whole-document parsing or layout.
The model assumes a monospace editor font; table layout for proportional
fonts would be front-end geometry.

Done when the terminal renderer draws from the shared styled lines and
keeps only terminal styles, cell painting and wrapping. Shared tests cover
Unicode coordinate conversion, hidden-marker boundaries, formula substitution,
table reflow and generated-span hit policies. Terminal replays and performance
gates verify that mapping and cache extraction preserve existing behavior.

### Later, only when needed

Fold view map. Move it when a second consumer or a test needs it, not before.
Reminder mapping orchestration is required by phase 7.

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
  history, post-edit plan, session) becomes canonical.
- The session crate (phase 7) depends only on `editor-core`, `app-core` and
  `table-syntax`. It must not spawn threads or read the clock: add a
  `clippy.toml` in the crate with `disallowed-methods` for
  `std::thread::spawn`, `std::thread::Builder::spawn`, `std::thread::scope`,
  `std::time::Instant::now` and `std::time::SystemTime::now`, and set `disallowed_methods = "deny"` under
  `[lints.clippy]` in its `Cargo.toml`. Configuration alone emits warnings;
  the deny level makes the prescribed workspace Clippy check fail on a
  violation. Verify enforcement with a temporary forbidden call when the
  crate is introduced.

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
| 7. Shared note session | Planned | Step-by-step plan in "Phase 7 implementation plan"; starts with the undo cursor fix and the fold-upkeep and host-only-edit follow-ups |
| 8. Shared display model | Planned | |

Track progress by which semantic decisions have a canonical core owner,
which terminal paths delegate to it, and which core regression tests cover
the contract. Record remaining host-owned correctness policy explicitly.
File sizes and import counts may describe the codebase, but reducing them is
not an acceptance criterion.

### Host-owned correctness policy

Policy that stays in `TerminalApp` after phases 1–6. Phase 7 moves note-local
policy into the shared session and application-wide currency policy into the
rate service in the same crate. Pure scheduling (timers, threads, idle ticks) stays with
each front end.

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
- **Document state.** Now phase 7.
- **Pre-existing bug, also on `main`.** Typing `one`/`two`, then `o` with
  `three`/`four`, `gg V d`, `u`, Ctrl-R, `u` and `x` in Vim mode joins the
  first two lines (`onetwo`) instead of deleting a character. The restored
  cursor is probably left past the line end; undo should clamp it as Normal
  mode does.
- **Display model.** Now phase 8.

## Session execution (2026-10-10)

Branch: `session`, based on `af2a090` (current `main`). The owner explicitly
approved unattended code changes and one commit per step, including mechanical
moves. Existing golden cases and performance limits remain unchanged.

- [x] Step 0: branch, clean main snapshot and baseline capture.
- [x] Step 1: Normal-mode undo/redo cursor regression.
- [x] Step 2: fold upkeep from exact edit spans.
- [x] Step 3: core same-line replacement plans.
- [x] Step 4: core line insertion/removal plans.
- [x] Step 5: document crate and terminal view separation.
- [x] Step 6: undo, dirty and reminder session state.
- [x] Step 7: session edit pipeline and undo/redo.
- [x] Step 8: private document text.
- [x] Step 9: shared calc state.
- [x] Step 10: session calc upkeep.
- [x] Step 11: calc preparation/index jobs.
- [x] Step 12: session fold structure.
- [x] Step 13: open/reload/outside-change/leave policy.
- [x] Step 14: save and autosave jobs/policy.
- [x] Step 15: script tickets and result validation.
- [x] Step 16: application-wide rate service.
- [x] Step 17: headless host integration.
- [ ] Phase 8: shared semantic display and source mappings.
- [ ] Final round: checks, performance A/B, live terminal and diff review.
- [ ] Step 18: final contracts and readiness evidence.

Step 1: the added replay failed before the fix and passes afterwards; all
existing cases remain unchanged. Restored Normal-mode cursors clamp before
history checkpoints; Insert-mode exhaustion behavior is retained. Workspace
tests, formatting and Clippy warning comparison passed.

Step 2: exact deltas locate off-cursor fold edits; unchanged same-line edits
retain their snapshot allocation. Core regression tests and workspace checks
passed, with no new Clippy warnings. Release large-note gates passed; no
30k/100k metric crossed the investigation threshold against the lower baseline.

Step 3: same-line replacements use character ranges and exact byte metadata
from editor-core. Unicode, deletion, clamping and no-op cases pass. Calc trailer
rewrites retain their existing enclosing transaction; all workspace checks
passed with unchanged golden fixtures and no new Clippy warnings.

Step 4: whole-line plans use one splice, preserve owned strings and expose
reminder fates. Workspace checks, line-plan tests, golden replay and table gates
passed. Large-note limits passed. Build-load noise disappeared on serial reruns;
30k paste remains about 5.70 ms versus 5.05 ms initial lower baseline (5.14 ms
fresh main), while 100k/400k remain near main. This benchmark uses the unchanged
shared Vim path, not the changed empty-register fallback. Final A/B must recheck
this investigation; performance qualification remains open.

Ordering clarification for step 7: direct edits currently run synchronous calc
before recording history, whereas keyboard edits may defer calc until after
history. Preserve both through a temporary finalize-after-upkeep continuation
and remove it with session calc ownership in step 10; the final single-call
contract is not met while that continuation exists.

Step 5: `note-session::Document` owns text/cache/cursor/selection; viewport
state stays in terminal `ViewState`. Existing dependencies only. The temporary
thread-spawn probe failed under Clippy from the workspace root and was removed.
Document tests and full workspace checks passed with no new warnings.

Step 6: undo/dirty/reminder state and pure mapping moved into `NoteSession`.
Save/leave/render identity uses `edit_seq()`; terminal Instants remain for
debounce. Existing save-race, reminder and workspace tests passed; no new
Clippy warnings. Failed-save pause semantics remain unchanged until step 14.

Step 7: session requests prepare/apply primitives, replacements, word deletion,
pastes/imports, visual edits and compound operations; text undo/redo restores
reminders in the crate. Empty-buffer initialization is also session-owned. The
terminal consumes outcomes and retains presentation/calc upkeep. Compound edits
rebuild final calc metadata once rather than inspecting intermediate buffers
after mutation. Direct-versus-keyboard history ordering is preserved through
`defer_history`/`finish_edit`, still due for removal in step 10.

Review fixes preserve the previous table cursor column and empty-line reminder
bookkeeping. `O`, Escape now immediately records its inserted empty line, fixing
an existing unsaved-edit gap; a focused save/undo/redo test covers it. Existing
golden fixtures are unchanged. Workspace checks: 1,317 passed, six ignored, no
new warnings. Latest large-note capture passed all limits and crossed no
investigation thresholds; 30k paste is 5.07 ms versus 5.05 ms lower baseline.

Step 7 unified checker: table and large-note gates passed with isolated config
and data. Startup ran successfully but exceeded the stored April baseline;
final startup acceptance is the required five-run comparison against main.
No performance baselines were edited.

Baseline workspace: 1,294 tests passed, six ignored. The initial sandbox run
could not create a private runtime image; the approved rerun passed. Baseline
Clippy warnings and performance captures are in `/tmp/slate-session-*.log`.

## Phase 7 implementation plan

Written for an agent implementing phase 7 without prior context. Read
`AGENTS.md`, this document's phase 7 section ("Shape" and "Result
acceptance") and `roadmap/editor-engine-contract.md` first. Line counts and
names below were checked against the code on `main` at `4020693`; re-check them before
each step, since earlier steps move code.

### Ground rules

- Work on a branch named `session`, created from an up-to-date `main`.
- One commit per step below, using the given commit title. Keep each commit
  under 300 changed lines excluding tests. Steps marked **[mechanical]**
  are renames or moves that may exceed that; stop and get explicit approval
  before committing one. Follow the repository owner's rules for asking
  before commits and pushes.
- Every step is behavior neutral, except steps marked **[fix]**, which fix
  one recorded bug and add a regression test for it.
- Keep `cargo build -p slate` free of new dependencies except the new crate.
  The new crate may use crates already in the workspace (`rustc-hash`,
  `aho-corasick`) and nothing else.
- Do not change `perf/baselines/*.json`. A regression is fixed or reported,
  never absorbed into a limit.
- The typing path must not get new work. `EditOutcome`, `Effects` and other
  values returned per key must not allocate on that path (an empty
  `Vec::new()` is fine; a status `String` or a non-empty job list only when
  there is something to report). Cross-crate calls are free in release
  builds (`lto = "thin"`, `codegen-units = 1`), so no `#[inline]` tuning is
  needed; do not trade structure for imagined call overhead.
- Stop and ask instead of improvising when: a step needs a behavior change
  not listed here, a perf regression from the table below cannot be removed,
  a golden replay fixture would have to change, or the code no longer
  matches what a step describes.
- After finishing, update "Progress" and the contract document (step 18).

**Verification after every step** (run from the repository root):

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets 2>&1 | grep -E '^(warning|error)' | sort | uniq -c > /tmp/clippy-step.txt
diff /tmp/clippy-baseline.txt /tmp/clippy-step.txt   # no new warnings or errors
cargo test --workspace
```

**Performance check** for steps marked **[perf]**:

```sh
cargo test --release -p slate --lib large_note_perf -- --ignored --nocapture > /tmp/ln-step.txt 2>&1
cargo run --release -p slate --bin perf-check     # read the `table` line when a step touches tables
```

Compare each metric's p50 and p95 for 30k and 100k lines with the step 0
baseline. Investigate any metric whose p50 rises by more than 10% and more
than 0.3 ms with at least 5 samples (`n` column); `type_table` has only 2
samples at 30k, so judge it at 100k. Every metric must stay within
`perf/baselines/large_note.json`. 400k lines are reported, not gated, but
compare them too.

### What the crate must look like when done

Package `note-session` (library `note_session`) in `crates/session`, a
workspace member depending on `editor-core`, `app-core`, `rustc-hash` and
`aho-corasick`.

```text
crates/session/
  Cargo.toml        [lints.clippy] disallowed_methods = "deny"
  clippy.toml       disallowed-methods: std::thread::spawn, std::thread::Builder::spawn,
                    std::thread::scope, std::time::Instant::now, std::time::SystemTime::now
  src/lib.rs
  src/document.rs   Document: lines (private), cursor, selection anchor,
                    text generation, joined-text cache
  src/session.rs    NoteSession: identity, revision, dirty, edit sequence,
                    history, undo policy, reminder state, fold structure, calc state
  src/edit.rs       SessionEdit (requests), EditContext, EditOutcome
  src/reminders.rs  reminder marks, pending line changes, mapping (from reminder_helpers.rs)
  src/folds.rs      fold structure caches and upkeep application
  src/calc.rs       CalcState (from calc_cache.rs) and calc upkeep
  src/jobs.rs       job and result types, tickets, runner functions
  src/lifecycle.rs  open, reload, outside change, leave and save policy
  src/scripts.rs    script result acceptance
  src/rates.rs      RateService (from terminal/app/currency.rs)
  tests/headless_host.rs
```

Rules the code must satisfy, with a GPUI front end in mind:

- **No front-end types or units.** Positions are line plus character column
  (cursor) or byte offset (exact edits), as `editor-core` documents them. No
  cells, display widths, colors or key types.
- **Text changes only through the crate.** `Document` keeps `lines` private
  from step 8 on. Every mutation is a `SessionEdit` the session prepares and
  applies in one call, so a prepared plan can never meet a changed buffer.
- **No threads, no clock.** Work that ran on a thread or touched the database
  becomes a job. Jobs and job results are `Send + 'static` (add
  `fn assert_send<T: Send + 'static>() {}` checks in tests); `NoteSession`
  itself need not be `Send`, since a GPUI entity lives on the main thread.
  Time arrives as input (`EditContext::since_last_edit`, `now_ms` arguments).
  Synchronous calc evaluation on the calling thread is allowed; it is what
  the terminal does today on the input path.
- **Runner functions live in the crate.** For each job type, a free function
  (`jobs::run_save(job, &Db) -> SaveResult`, and so on) does the work, so both
  front ends share it and only choose the thread or executor.
- **Results are validated by the session**, following "Result acceptance".
  A job carries a `JobTicket { session_id, note_id, text_generation, epoch }`;
  each result type documents which fields it checks.
- **Effects, not callbacks.** Session calls return what changed: an
  `EditOutcome` (the `EditDelta`, first changed line, calc action, fold effect,
  title changed) or an `Effects` value (repaint range in source lines, status
  text, jobs to run).
- **Edit identity is a counter.** Replace uses of `last_edit: Instant` as an
  identity (save `edit_mark`, `autosave_paused_at`,
  `SelectionStatsKey::last_edit`) with `NoteSession::edit_seq() -> u64`. The
  terminal keeps an `Instant` only for debounce timing.

### Step 0: branch and baselines (no commit)

1. `git switch main && git pull --ff-only && git switch -c session`.
2. Record warnings: `cargo clippy --workspace --all-targets 2>&1 | grep -E
   '^(warning|error)' | sort | uniq -c > /tmp/clippy-baseline.txt`.
3. Record performance on `main` twice and keep both outputs:
   `cargo test --release -p slate --lib large_note_perf -- --ignored
   --nocapture > /tmp/ln-baseline-1.txt 2>&1` (and `-2`). Use the lower p50
   of the two runs per metric as the baseline.
4. Record `cargo run --release -p slate --bin perf-check` output. The startup
   check currently fails on some machines on `main` itself (see "Completion
   checks"); startup is compared A/B against `main`, not against its limits.
5. Run `cargo test --workspace` and note the test count.

### Prerequisites

**Step 1 [fix]: `Clamp the cursor after undo in Normal mode`.**
Bug from "Follow-ups": in Vim mode, typing `one`/`two`, `o` with
`three`/`four`, `gg V d`, `u`, Ctrl-R, `u`, `x` joins the first two lines.
Add the sequence to `crates/tui/src/terminal/tests/golden/vim_replay.json`
(expected: `x` deletes one character), confirm it fails, then clamp the
restored cursor in the undo/redo path (`undo_text_action`,
`redo_text_action` in `editing.rs`) the way Normal mode clamps after
Escape. Check that redo of a linewise delete keeps its current cursor.

**Step 2 [perf]: `Locate fold upkeep edits from EditDelta`.**
`folding::upkeep::plan_fold_upkeep` guesses a one-line insert or delete at
the cursor line. Add a `delta: Option<EditDelta>` parameter: with a delta,
map the affected range from it (any line, any span; spans other than one
line in or out still return `Recompute`); without one, keep the cursor
fallback. Pass the delta from `mark_edited_from_line_with_span` through
`recompute_folding_if_needed`. While there, stop cloning the current line
three times per keystroke: compare `&str`, then replace the snapshot entry
with `std::mem::replace`. Core tests: insert and delete away from the cursor,
undo of a multi-line delete, unchanged same-line typing.

**Step 3: `Plan same-line replacements in editor-core`.**
Add `editor_core::buffer::prepare_line_replace(lines, line, char_range,
text) -> Option<LineReplacePlan>` carrying `ExactTextEdit`, `EditDelta`
(1 → 1) and the resulting cursor column, plus `apply_line_replace`. Use it
in `apply_variable_autocomplete_pick`, `apply_calc_tab`, the trailer refresh
in `run_calc_recompute`, wiki-link selection and heading-suffix removal.
Record each through `mark_edited_with_delta` instead of `mark_edited()`, so
history uses the span path. Core tests: Unicode ranges, empty replacement,
range past line end.

**Step 4 [perf]: `Plan line insertion and removal in editor-core`.**
Add `prepare_insert_lines(lines, at, new_lines)` and
`prepare_remove_lines(lines, start, end)` with `EditDelta`, cursor and the
block fates reminders need (`old line -> Option<new line>`), applied with one
`Vec::splice`. Use them for Vim linewise paste (today one `insert` and one
clone per line), `o`/`O`, and `prune_empty_table_continuation_row_at_cursor`.
Route Vim charwise paste through the existing paste plan. Afterwards, check
direct buffer mutation in the terminal with:
`grep -rnE 'editor\.lines(\[[^]]*\])?\.(insert|remove|push|splice|truncate|push_str|insert_str|replace_range|clear)\(|editor\.lines\[[^]]*\] *=[^=]' crates/tui/src --include='*.rs' | grep -v /tests`.
The only matches left may be empty-buffer guards (`push(String::new())`);
step 7 removes those.
Check `paste_line` and `open_line` in the perf output.

### The crate and its state

**Step 5 [mechanical]: `Add the note-session crate with the document model`.**
Create the crate (layout above) with `Document` holding the document fields
of `EditorModel` (`crates/tui/src/terminal/app/mod.rs`): `lines`,
`joined_text_cache`, `text_generation`, `cursor_line`, `cursor_col`,
`selection_anchor`, keeping those names and keeping them `pub` for now.
Move `cursor()` / `set_cursor()` with them. In the terminal, `TerminalApp.editor`
becomes a `note_session::Document`; the view fields (`scroll_line`,
`scroll_row_offset`, `scroll_col`, `markdown_formatting_right_boundary_exit`,
about 85 references) move to a new terminal struct at `self.view`. Add the
crate to the workspace members. Verify the clippy rule by adding a temporary
`std::thread::spawn` call: `cargo clippy -p note-session` must fail. Remove
it before committing.

**Step 6 [mechanical]: `Move undo, dirty and reminder state into the note session`.**
Add `NoteSession` and move into it from `TerminalApp`: `history`,
`undo_policy`, `dirty`, `reminder_ghosts`, `pending_line_edits`,
`reminders_generation`, `persisted_reminders_generation`, and the types
`LineReminderGhost`, `ReminderMarks`, `ReminderUndoEntry`, plus
`PendingLineChange` and the pure mapping functions of
`terminal/app/reminder_helpers.rs` (`line_change`, `fates`, `move_lines`).
Database loading and persistence of reminders stay in the terminal for now.
Introduce `edit_seq` and replace the identity uses of `last_edit` listed
above. The terminal holds `session: NoteSession` and passes `&mut self.editor`
where needed. No logic changes.

**Step 7: `Apply edits through the note session`.**
Add `NoteSession::apply(&mut self, doc: &mut Document, edit: SessionEdit,
ctx: EditContext) -> Option<EditOutcome>`. `SessionEdit` covers every text
change: primitives (`PrimitiveEdit`), `EditOperation` (today's
`apply_edit_operation`), plain and table-cell paste, table import, backward
word delete, visual selection plans, and the line plans from steps 3–4. In
one call it prepares the core plan, records the pending reminder change from
the exact edit or block fates, applies the plan, bumps `text_generation`,
drops the joined-text cache, sets dirty, increments `edit_seq`, records
history with the `UndoGrouping` from `ctx`, and maps reminders. This is the
existing order in `apply_buffer_primitive`, `mark_edited_from_line_with_span`
and `record_history_after_edit`; keep it exactly, and keep each variant's
fast path: a same-line character edit records no structural reminder change
(only edits that add or remove lines call `note_line_edit` today) and
refreshes calc metadata for one line only. Do not route every variant
through the most general path. Run the **[perf]** check for this step even
though it is not marked, and compare `type_prose`, `type_calc`,
`delete_char` and `enter`. Calc and fold upkeep stay
in the terminal and run from the returned outcome. Add `undo(doc)` and
`redo(doc)` returning outcomes the same way. Session tests: reminder
positions through joins, splits, block replacement and undo/redo; dirty and
`edit_seq` after no-op edits.

**Step 8 [mechanical]: `Make the note session the only writer of note text`.**
Make `Document::lines` private with `lines() -> &[String]` and
`set_text(&mut self, text: &str)` (used when a note opens or reloads). Replace
the terminal's reads with `lines()` (about 620 references) and move test
setup that assigns `app.editor.lines` (25 places) to `set_text`. The grep
from step 4 must now match nothing at all in `crates/tui`.

### Calc

**Step 9 [mechanical]: `Move calc state into the note session`.**
Split `CalcCache` (`crates/tui/src/terminal/calc_cache.rs`): everything except
the two worker receivers (`range_context_build`, `index_build`) becomes
`note_session::calc::CalcState`, owned by `NoteSession`; the receivers move
to a terminal `CalcWorkers` struct. Move `VariableNames`
(`terminal/render_styles.rs`) into the crate unchanged, including its lazily
built matcher, so the renderer keeps reusing it; the terminal imports it.
`CalcRuntime` (scheduling flags and the viewport range) stays in the terminal.

**Step 10 [perf]: `Run calc upkeep in the note session`.**
Move the calc work that runs after an edit into `CalcState` methods taking
`&Document` and a `CalcInputs` value (feature mask, module flags, thresholds,
whether a key is still being handled, and a way to get extern variables): the
`CalcAfterEdit` dispatch from `mark_edited_from_line_with_span`,
`try_remap_calc_results_after_structural_edit`, `recompute_calc_range`, and
the non-scheduling part of `run_calc_recompute` and
`ensure_calc_for_viewport`. They return a `CalcEffect` (done, schedule an idle
pass, recompute after the current key, refresh the viewport). The terminal
keeps `CalcRuntime`, debounce times, idle ticks and `key_depth`. Extern
variables must be fetched lazily: today `run_calc_recompute` locks
`CrossNoteVarIndex` and copies `extern_vars_for` only after deciding to
recompute (`editing.rs`, the `incremental_extern_vars` block). Pass a closure
or a small trait object that the session calls only on paths that evaluate
cross-note references, never a precomputed `Vec` built before every call,
which would add a lock and a copy to every keystroke. Split into two commits
(remap and range first, full recompute second) if one exceeds 300 lines.
Check `type_calc`, `enter`, `delete_line`, `undo` and `scroll_*`.

**Step 11 [perf]: `Turn calc preparation into note-session jobs`.**
Viewport preparation (`start_viewport_calc_preparation` /
`install_viewport_calc_preparation`) and the background index build
(`install_background_calc_index`) become `Job::PrepareCalc` and
`Job::BuildCalcIndex` with a `JobTicket`. Their bodies become runner
functions in `jobs.rs`; the terminal spawns a thread that calls the runner
and sends the result back. `NoteSession::complete(result)` installs it only
when the ticket still matches: same note and session, and the checks the
current code does (text rehash for the prepared context, reset
`cross_note_refs_generation`, `CrossNoteVarIndex::epoch()` for index writes,
the currency generation). Session tests: a result for a switched-away note,
a stale text generation, an epoch change. Check `open`, `open_calc` and
`idle_tick`.

### Folds

**Step 12 [perf]: `Move fold structure into the note session`.**
Move `line_has_structure`, `line_text_snapshot`, `ranges`, `range_by_start`,
`rescan_pending` and `analysis_ready` from `FoldingState`
(`crates/tui/src/terminal/folding_state.rs`) into `note_session::folds`.
Collapsed ranges, the visible/real line maps, hidden owners, placeholders
and the pending `z` prefix stay in the terminal. `EditOutcome` carries the
fold effect from step 2's planner; the terminal applies view-map changes.
The idle rescan becomes a session method the terminal calls from its tick.

### Note lifecycle

**Step 13: `Move note open and reload policy into the note session`.**
Add `NoteSession::open(note: &Note, doc: &mut Document) -> Effects`, which
resets history, undo policy, reminders, folds and calc state and increments
`session_id`; it replaces the state-resetting parts of `set_active_note` and
`reload_active_note`. Add `outside_change(stored_revision) ->
OutsideChange` for `maybe_take_outside_change` and `leave_decision()` for
`can_leave_note`. Reading notes from the database stays in the terminal.
Session tests: switching away and back produces a new `session_id`.

**Step 14: `Move save and autosave policy into the note session`.**
Add `request_save(doc, ctx) -> Option<SaveJob>` (body, reminders to store,
expected revision, `edit_seq` at the time) and `jobs::run_save(job, &Db)`,
taking the body of the thread in `start_background_autosave`. Add
`complete_save(result)` implementing "Result acceptance" exactly as
`poll_background_save` does today: acknowledge the persisted snapshot even
after newer typing, advance the stored revision only when the expected
revision still matches, clear dirty only when `edit_seq` is unchanged,
record the reminder generation, and pause autosave at the current
`edit_seq` on failure. `format_on_save` stays a terminal command run before
the request. Before switch, reload and close, the terminal waits for an
in-flight save and passes its result to `complete_save`. Session tests:
typing during autosave, a save completing after a newer revision, a failed
save pausing autosave until the next edit, reminders-only saves.

### Scripts and rates

**Step 15: `Validate script results in the note session`.**
`start_script_inner` asks the session for a `ScriptTicket` (session id, note
id, text generation, editable). `poll_script_result` passes the response to
`NoteSession::accept_script_result(ticket, response)`, which returns the
`SessionEdit` to apply or the reason it was rejected; the status text stays
in the terminal. Cancellation on note change keeps working through the new
`session_id`. Session test: switch away and back during a run rejects it.

**Step 16: `Move exchange-rate refresh policy into the session crate`.**
Move the coordination in `crates/tui/src/terminal/app/currency.rs` (one fetch
at a time, startup fetch, `:currency refresh`, applying a result) into
`note_session::rates::RateService`, which returns `RateJob`s and has
`jobs::run_rate_job`. `complete` installs rates through `app_core::currency`
and reports whether they changed. On a change, the terminal calls
`NoteSession::invalidate_calc()` and `CrossNoteVarIndex::invalidate_calculations()`.
`RateService` lives in the terminal app beside the session, not inside it,
because it outlives note switches. Session tests: refresh across a note
switch, identical rates, a failure keeping cached rates.

### Validation

**Step 17: `Test the note session through a headless host`.**
Add `crates/session/tests/headless_host.rs`: a minimal front end that holds
a `Document` and `NoteSession`, applies edits, runs jobs synchronously and
also out of order and late, and asserts results and effects. Cover the
"Done when" list of phase 7: typing during autosave, save completion after
a newer revision, switch-away/back script rejection, global rate refresh
across note switches, reminder positions through joins, block replacements
and undo/redo. Also assert the `Send + 'static` bounds on jobs and results,
and that `cargo tree -p note-session -e normal` has no `ratatui`,
`crossterm` or GUI crates.

**Step 18: `Record session extraction validation`.**
Run the final round below, then update this document ("Progress", the
host-owned policy table, "Follow-ups") and
`roadmap/editor-engine-contract.md` (session, document, job and rate
contracts). This commit contains documentation only, unless the round finds
bugs; fix each found bug in its own **[fix]** commit before this one.

### Final round: bugs and performance

Do all of this after step 17, on the finished branch.

1. **Checks.** Verification commands above; the test count must be at least
   the step 0 count plus the new tests.
2. **Invariants.**
   - The step 4 grep matches nothing in `crates/tui`.
   - `grep -rn 'Instant::now\|thread::spawn\|SystemTime::now' crates/session/src`
     matches nothing, and the clippy rule still fails on a temporary call.
   - Every row of "Host-owned correctness policy" is moved or marked as pure
     scheduling.
3. **Performance A/B against `main`.** Build `main` in a separate worktree
   with its own target directory; sharing one target directory between two
   checkouts of the same path crates mixes their artifacts:
   ```sh
   git worktree add /tmp/slate-main main
   (cd /tmp/slate-main && CARGO_TARGET_DIR=/tmp/slate-main-target \
     cargo test --release -p slate --lib large_note_perf -- --ignored --nocapture)
   ```
   Run each side three times, alternating, and compare medians per metric
   and size (30k, 100k, 400k) with the thresholds above. Also compare
   `table-perf` and the startup marks from `perf-check` (5 runs each side).
   Record the table in this document as for phase 5. Remove the worktree and
   target directory afterwards.
4. **Live terminal.** Run a debug build in tmux with isolated directories
   and without a display, so the system clipboard is never touched:
   ```sh
   S=$(mktemp -d); mkdir -p "$S/config/slate" "$S/data"
   printf '[editor]\nvim_mode = true\n' > "$S/config/slate/config.toml"
   tmux new-session -d -s slate-check -x 110 -y 24 \
     "env XDG_CONFIG_HOME=$S/config XDG_DATA_HOME=$S/data HOME=$S \
      WAYLAND_DISPLAY= DISPLAY= target/debug/slate --new"
   sleep 1
   tmux send-keys -t slate-check i 'price := 20' Enter 'price * 3' Escape
   tmux capture-pane -p -t slate-check
   ```
   Script and compare these scenarios against a `main` build with the same
   keys: typing, Enter, Backspace across lines, visual and linewise delete,
   yank and both pastes, `o`/`O`, undo/redo chains (including the step 1
   sequence), a calc variable used below its definition, a table edit, a
   reminder on a line that is then joined and undone, switching notes during
   autosave (wait 1 s after typing), and a script run interrupted by a note
   switch. Screens must match `main` except where step 1 fixed the bug.
5. **Review.** Read the full diff against `main` for: logic left in the
   terminal that decides what text or state becomes (it belongs in the crate),
   new clones or allocations on typing paths, API that would not suit a GPUI
   entity (front-end units, threads, callbacks), and public items without a
   documented unit or invariant.
6. **Report.** Record findings in "Follow-ups" and the results in
   "Completion checks", stating anything not verified.

## Second front end (outside this plan)

Notes for when a GUI starts, recorded so phases 7 and 8 keep them possible:

- Keep the GUI out of the root workspace at first: its own workspace (listed
  under `exclude`) with path dependencies on the core crates and the session.
  Cargo resolves the whole workspace, so a GPUI git dependency in it would be
  fetched by every terminal-only build and CI run, and `default-members` does
  not change what `--workspace` builds. Give it its own CI job with the GPUI
  system libraries.
- GPUI's text input uses UTF-16 ranges, marked (IME composition) text and
  pixel bounds. The GUI needs an adapter to the session's character columns
  and byte-offset edits, converted per line, and a decision on how
  composition interacts with undo grouping.
- Decide the editor font early: phase 8 assumes monospace.

Step 8: document lines are private outside the session crate. Borrowed reads
replace terminal field access; lifecycle replacement uses `set_text` and
capacity trimming uses `compact`. Unicode/trailing-line/cache-generation
coverage was added. Workspace formatting, clippy (no new warnings), all
1,318 tests and existing golden cases pass. Read-only diff review found no
behavior or allocation regression.

Step 9: CalcState and the unchanged lazy VariableNames matcher/tests moved
to the session crate. CalcWorkers owns both receivers in the terminal;
CalcRuntime remains host scheduling state. All workspace checks pass with
no new warnings; read-only extraction review found no initialization or
receiver-ownership change.

Step 10a: metadata maintenance, structural result remapping, viewport result
refresh and range evaluation now run in CalcState; clocks, scheduling and
geometry remain terminal-owned. Cross-note scans retain take/restore ownership
and generation reuse; extern variables are acquired only after evaluation
guards. Two shared tests cover suffix remapping and absolute range slots.
Workspace checks pass with no new warnings. Release large-note gates pass
(13.23 s), with no 30k/100k investigation thresholds crossed. Full recompute
remains for Step 10b, followed by edit-upkeep coordination; Step 10 stays unchecked.

Step 10b: full/incremental calc recomputation, pathological-window policy,
table seed reuse, selection-aware trailer rewrites and cache snapshots moved
into the session. The public DerivedLineReplace bypass was removed. Lazy
extern lookup occurs inside actual incremental recomputation. Shared trailer
coverage and all workspace checks pass, with no new warnings. Release
large-note gates pass (13.29 s), no 30k/100k investigation thresholds crossed.
A separate Step 10c coordinates edit upkeep and removes deferred history;
Step 10 remains unchecked until that boundary is complete.

Step 10c: session apply now coordinates calc upkeep and finalizes history in
one transaction. CalcEffect returns host scheduling; a borrowed CalcProvider
supplies only lazy external data and evaluation inputs. Removed deferred-history
continuations and duplicate host upkeep. Regression coverage includes recursive
table import, visual-yank scheduling, pruning/replacement redo cursors, unchanged
completion and disabled math without a provider. All 1,326 workspace tests pass,
no new clippy warnings. Release large-note gates pass with no 30k/100k
investigation thresholds crossed; final alternating main comparisons remain.

Step 11: owned calc preparation/index jobs and runners now live in the session.
Tickets fence note lifetimes, config masks, currency generation and database
epochs. Prepared contexts retain engine text revalidation; stale scans cannot
publish active-note dependencies. Export loading pins the dispatch epoch through
recursive reads and publication. Late indexes catch up before exposing names;
accepted completions repaint. Five shared race/Send tests and a repaint
regression pass, as do workspace checks (no new warnings). Release large-note
gates pass (13.32 s); no 30k/100k investigation threshold crossed.

Step 12: fold structure, incremental upkeep, range indexing and rescans moved
into the session. EditOutcome carries fold effects; the terminal retains
collapse choices, line maps, placeholders and the z-prefix timer. Fold policy
uses the resulting line count; a shared regression covers both threshold
crossings. Workspace checks pass, no new warnings. A loaded performance run
failed broadly; serial reruns pass all limits and the latest crosses no
investigation thresholds (100k paste 9.60 ms versus baseline 9.12 ms).

Step 13: session open owns lifetime, revision/access metadata, history capacity
and resets; installed reminders share history marks. Outside-change conflict
and deletion deduplication, stored replacement undo boundaries, and repeated
leave decisions use session policy. The terminal drains pending saves before
switch/reload/close even when autosave is disabled. Four shared lifecycle tests
and all workspace checks pass, no new warnings; read-only review found no
remaining host reads of stale revision/access metadata.

Step 14: shared SaveJob snapshots capture body/reminders, expected revision,
lifetime and edit identity. The shared runner performs manual, reminder-only
and background writes; session acknowledgement independently fences revision,
dirty and reminder generations. Failures pause the current edit identity until
a new edit. Manual saves transfer/restore the joined allocation; background
saves clone only their required snapshot. Seven shared race/ownership/Send tests
and all workspace checks pass, no new warnings. Format commands, workers,
receiver polling, status and note-list refresh remain terminal-owned.

Step 15: ScriptTicket captures note lifetime, text generation, access and
byte-offset target. Shared acceptance returns a session edit that revalidates
on consumption and isolates undo. Message-only results validate without edits.
Seven shared tests cover switch-away/back, Unicode edits, access changes,
delayed consumption, cursor/undo and Send bounds. Workspace and existing
terminal script tests pass, no new warnings; Visual/Vim reset and process
cancellation remain in the host.

Step 16: RateService lives beside the note session and owns one-fetch policy,
startup cache acceptance, refresh generation, cancellation and installation.
Shared startup/fetch runners perform IO; publication is fenced while writing the
cache. Identical rates skip calc invalidation; failures preserve installed
rates. The host retains workers, receiver polling and joins. Shared policy/Send
coverage plus existing actual-process startup/currency tests and all workspace
checks pass, no new warnings. Note lifetimes do not affect global refresh.

Step 17: a frontend-free integration host runs real SQLite save and calc jobs
and delayed/out-of-order completions. Five tests cover save acknowledgement,
conflicts, reminder joins/block edits/history, lifetime/epoch rejection, script
switch-away/back, global rates and owned Send messages. Workspace checks pass;
normal dependencies contain no terminal or GUI libraries.

Phase 8 implementation: the renderer consumes shared semantic styles; marker
visibility, formula labels, media placeholders and monospace table reflow live
in note-session. Explicit scalar provenance composes formula/link/media/table
substitutions with hiding; generated owners are noneditable and ghosts unowned.
Unicode conversion, affinity, reflow and cache invalidation tests pass. Source
search/selection ranges map through transforms. Styling and row/media caches
are bounded and keyed by their semantic inputs. Workspace checks pass without
new warnings; final performance and live-terminal qualification are pending.

Final bug round, undo: headless callers previously bypassed reminder action
ordering and locked-note protection. Shared dispatch now owns text/reminder
actions, derived-state upkeep and Normal/table source-cursor checkpoints;
hosts consume effects without acknowledging history again. Four shared
regressions and workspace checks pass, with no new warnings.
