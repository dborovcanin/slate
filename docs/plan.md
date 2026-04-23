# Slate Plan

## Product Intent

Slate is a notes application with:

1. A powerful math and calculations engine, including variables and autocomplete.
2. Fast, lightweight, and highly usable table manipulation inspired by Excel workflows.
3. Markdown-style text editing as the primary content model.
4. Clean architecture boundaries where UI focuses on editing and decoration/rendering, and can be replaced with limited core churn.
5. A shared editor core with vim-like behavior and separate rendering engines for UI and terminal.
6. Performance-first modularity: UI/TUI are decoupled as much as possible, while preserving separate hot paths when performance justifies it.
7. Extensible command surfaces (vim-like command model) for fast, scriptable editing workflows.

## Architecture Principles

- Shared-core first for editing semantics.
- Thin frontends (UI/TUI as adapters + rendering layers).
- Deterministic editor behavior across frontends unless divergence is explicitly intentional.
- Performance and responsiveness before convenience abstractions.
- Incremental computation over full-document recomputation.
- Small, reviewable, subsystem-scoped changes.

## Target Architecture

### Layering

- `editor-core` (Rust): canonical editing semantics, vim intent resolution, markdown/table/list transforms, folding index semantics, calc/variable semantics, command planning.
- UI adapter (Tauri + CodeMirror): key/input mapping, rendering, decorations, viewport scheduling, UI-only UX.
- TUI adapter (terminal): key/input mapping, terminal rendering, terminal-only UX.
- Backend/runtime layer: persistence, reminders/notifications, clipboard watchers, note security IO, module persistence.

### Ownership Rules

- If behavior changes document text, cursor/selection semantics, vim semantics, calc semantics, or fold semantics, it should default to shared core ownership.
- UI/TUI may keep local implementation only for:
  - rendering and viewport concerns,
  - host/runtime side effects,
  - proven hot paths where wasm/native boundary crossing regresses UX.

### Hot Path Policy

- Do not force per-keystroke wasm roundtrips for insert-mode/UI hot paths.
- Shared semantics should be batched where possible.
- Keep fast local paths when measured latency or responsiveness would otherwise regress.

## Consolidated Subsystem Migration Plan

### Commands/Vim

Current state:

- Vim step parsing/intents and command planning are shared-core owned.
- A substantial execution subset is shared for line/word/text-object actions.
- UI and TUI both consume shared execution where migrated.
- Non-migrated action semantics still exist in frontend adapters.
- Parity replay is significantly expanded but not complete for all editing semantics.

Action points:

1. Continue migrating non-hot-path `VimIntent -> EditOperation + register/selection delta` execution into shared core.
2. Keep high-frequency UI movement/insert interactions local when wasm boundary cost is measurable.
3. Expand parity corpus for visual/linewise variants, command-bar interactions, and corner-count semantics.
4. Remove unsupported-intent panic paths from parity simulation by covering or explicitly skipping with rationale.
5. Keep one canonical intent mapping source and avoid frontend numeric coupling.

Acceptance criteria:

- Same semantic result across UI/TUI for migrated intents.
- No measurable UI regression on normal typing/navigation hot paths.

### Markdown/Table/List

Current state:

- Core markdown/table/list transforms are mostly shared in `editor-core`.
- UI consumes batched wasm transactions.
- TUI consumes shared rules natively.
- Some trigger/scheduling heuristics and cursor clamping remain frontend-local.

Action points:

1. Define and document canonical trigger points for markdown/table/list rules.
2. Migrate remaining semantics that affect document correctness (not rendering cadence) into shared core.
3. Add cross-frontend parity fixtures for table edits, list rewrites, and markdown autoformat transitions.
4. Keep frontend-local only the pieces tied to viewport/event cadence.

Acceptance criteria:

- Rule output parity for shared scenarios.
- No full-document recomputation introduced into typing hot paths.

### Folding

Current state:

- Fold structure/index and incremental remap decisions are shared-core oriented.
- Rendering/state presentation remains frontend-local by design.
- Cross-frontend fold parity corpus is still missing.

Action points:

1. Add fold transition parity fixtures (insert/delete near boundaries, heading-level interactions, fence transitions).
2. Enforce shared fold index decisions as canonical for both adapters.
3. Keep fold visualization and viewport expansion behavior frontend-specific.

Acceptance criteria:

- Equivalent fold ranges and transition decisions for shared scenarios.
- No fold-state drift between UI and TUI for identical edits.

### Calc/Variables

Current state:

- Trigger/eval-scope/trailer-refresh semantics are largely shared.
- Scheduling, async lifecycle, and ghost/widget rendering remain frontend-local.
- Parity coverage across adapters is still partial.

Action points:

1. Add cross-frontend parity scenarios for assignment lines, formula cells, trailer refresh, and variable dependencies.
2. Keep async scheduling and UI widget rendering local, but centralize correctness semantics in core.
3. Tighten invalidation/remap behavior for large-note edits to avoid stale calc state.

Acceptance criteria:

- Equivalent semantic outputs for shared calc scenarios.
- Large-note responsiveness remains stable during incremental recalculation.

## Cross-Subsystem Architecture Action Plan (Ordered)

1. Define explicit migration boundary policy for the remaining semantics:

- what must move to shared core,
- what remains adapter-local for performance or rendering reasons,
- and why.

1. Expand parity from vim key replay to command/rule/folding/calc semantic parity suites.

2. Introduce a live GUI parity runner (in addition to simulator parity) for end-to-end confidence against real CodeMirror integration.

