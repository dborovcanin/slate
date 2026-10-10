# EditorEngine Contract

This document defines the canonical shared-core contract for editor semantics.

## Ownership

- `crates/editor-core/src/engine.rs` is the canonical entrypoint for:
  - command normalization and resolution
  - command suggestions
  - note-security command parsing
  - vim key stepping (`VimState` + `VimContext` -> `VimStep`)
  - module-command planning (`CommandId` + current module state -> deterministic plan)
- `crates/editor-core/src/folding.rs` owns fold-range computation plus incremental line-edit mapping/rebuild decisions.
- `crates/editor-core/src/history.rs` owns the line undo store and its generic attached marks.
- `crates/session` coordinates these lower-level contracts through `NoteSession`.
  In the buffer/history contracts below, "host" means the session unless the
  responsibility explicitly concerns frontend input, geometry or I/O.
- Frontends call session transactions for note-text changes and semantic undo;
  they read the separate `Document` and may move its cursor/selection.
- Frontends own rendering, viewport/collapse choices, input mapping, dialogs,
  clipboard, timers and executor choice. Shared job runners perform persistence
  and other I/O; frontends execute them and return results to the session.

## Current input/output contracts

- Buffer contract (`editor_core::buffer`):
  - `prepare_text_change`: input is the current lines, a byte-offset `TextChange`
    and current document byte length; output is replacement lines plus
    `ExactTextEdit` (pre-edit line/byte coordinates, boundary-line byte lengths
    and inserted line breaks) and `EditDelta` (line-span invalidation summary).
  - Record host metadata before calling `apply_text_change_in_place`, which
    consumes the prepared change and mutates only the affected lines. The
    buffer must stay unchanged between preparation and application.
  - Change offsets must be UTF-8 boundaries; offsets beyond the document
    length retain the existing end clamping. The host still applies compound
    changes in descending offset order, records each change before mutation
    and retains the existing undo/calc transaction boundary.
  - Offset helpers use bytes and count inter-line newlines. They do not build
    a joined copy of the document.
- Whole-line edit contract (`editor_core::buffer::lines`):
  - Insertion accepts borrowed or owned lines and applies one splice. Owned
    strings retain their allocations. Removal uses a clamped half-open range;
    removing all text retains an empty line without inheriting removed marks.
  - Plans expose cursor, delta and relative reminder fates. Prepare and apply
    against the same buffer; the host retains its cursor and transaction policy.
- Same-line replacement contract (`editor_core::buffer::replace`):
  - Character ranges clamp to the line end; plans carry exact pre-edit byte
    coordinates, a 1-to-1 delta and the resulting character cursor.
  - Missing lines, reversed ranges, multiline text and unchanged replacements
    are no-ops. Apply to the unchanged buffer; the host owns transaction order.
- Primitive buffer contract (`editor_core::buffer::primitives`):
  - Input: lines, `BufferCursor` (line and character column), and
    `PrimitiveEdit` (character/text insertion, newline, Backspace or Delete).
  - `prepare_primitive_edit` returns an allocation-free plan with exact byte
    metadata, a line-span summary and the resulting character cursor, or
    `None` for an ignored input or document-boundary no-op.
  - The host records structural reminder metadata before consuming the plan
    with `apply_primitive_edit`. The buffer must stay unchanged between these
    calls. Insertion text is borrowed; mutation retains the existing in-place
    insert, remove, split and join paths.
  - Table-specific handling runs first. Calc/history bookkeeping, autoformat
    and transaction boundaries remain in the host. Empty buffers are initialized
    by insertion/newline application; their deltas report zero replaced lines.
    Literal text insertion
    stays within one stored line; newline and paste are distinct operations.
- Paste contract (`editor_core::buffer::paste`):
  - Normalize CRLF and CR to LF before selecting table-cell or plain paste.
  - Plain preparation reports exact pre-edit coordinates, line spans and the
    resulting character cursor. It borrows normalized parts and retains the
    existing current-line clone; application keeps the per-line insert path.
  - Table-cell preparation uses the cached core table planner. The host
    records the old/new block mapping before core applies the replacement
    and resolves the character cursor.
  - Core decides whether CSV/TSV can become a table and whether it belongs
    on a blank line or below existing text. The host supplies cached fence
    state only after parsing succeeds; opening and closing fences count as
    code. Table-disabled notes and existing table rows retain their guards.
  - Hosts route exact line/block edits to reminder mapping before mutation
    and retain the existing history/calc transaction after mutation. Prepared
    edits must be applied to the unchanged buffer they were prepared against;
    debug builds assert this on the affected line or line count.
