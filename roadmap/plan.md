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
3. [x] Add cross-frontend parity fixtures for table edits, list rewrites, and markdown autoformat transitions.
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

### Table Formula Expressions

Table cells support full arithmetic expressions using the `:=` prefix:

- `:=sum_col() + var` — builtin aggregate plus a variable
- `:=5 * sum_col() + var` — arithmetic before a function call
- `:=avg_col() * 2 - offset` — compound expression with multiple terms
- `:=var` — bare variable reference
- `:=(1,2) + (2,2)` — 1-based table cell references (data rows only)

**Syntax rules:**

- The `:=` prefix at the **start** of a cell (nothing before it) marks the cell as a formula expression. The content after `:=` is evaluated as an arithmetic expression.
- `name :=` (with a name before `:=`) remains a variable assignment, unchanged.
- Variable definitions (`name := expr`) cannot be created inside table cells; `:=expr` without a name is always a formula, never a definition.
- Builtin functions (`sum_col()`, `avg_col()`, `sum_row()`, `avg_row()`) can appear anywhere in the expression and are substituted with their computed values before evaluation.
- Variable references are resolved from the full document context.
- Cell references use `(row,col)` with 1-based indexing over table **data rows** (rows after the delimiter) and 1-based columns within the same table block.
- Reference errors return explicit markers: `!ERROR#out_of_bounds`, `!ERROR#non_numeric`, `!ERROR#self_reference`, `!ERROR#cycle`.
- Multiline table cell continuation uses canonical `|>` rows:
  - base row: `| alpha | one |`
  - continuation row: `|> beta | two |`
  - continuation rows append to the previous logical data row cell-by-cell (logical row index does not advance).

**Module gating rules:**

- `modules.math` is the master calc gate. If off, no expressions are evaluated.
- `modules.table` gates table-formula parsing/evaluation (`:=...` inside table cells).
- `modules.variables` gates variable assignment/reference behavior and autocomplete.
- Mixed-mode expectations:
  - `math=on`, `table=off`, `variables=on`: variable math works outside tables; table formulas are ignored.
  - `math=on`, `table=on`, `variables=off`: builtin table formulas still work; variable references stay unresolved.

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
- startup/hot-path performance checks (`perf:check` unified startup+table guardrail),
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

## Architecture and Performance Review Backlog (2026-04-30)

Source: deep architecture/performance pass over `editor-core`, `app-core`, TUI, and UI adapters.

### 2026-05-11 Table Performance Update (shipped)

- [x] Shared-core table parse hot path now caches per-row pipe/cell parsing in `TableFormatCache` and reuses it across table edit rules (multiline break, insert/delete column, header detection).
- [x] UI calc decoration path reduced repaint churn with deferred rebuilds, selection-only skip guards, and memoized non-cursor formula-line replacement plans.
- [x] TUI formula-row rendering now avoids repeated per-segment scans by indexing `cell_results` once per row and reusing precomputed segment char-delta prefixes for cursor/pipe translation.
- [x] Perf checks are unified under `npm run perf:check` (startup + table), with one global switch in `config.toml` (`[perf].enabled`, default `false`) and OS temp log defaults (`slate-log-ui.log`, `slate-log-tui.log`) unless overridden.

### 2026-05-12 Large-Note Threshold Policy (shipped)

- [x] Introduced a single full-feature cutoff at `30,000` lines.
- [x] Notes above the cutoff run in reduced-feature mode in UI and TUI:
  - math/calc disabled
  - markdown decorations disabled
  - folding disabled
  - undo/redo kept enabled
- [x] TUI undo/redo path now supports span-based history recording for known edit windows, reducing hot-path full-note diff work on large notes.
- [x] Added large-note mode indicators:
  - GUI status tag
  - TUI title bar label

### Next Sprint Checklist (2026-05-12 to 2026-05-23)

- [ ] Shared vim action snapshot elimination (line-window / line+col API)
  Owner: editor-core + UI adapter
  Expected impact: removes O(document size) string snapshots on intent execution and reduces UI/TUI latency spikes on large notes.
- [ ] TUI localized edit application (`apply_edit_operation` without full `join -> replace -> split` for local edits)
  Owner: TUI adapter
  Expected impact: lower per-edit allocation churn and smoother terminal typing on large files.
- [ ] Calc note-line cache mutation hardening (`sync_note_lines` path)
  Owner: calc runtime + editor-core
  Expected impact: avoids accidental full `Vec<String>` clones during eval overlap, reducing jitter during rapid edits.