3. Close remaining frontend semantic drift in non-hot paths subsystem-by-subsystem.

4. Add architecture guardrails in CI:

- parity suites,
- startup/hot-path performance checks,
- protections against reintroducing duplicated semantics.

1. Decide migration feature-flag policy for remaining moves:

- either add focused migration flags,
- or explicitly document why permanent adapter-local ownership is intentional.

## Definition of Done for This Track

- Shared core is canonical for agreed semantic domains.
- UI/TUI parity suites cover vim, command/rule, folding, and calc semantics at meaningful depth.
- Hot path performance remains equal or better.
- Frontends stay adapter- and rendering-focused.
- Architecture intent is documented and enforced by tests/checks.

## Future Updates

### Global

- [ ] Ability to open markdown files directly and save (export) to them
- [ ] Global quick-capture: `slate capture "thought"` appends to today's inbox note without opening UI.
- [ ] Clipboard-watch into a named note/section.
- [ ] Pipe-in mode: `cmd | slate append`.
- [ ] Email forwarding address that ingests into inbox note.
- [ ] Templates (`:template meeting`).
- [ ] Daily note auto-create with configurable template.
- [ ] "On this day" recall view.
- [ ] Random note resurfacing.
- [ ] Recurring reminders (cron-like).
- [ ] Reminder snooze.
- [ ] Agenda digest of active reminders and open tasks.
- [ ] Tags (`#tag`) with tag index views.
- [ ] Backlinks (`[[note]]`) with mentions footer.
- [ ] Pinned notes.
- [ ] Archive mode.

### Aggregation

- TODO aggregator across all notes.
- Saved searches.
- Inline query blocks.
- Habit tracker using checklist + calc semantics.
- Time tracking commands with daily totals.

### External

- Per-note export (markdown/html/pdf).
- URL unfurl on paste.
- One-way calendar sync (ICS/Google/iCal).
- Web clipper endpoint.

### Personal Data

- Contacts as notes via `@person` references.
- Book/movie note schema with lightweight metadata.
- Journal mode with mood/energy metrics.

### Reflective

- Weekly review prompt/template.
- Writing streak tracking.

### Other

- [ ] Multicursor support.
- [ ] Context menu for formatting conversions.
- [ ] Right-click conversion to checklist/ordered/unordered list.
- [ ] Support variable assignment from formula helpers like `a := sum_column()`.
- [ ] UI settings page.
- [ ] Code folding UX improvements.
- [ ] Search notes content.
- [ ] Auto backups.
- [ ] Support multiple formulas in row/column contexts.
- [ ] Link handling polish for `[text](url)` display behavior.
- [ ] Currency conversion support with local cache and startup sync.
- [ ] Keyboard shortcut expansion and menu coverage in UI and terminal flows.
- [ ] Export command enhancements.
- [ ] Micro-optimizations for precomputed derived UI styles.
- [ ] Improve overflow to full soft-wrap.
- [ ] Improve date-vs-list parsing edge cases.

## Bugs and Fixes Backlog

- [ ] Stability and performance hardening.
- [ ] Dedup cleanup: unify text-object methods, undo/redo semantics, and vim line-range behavior.
- [ ] Calc engine enhancements and extensive testing.
- [ ] Improve note switching behavior.
- [ ] Fix pasting deleted text behavior in UI.
- [ ] Ensure banner priority over H1 where intended.
- [ ] Improve command selection UX and UI window decoration polish.
- [ ] Optimize checkbox rebuild to avoid full visible-range rebuilds.
- [ ] Checkbox UI style performance review.
- [ ] Fix TUI shortcuts in non-vim mode.
- [ ] Fix `db` behavior in TUI.
- [ ] Fix visual and visual-line large-count motions (for example `10k`, `10j`) while preserving selection.
- [ ] Cross-platform notification handling: test and improve the flow.
- [ ] Folding behavior improvements.
- [ ] Fix terminal auto-reordering checklist cursor behavior.
- [ ] Fix TUI border padding during floating mode and resize.
- [ ] Fix UI cut selection behavior; align terminal cut line behavior (`xx`).
- [ ] Fix UI cursor movement after `Esc` navigation flows.
- [ ] Add assignment-trailer evaluation support (`val := a - b = 44` style reconciliation on tab).
- [ ] Revisit insert-mode persistence after command execution where expected.
- [ ] Table movement bugfixes.
- [ ] Architecture assessment.
- [ ] Fix UI cursor sometimes showing as block in insert mode.
- [ ] Tab replaces value in table cell with calculated value.
- [ ] Consider removing `:sum`, `:avg` commands (**probably not**).
- [ ] Fix Markdown decoration around `**` before and after `,`, `(`, or `{...`, not only whitespace.
- [ ] Fix memory leak.
- [ ] 3-time password block.
- [ ] Enrich table with `(1,2)` access for better experience.
- [ ] Improve overflow handling.
- [ ] Improve theming and overall visual polish in UI and TUI.
- [ ] Memory consumption and micro-optimizations.
- [ ] Add `Ctrl+Q` UI exit.
- [ ] Improve exports.
- [ ] Do we need a hard stop on modules (note size) since we can manually control it?
- [ ] Fix modules in TUI to apply actual changes.
- [ ] Do not follow cursor for a checkbox that is moved to the bottom because it was checked.
- [ ] Improve encrypted notes (per-note passphrase, locked from search until unlock).
- [ ] Vault mode for hidden tagged notes.