- History store contract (`editor_core::history`):
  - `LineHistory<M>` stores text, character-column cursors and opaque attached
    marks together. Recording, undo and redo retain the existing span storage,
    coalescing, redo truncation and bounded entry eviction behavior.
  - `record_edit_span` takes an `EditDelta` describing the replacement of
    pre-edit lines in the stored snapshot by lines in the current buffer.
    It retains the existing span clamping and changed-line storage.
    `take_last_delta` returns the removed and inserted lines for host metadata
    updates; the invalidation summary does not replace this exact text delta.
  - `COALESCE_ANCHOR_MAX_LINES` retains the 5,000-line anchor limit.
- Undo policy contract (`editor_core::history::policy`):
  - `UndoPolicy<R>` owns the text/reminder action sequence and its undo/redo
    position. Reminder payloads remain opaque; the host applies them and
    persists their effects.
  - The host classifies input as an insert session, normal/visual command or
    other input. `LineHistory::begin_input` breaks coalescing for commands. Explicit edit
    boundaries and checkpoints still use the core history methods.
  - `record_text` receives cursor, optional `EditDelta` and `UndoGrouping`
    (session and elapsed time captured before post-edit work). Core coalesces
    insert sessions across pauses, otherwise within 300 ms, selects the span
    fast path above the anchor cap, and adds or removes text action markers.
    Large-note grouping behavior remains unchanged. New entries and evictions
    are tracked explicitly: the action sequence drops the evicted text marker
    and records the new one even when history depth stays at its limit.
  - Recording a new action truncates the action redo tail. Self-cancelling
    merged text edits remove their trailing marker. The host maps reminders
    and records attached marks after text recording, preserving call order.
  - `undo_action` / `redo_action` offer the next action; the host applies it
    before acknowledging with `complete_undo` / `complete_redo`. A reminder
    error leaves the action pending. Existing text-store exhaustion behavior
    is preserved. Note switching clears both store and policy.
- Word editing contract (`editor_core::buffer::words`):
  - Left/right motions take lines, a character cursor, the table-module flag
    and a lazy iterator of visible neighbor line indices in motion order.
    Only the current line and needed neighbors are inspected; the host owns
    fold visibility and supplies the iterator without building a new list.
  - Prose motions retain whitespace, word (alphanumeric/underscore) and
    punctuation classes. Table motions reuse core row-word helpers, skip
    delimiter rows, land on empty rows and step into adjacent prose.
  - `prepare_backward_word_delete` returns a same-line byte range carrying
    `ExactTextEdit` and `EditDelta`, or a previous-physical-line join.
    Apply a range to the unchanged line with
    `apply_word_delete`; joins use the existing Backspace transaction.
    Deletion retains its distinct alphanumeric-only word rule and existing
    table padding bounds. Out-of-range character columns keep their behavior.
  - The host preserves calc/history/reminder bookkeeping and continuation-row
    cleanup. No document copy or eager neighbor collection is added.
- Vim buffer plans (`vim_actions::buffer`) own inclusive visual ranges,
  register contents/modes, cursor placement and line-buffer replacement. Hosts
  record returned reminder mappings before applying a plan to its unchanged
  buffer. Visual deltas include the retained empty line after whole-buffer
  deletion; yanks retain the selected span length and report no text change.
  Changed visual selections use the host's span history path. Viewport/fold
  visibility and clipboard I/O remain host concerns.
- Post-edit calc plans (`calc_plan::plan_after_edit`) read cached signal flags,
  line counts, stale/viewport state and host thresholds without scanning text.
  Hosts execute skip, defer, viewport refresh, remap/recompute or idle work.
  `plan_result_remap` checks unchanged metadata prefix/suffix and affected text;
  it borrows metadata rather than copying hash arrays. The session owns worker
  result acceptance; frontend executors and timers own dispatch and timing.
- Fold upkeep (`folding::upkeep::plan_fold_upkeep_with_delta`) updates affected structure
  and text-cache entries and returns view effects, mapped ranges or a rescan
  decision. Exact `EditDelta` spans locate edits; callers without a delta retain
  cursor-based fallback. It defers expensive analysis when no collapsed ranges are visible;
  the host applies the decision and owns viewport maps and the idle tick.
- Completion (`completion`) owns prefix spans in character columns, candidate
  precedence and filtering. Hosts supply variable/export/title data and retain
  popup selection, database access and dependency preloading. Qualified note
  references take priority over local variables and table helpers.
- Search (`search`) matches each line case-insensitively and returns original
  character ranges, including Unicode lowercase expansion. Queries passed to
  the matching API must already be lowercased. Hosts own match selection and
  navigation; `find_matches` clears and reuses the supplied vector.