- [ ] Partial calc variable-definition indexing (incremental index for changed windows)
  Owner: calc runtime + editor-core
  Expected impact: makes partial eval truly partial and cuts needless full-note scans after localized edits.
- [ ] Live GUI parity runner for markdown/calc/folding (CodeMirror-backed)
  Owner: UI adapter + test infrastructure
  Expected impact: catches simulator-vs-runtime parity drift earlier and protects cross-frontend behavior consistency.
- [ ] `TerminalApp` state decomposition (`EditorModel`, `CalcRuntime`, `OverlayState`, `RenderState`, `FoldRuntime`)
  Owner: TUI adapter
  Expected impact: smaller, safer diffs in terminal rendering/runtime changes and easier targeted performance work.
- [ ] Adaptive large-note mode (follow-up to threshold policy)
  Owner: UI + TUI adapters + editor-core
  Scope:
  - keep strict `30k` full-feature cutoff as baseline policy
  - introduce adaptive tiers above the cutoff (viewport-first calc, lazy fold/indexing, bounded decoration caches)
  - preserve undo/redo semantics with memory-bounded span deltas
  Expected impact: better UX on medium-large notes while keeping predictable resource usage.
- [ ] Large-note performance hardening and regression gates
  Owner: perf/tooling + adapters
  Scope:
  - add `30k/100k/200k/400k` fixtures for edit-latency + memory checks
  - define p95 targets for keypress/render/save and memory ceilings per tier
  - fail CI/perf checks on sustained regressions
  Expected impact: prevents silent large-note regressions and makes tuning measurable.

Sprint acceptance criteria:

- No O(document size) conversions in hot edit/action paths unless explicitly required.
- Large-note edit latency remains stable for localized edits.
- Calc delta path avoids full-cache clones during normal typing/eval overlap.
- Partial eval no longer performs full-document variable-definition scans.
- Parity failures are reproducible against real GUI runtime behavior.
- TUI runtime state changes are reviewable in subsystem-scoped diffs.

Roadmap ownership:

- `roadmap/plan.md` is the canonical execution backlog for large-note performance and feature-tier policy.
- `roadmap/performance.md`, `roadmap/perf-tracing.md`, and `roadmap/perf-multirow-table.md` are measurement/operations references and should not carry parallel execution backlogs.

## Future Updates

## WASM Plugin System Track

- [ ] Design finalized in `roadmap/wasm-plugin-system.md`.
- [ ] Implement shared host runtime and `HostPlugin` command dispatch.
- [ ] Add capability prompts, lifecycle hooks, and parity/conformance suites.

### Global

- [x] Ability to open text/code files directly and save back to source path (GUI/TUI launch via `slate <file>`)
- [ ] Global quick-capture: `slate capture "thought"` appends to today's inbox note without opening UI.
- [ ] Clipboard-watch into a named note/section.
- [ ] Pipe-in mode: `cmd | slate append`.
- [ ] Email forwarding address that ingests into inbox note.
- [ ] Templates (`:template meeting`).
- [ ] Daily note auto-create with configurable template.
- [ ] "On this day" recall view.
- [ ] Random note resurfacing.
- [ ] Add workspaces and workspace encryption.
- [ ] Recurring reminders (cron-like).
- [ ] Reminder snooze.
- [ ] Agenda digest of active reminders and open tasks.
- [ ] Tags (`#tag`) with tag index views.
- [ ] Backlinks (`[[note]]`) with mentions footer.
- [ ] Pinned notes.
- [ ] Archive mode.
- [ ] Add option for comments and math like Obsidian.
- [ ] Add highlight.
- [ ] Improve search style.
- [ ] Consolidate commands autocompletion.
- [ ] Add dt{char}.

### Aggregation

- TODO aggregator across all notes.
- Saved searches.
- Inline query blocks.
- Habit tracker using checklist + calc semantics.
- Time tracking commands with daily totals.

### External

- Per-note export (markdown/html/pdf).
- [x] PDF export renders markdown notes with support for headings/lists/code blocks, markdown tables, markdown images, theme-aware inline styling (bold/italic/variables/code tokens), and rendered checklist boxes.
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

- [ ] Multicursor support
- [ ] Context menu for formatting conversions
- [ ] Right-click conversion to checklist/ordered/unordered list
- [ ] Support variable assignment from formula helpers like `a := sum_column()`
- [ ] Trie search
- [ ] Search coloring bug
- [ ] UI settings page
- [ ] Code folding UX improvements.
- [ ] Search notes content
- [ ] Auto backups
- [ ] Support multiple formulas in row/column contexts
- [ ] Link handling polish for `[text](url)` display behavior
- [ ] Currency conversion support with local cache and startup sync
- [ ] Keyboard shortcut expansion and menu coverage in UI and terminal flows
- [ ] Export command enhancements
- [ ] Micro-optimizations for precomputed derived UI styles
- [ ] Improve overflow to full soft-wrap
- [ ] Improve date-vs-list parsing edge cases
- [ ] GD to go to link, header, tag
- [ ] Store version in note in case we decide to extend MD sometime
- [ ] Search notes by text
- [ ] Add Archive mode - read only, special search

## Bugs and Fixes Backlog

- [ ] Stability and performance hardening
- [x] Support multiline table cell content via `<br>` insertion (`Shift+Enter`) with escaped-pipe-safe table parsing (`\|`) across UI/TUI/calc paths
- [ ] Allow multiple rows in table cell
- [ ] Dedup cleanup: unify text-object methods, undo/redo semantics, and vim line-range behavior
- [ ] Calc engine enhancements and extensive testing
- [ ] Improve note switching behavior
- [ ] Fix pasting deleted text behavior in UI
- [ ] Ensure banner priority over H1 where intended
- [ ] Improve command selection UX and UI window decoration polish
- [ ] Optimize checkbox rebuild to avoid full visible-range rebuilds
- [ ] Checkbox UI style performance review
- [ ] Fix TUI shortcuts in non-vim mode
- [ ] Fix `db` behavior in TUI
- [ ] Fix visual and visual-line large-count motions (for example `10k`, `10j`) while preserving selection
- [ ] Cross-platform notification handling: test and improve the flow
- [ ] Folding behavior improvements
- [ ] Fix terminal auto-reordering checklist cursor behavior
- [ ] Fix TUI border padding during floating mode and resize
- [ ] Fix UI cut selection behavior; align terminal cut line behavior (`xx`)
- [ ] Fix UI cursor movement after `Esc` navigation flows
- [ ] Add assignment-trailer evaluation support (`val := a - b = 44` style reconciliation on tab)
- [ ] Revisit insert-mode persistence after command execution where expected
- [ ] Table movement bugfixes
- [ ] Architecture assessment
- [ ] Fix UI cursor sometimes showing as block in insert mode
- [ ] Tab replaces value in table cell with calculated value

## Note Search Pass 2 Plan (Post First-Pass FTS)

Context:

- First pass introduced operational content search using SQLite FTS5 in shared core.
- Current behavior indexes only `access_mode = 'none'` notes and powers switcher search.

Goal:

- Upgrade note search from "working" to "robust and scalable" while preserving architecture boundaries and responsiveness on large note sets.

Affected layers:

- Shared editor/runtime core (`crates/app-core`) owns indexing semantics, query parsing, ranking, and result shaping.
- UI/TUI stay as thin adapters that call shared search APIs and render results.

Risks to manage:

- Index drift when write paths evolve.
- Query syntax edge cases causing confusing "no results" behavior.
- Large-note / large-dataset latency regressions from poorly bounded search requests.
- Leaking protected-note content through snippets or stale index rows.

### Phase 2.1: Search Contract and Index Lifecycle Hardening

1. Add an explicit shared search contract:

- Request shape: query text, limit, optional paging cursor, optional filters.
- Response shape: note id, title, optional snippet, rank score, updated_at.

1. Add index maintenance guardrails:

- Keep trigger-based incremental indexing for note writes.
- Add `rebuild_note_search_index` in shared core for repair/recovery.
- Add startup health check with light validation (sampled or bounded consistency checks).

1. Add operational controls:

- Bounded limit and query term count in shared core.
- Optional maintenance hooks (`optimize`) for long-running datasets.

Acceptance criteria:

- Index stays in sync across create/save/append/delete/lock/encrypt/decrypt/ingest flows.
- Rebuild operation restores correct results after forced index reset.

### Phase 2.2: Search Quality and UX Semantics

1. Improve query semantics:

- Normalize query tokens (phrase-safe parsing, sane punctuation handling).
- Support quoted phrase matching and prefix behavior intentionally.
- Define deterministic behavior for empty/very-short queries.

1. Improve ranking:

- Keep `bm25` base ranking.
- Add deterministic recency tie-break weighting.
- Optionally boost title hits over body-only hits.

1. Add snippet generation:

- Return short matched excerpt for UI/TUI result context.
- Keep snippets derived only from indexed plaintext notes.

Acceptance criteria:

- Search results are stable and relevant for mixed short/long queries.
- UI/TUI can present match context without adding frontend-specific parsing logic.

### Phase 2.3: Parity, Performance, and Safety

1. Parity:

- Expose identical shared search behavior to both Tauri UI and terminal.
- Add parity tests around ranking order and query parsing behavior.

1. Performance:

- Add benchmark scenarios for 1k/10k+ note metadata sets and large note bodies.
- Track p50/p95 query latency and enforce bounds in CI/perf checks where feasible.

1. Security and privacy:

- Keep protected notes (`locked`/`encrypted`) excluded from persisted searchable content by default.
- Ensure transitions to protected modes remove searchable rows immediately.
- Add regression tests that verify no protected content appears in search results/snippets.

Acceptance criteria:

- No measurable regressions to note editing responsiveness.
- Search remains fast under realistic large datasets.
- Protected-note content remains non-searchable by default.

### Implementation checklist (Pass 2)

- [ ] Define and document shared `search_notes` API contract in app-core.
- [ ] Fix dialog search in TUI.
- [ ] Fix typing in search
- [ ] Add shared-core index rebuild command and command surface exposure.
- [ ] Add phrase/prefix query parser and deterministic token normalization.
- [ ] Add snippet extraction in shared core response type.
- [ ] Integrate search UI/TUI rendering with shared snippets and ranking metadata.
- [ ] Add lifecycle tests for all note mutation/protection transitions.
- [ ] Add parity tests for UI/TUI search behavior.
- [ ] Add performance benchmark fixtures and targets for search latency.
- [ ] Consider removing `:sum`, `:avg` commands (**probably not**)
- [ ] Fix Markdown decoration around `**` before and after `,`, `(`, or `{...`, not only whitespace
- [ ] Fix memory leak
- [ ] 3-time password block
- [ ] Improve overflow handling
- [ ] Improve theming and overall visual polish in UI and TUI
- [ ] Memory consumption and micro-optimizations
- [x] Add `Ctrl+Q` UI exit
- [ ] Improve exports
- [x] Add markdown-aware PDF export with table/image support plus theme-aware inline styling and rendered checkboxes
- [ ] Do we need a hard stop on modules (note size) since we can manually control it?
- [ ] Fix modules in TUI to apply actual changes
- [ ] Do not follow cursor for a checkbox that is moved to the bottom because it was checked
- [ ] Improve encrypted notes (per-note passphrase, locked from search until unlock)
- [ ] Vault mode for hidden tagged notes
- [x] Enrich table with `(1,2)` access for better experience

## Wiki Links

### Goal

Obsidian-style inter-note links: `[[shortid|Note Title]]` and `[[shortid|Note Title#Heading Text]]`.

Navigation opens the target note; heading variant scrolls to the matched heading after load.

### Stored format

```
[[a3f8b2c1|Note Title]]
[[a3f8b2c1|Note Title#Heading Text]]
```

- Short ID = first 8 chars of the note UUID (first hex segment).
- Display title and heading fragment are cosmetic — resolution always goes through the ID.
- Broken links (deleted note) render visually distinct; no silent fallback.

### Layers

**editor-core (`markdown_tokens.rs`)**

New token types: `WikiLinkMarker`, `WikiLinkId`, `WikiLinkSep`, `WikiLinkTitle`, `WikiLinkAnchor`.

Parser scans for `[[...]]`, validates structure `shortid|title` or `shortid|title#anchor`, emits tokens. Purely syntactic — no DB access. Must not conflict with `[text](url)` parser (different opening character sequence).

**app-core (`storage/sqlite.rs`)**

- `resolve_wiki_link(short_id: &str) -> Option<NoteSummary>` — `WHERE id LIKE '{short_id}%'`
- Autocomplete reuses existing note list/search query.

**Tauri commands**

- `resolve_wiki_link(short_id)` → note id + title, or not-found.
- Navigation command opens note by full ID (already exists).

**UI (`src/editor/`)**

- On `[[` typed: auto-insert `]]`, position cursor inside, open inline note picker.
- Picker filters by title as user types; on select inserts `[[shortid|Title]]` replacing the `[[]]` placeholder.
- Decoration: dim `[[`, `shortid|`, `]]`; style title as internal link. Broken link gets distinct style.
- Click handler: resolve short ID → open note; if anchor present, scroll to heading after load.

**TUI (`src-tauri/terminal/`)**

- On `[[` typed: auto-insert `]]`, open inline autocomplete (same pattern as variable autocomplete).
- Navigation keybind on wiki-link token: `gf` (go to file, vim convention).
- Broken links rendered with distinct style (e.g. strikethrough or dim-red).