- Command contract:
  - input: `CommandMode`, `raw_input`
  - output: canonical command definition (or none), suggestions, normalized input
- Vim contract:
  - input: `VimState`, `VimKey`, `VimContext`
  - output: `VimStep` (`next state`, `actions`, `handled`)
- Module command contract:
  - input: `CommandId`, `ModuleState`
  - output: `ModuleCommandPlan` (`changed`, `next`, `message`)
- Markdown rule contract:
  - input: `ResolvedContext` + rule options
  - output: optional `EditOperation`
- Fold incremental contract:
  - input: existing fold ranges + line-edit deltas
  - output: mapped fold ranges plus rebuild-needed decision
- Calc scope/trigger contract:
  - input: changed-line slices + previous-change metadata + eval context flags + calc feature mask (`math_enabled`, `table_enabled`, `variables_enabled`)
  - output: deterministic eval-window decision (`eval_from`, `eval_to`, dependency flags, `can_use_partial`)
- Calc trailer-refresh eligibility contract:
  - input: previous/current line identity hash, previous result presence, selection guard
  - output: deterministic eligibility decision for commit-style trailer refresh

## Determinism requirements

- The lower-level `editor-core` plans produce identical outputs for identical inputs
  and avoid runtime side effects.
- Session transactions mutate their explicit document/session state. They have no
  clock, event loop or executor; elapsed input timing and scheduling inputs come
  from the frontend.
- Job runners may perform database writes, dependency loads, cache reads/writes
  and script-backed rate fetches. Clipboard and notification delivery remain
  frontend I/O. Result acceptance and acknowledgement belong to shared policy.

## Replay fixtures

Behavior is frozen by fixture-backed tests in `crates/editor-core/tests/golden_replay.rs`:

- `golden/vim_replay.json`
- `golden/module_command_replay.json`
- `golden/command_replay.json`

Terminal behavior is frozen by replay tests in `crates/tui/src/terminal/app/tests/replay.rs`, which drive the terminal app with key sequences and compare against expected snapshots in:

- `crates/tui/src/terminal/tests/golden/vim_replay.json`
- `crates/tui/src/terminal/tests/golden/markdown_replay.json`
- `crates/tui/src/terminal/tests/golden/calc_replay.json`

## Note session and document

`note_session::Document` carries source lines, character cursor/selection, joined
text cache and text generation. Source lines are private to the crate and exposed
as `lines() -> &[String]`. `from_lines`/`from_text` initialize a document; `set_text`
is for open/reload initialization, never an interactive edit bypass. Viewport
state lives separately. Public cursor/selection fields use Unicode scalar columns;
exact text edit metadata and command operation offsets use UTF-8 bytes.

`NoteSession` owns note lifetime/identity, access and stored revision, dirty state,
edit sequence, history/undo policy, reminder marks, calc state and fold structure.
`apply` prepares and consumes a `SessionEdit` against the separate document.
Requests cover primitives, character replacements, paste/import, whole-line edits,
visual ranges, word deletion, byte-offset operations and ticketed script output.
Outcomes describe source-line deltas, register values and calc/fold effects.
Boundary primitives and unchanged replacements preserve dirty state, generation
and edit identity. Typing does not copy the document or allocate edit-plan vectors.

Frontends use `apply_with_upkeep` with calc/fold inputs for a complete transaction.
`apply` uses the same mutation/history pipeline without calc upkeep; it is useful
for hosts that intentionally handle derived-state scheduling separately. The
borrowed `CalcProvider` supplies evaluation inputs, dependency preloads and lazy
external values only on paths that evaluate. Skip/remap paths do not copy extern
variables or acquire their index lock. If synchronous evaluation needs a provider
and none is supplied, the session preserves the edit/history transaction and
returns `CalcWork::Idle`; the host must execute the deferred work.

The session records exact reminder changes before mutation, invalidates text,
updates derived state, records text history and then maps attached reminders.
Calc-derived trailer replacements remain inside this transaction before synchronous
history finalization. End-of-key deferred recomputation preserves input grouping.
Final replacement/pruning cursor positions are established before recording history.
Frontends apply scheduling effects, repaint invalidation and view-map changes after
shared methods return.

## Semantic undo and reminders

`undo_action_with_upkeep` and `redo_action_with_upkeep` select and acknowledge the
shared text/reminder action sequence. Hosts supply `UndoContext`, optional calc
inputs/provider and fold inputs. Text restoration updates generation/dirty state,
clears pending calc splices, performs shared upkeep, normalizes the source cursor
(including Normal-mode and table bounds), and checkpoints it. Reminder restoration
updates marks/generation/edit identity without a text-generation change. Locked
sessions reject restoration. Hosts must not additionally call policy
`complete_undo`/`complete_redo`.