### Implementation checklist

- [x] Add `WikiLink*` token types and parser to `markdown_tokens.rs`
- [x] Add `resolve_wiki_link` query to `app-core/storage/sqlite.rs`
- [x] Add `resolve_wiki_link` Tauri command
- [x] UI: `[[` auto-close + inline note picker
- [x] UI: wiki-link decoration (valid / broken)
- [x] UI: click navigation (heading scroll is follow-up)
- [x] TUI: `[[` auto-close + inline autocomplete (popup, Tab/Enter/Esc)
- [x] TUI: `Ctrl+]` navigation keybind (insert + normal mode)
- [x] TUI: wiki-link rendering (valid / broken visual styles)
- [x] Tests: tokenizer round-trips, resolver query, broken-link path

## Image Support Plan

### Goal

Add markdown image support with correct architecture boundaries:

- UI renders inline images in-editor.
- TUI preserves editability and shows stable, useful placeholders.
- Shared core owns syntax/token semantics.
- Storage/runtime owns asset path policy and import behavior.

### Supported syntax (Phase 1)

```
![alt text](./assets/image.png)
![alt text](../images/photo.jpg)
```

- No HTML `<img>` support in Phase 1.
- No resizing/caption directives in Phase 1.
- Remote URLs are optional and can be gated by config later.

### Architecture ownership

**editor-core (`markdown_tokens.rs`, wasm exports)**

- Add image markdown token coverage (`![alt](src)`) as first-class inline syntax.
- Expose structured image match/range helpers for both UI and TUI.
- Keep it pure: no filesystem IO in core.

**app-core/runtime (`crates/app-core`, tauri commands)**

- Add image import command:
  - copy selected file into note-scoped or vault-scoped assets directory,
  - return normalized markdown path to insert.
- Add path normalization and safety checks (no traversal outside allowed roots).
- Keep note body markdown-only; store image refs as text links.

**UI (`src/editor/`)**

- Render image widget/decoration for valid local image paths when cursor is outside token.
- Keep raw markdown visible/editable when cursor is inside image token.
- Add insert flow (paste/drop/select file) that calls backend import command and inserts markdown.

**TUI (`src-tauri/src/terminal/`)**

- Keep markdown text editable as-is.
- Outside-caret display mode: replace with compact placeholder, for example:
  - `[img] alt text (image.png)`
- Add open action for image at cursor (external viewer command).
- Do not attempt true terminal inline bitmap rendering in Phase 1.

### Performance constraints

- Resolve/render images only in viewport lines.
- Cache per-line parsed image ranges similarly to existing wiki-link line caches.
- UI image decode/loading must be lazy and bounded (no full-document eager loads).
- Keep large-note startup behavior stable; avoid scanning entire note for image metadata upfront.

### Safety and path policy

- Imported files are copied (not referenced in-place) by default.
- Normalize and sanitize filenames.
- Reject unsafe paths and traversal attempts.
- Preserve relative markdown links so export/move workflows remain predictable.

### Delivery phases

1. **Shared-core syntax + tests**
- Add image token/match parsing and wasm plumbing.
- Add tokenizer/match tests (valid/invalid nesting/boundary cases).

2. **Runtime import pipeline**
- Add tauri command to import/copy image and return markdown path.
- Add runtime tests for copy + path normalization + failure handling.

3. **UI rendering and insert UX**
- Add decoration/widget behavior + cursor-inside edit mode.
- Add tests for render-vs-edit transitions and insertion behavior.

4. **TUI placeholder + open action**
- Add placeholder rendering and open-at-cursor action.
- Add tests for placeholder behavior and cursor-inside raw markdown visibility.

5. **Parity/perf hardening**
- Add UI/TUI parity fixtures for shared token semantics.
- Add targeted performance checks on notes with many image links.

### Implementation checklist

- [ ] Add image token/match support in `editor-core` and wasm exports
- [x] Add tauri/runtime command for image import + markdown path return
- [ ] Add path sanitization and traversal protection tests
- [x] Add UI image decoration/widget rendering with cursor-inside raw edit mode
- [x] Add UI insert flows (paste/drop) wired to import command
- [ ] Add optional UI file-picker image import flow
- [ ] Add TUI image placeholder rendering (outside caret) and raw markdown editing (inside caret)
- [ ] Add TUI open-image-at-cursor action
- [ ] Add UI/TUI tests for image syntax parity and editing transitions
- [ ] Add perf checks for notes containing many image references