Convenience `undo`/`redo` and `undo_action`/`redo_action` follow semantic action order
without supplied calc/fold upkeep. A reminder-only convenience operation returns
no text outcome. Frontends displaying derived state should use the upkeep APIs.
The host may call `checkpoint_restored_cursor` again after relocating the caret out
of a collapsed view; it keeps shared source-cursor rules while geometry stays local.

`set_reminder` validates access, database-backed reminder support and line bounds,
ignores unchanged requests, and atomically updates the mark map, optional semantic
undo entry, reminder generation, edit identity and history marks. Notification
acknowledgements use `record_undo = false`. `reminder_changed_outside_text` provides
shared bookkeeping for host reconciliation that already changed the map. Persistence
and notification delivery remain explicit host execution.

## Note lifecycle and saving

`open` starts a new `session_id`, including switching away and back to the same
note. It loads the document/metadata and resets history, undo, reminders, calc,
folds and pending policy state. Hosts read the note/reminders and call
`install_reminders`; database loading does not move into the session.
`outside_change` distinguishes unchanged, deleted, conflicting and reloadable
revisions, suppressing repeat conflict/deletion messages. Reminder changes count
as unsaved state. `leave_decision` permits repeated discard only at the same edit
identity after required save/drain work.

Stored-body replacement is a normal undoable `SessionEdit::Operation` planned by
`prepare_stored_replacement`; `acknowledge_reload` advances the stored revision and
clears dirty state without removing that history entry.

`request_save` captures body/reminders together with expected revision, note
lifetime, edit sequence and reminder generation. Manual saves transfer an existing
joined-text allocation; background saves clone the snapshot while retaining the
frontend cache. `jobs::run_save` performs the write and returns the owned snapshot.
`complete_save` restores that allocation only when current, acknowledges reminder
snapshots and advances revision only when the expected revision still matches.
Newer typing remains dirty. Failures pause background saving at the current edit
identity until another edit; explicit saves may retry. Hosts drain/reconcile an
in-flight save before switch, reload or close. Results from other lifetimes are
ignored rather than applied to a reopened note.

## Jobs, script results and exchange rates

Calc preparation/index jobs and results own their inputs and support
`Send + 'static` executor messages. Tickets capture note/session, text generation,
feature mask, cross-note state, shared-index epoch and currency generation.
Completion rejects other lifetimes/environments. Prepared contexts can be reused
across text edits because evaluation revalidates their text/options; stale snapshot
refs receive no generation acknowledgement or active-note dependency publication.
Background indexes catch up current text before exposing names and retain newer
foreground state. Dependency export loads retain the dispatch epoch through all
publication points. Frontends own workers/receivers and request fresh work after a
rejected completion when needed.

`ScriptTicket` captures lifetime, note ID, generation, editable state, byte range
and output kind. `accept_script_result` validates them and returns an optional
`SessionEdit`; application revalidates again so delayed requests cannot overwrite
intervening edits. Replacement output is an isolated undo transaction. Message
output is validated without changing text. Frontends execute/cancel script processes
and handle status and input-mode transitions.

`RateService` lives beside sessions and survives note switches. Startup cache
loading and rate fetches use owned jobs; the host chooses their executor. The
service permits one active refresh, applies each completion once, ignores canceled
or older generations and preserves installed rates on failure. Cache publication
is fenced against cancellation/newer jobs. Identical rate values do not invalidate
calculations merely because provider/fetch dates changed. After a changed result,
the host calls session calc invalidation and shared cross-note index invalidation.

## Shared display semantics and source coordinates

`note_session::display` owns Markdown reveal rules, semantic styles, formula/table
substitution and media/source interpretation. Frontends choose actual colors,
terminal cells or pixels, wrapping, scrolling and device input. `SemanticContext`
styles lines using fence/language state and borrowed `LineDecorations`; cached keys
include text, variable identity, cursor reveal state and decoration ranges. Unchanged
lines reuse semantic token/style state; local edits do not require a per-frame
whole-document parse.

`SourceDisplayMap` and `MappedLineBuilder` use Unicode scalar columns. Provenance
separates copied source, generated text owned by a source span, and unowned ghosts.
Hidden spans retain zero-width entries. Caret boundary affinity controls ambiguous
edges; clicking generated text returns its owner rather than inventing an editable
position. Compose maps through formula/table substitution and marker hiding before
mapping selections or clicks. Helpers convert scalar columns to/from UTF-8 bytes
and UTF-16 units; the frontend remains responsible for cell/pixel geometry and IME
composition lifetimes. Table display width/layout may remain frontend-specific
while the source ownership map is shared.
