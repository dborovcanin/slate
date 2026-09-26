# Slate Plan

## Direction (2026-09)

Slate is a terminal-only application. The Tauri GUI, CodeMirror front end, wasm bridge, and Node build toolchain were removed; there is no compatibility layer for them.

- `crates/tui` (package `slate`, binary `slate`): terminal app, CLI entry, host side effects.
- `crates/editor-core`: editing semantics.
- `crates/app-core`: storage, note sources, calc engine, config.

Keeping semantics in the core crates remains a goal: it keeps behavior testable without a terminal and leaves room for another front end later (for example a ratatui buffer rendered in a native window).

### Positioning

Slate is a **computational notebook for the terminal**: a fast scratchpad where notes calculate. Its distinctive pieces are live inline calculation with units, variables shared across notes, spreadsheet-style table formulas, styled-in-place markdown editing (no split preview), and instant capture.

Full knowledge-base apps (for example ZenNotes TUI: vault of Markdown files, panes and tabs, kanban tasks, CSV databases, preview pane, MCP) cover organisation breadth. Slate should not chase that feature list. New work should either strengthen the computational notebook or remove friction from everyday editing.

Already done on this track: ratatui + crossterm port, buffer rendering, soft wrap with screen-row motions, dialog polish, command-line cursor editing, daily notes and `slate capture`.

### Next steps (ordered)

1. **Editing papercuts** - done: hand-typed tables keep their cells, variable autocomplete falls back to the last word on prose lines, titles drop heading markers (existing notes pick up the clean title on their next save).
2. **Selection statistics** - while a visual selection or table cells are selected, show `sum`, `avg`, `count` of the numbers in the status bar (spreadsheet-style). Reuse the `:sum` / `:avg` scope logic in `editor-core`.
3. **Segmented status bar** - mode pill, note title, dirty mark, module chips, calc/selection result, working collection; transient messages ("autosaved ...") become short-lived toasts instead of overwriting the status line.
4. **Discoverability**
   - Which-key popup after a prefix (`g`, `z`, leader) listing the possible next keys, drawn with the dialog frame.
   - Command menu shows the catalog descriptions next to each command, with fuzzy matching.
   - `?` help overlay (keys and commands) and `:themes` picker with live preview.
5. **Inline images (sixel, kitty, iTerm2)** - see "Image Support" below.
6. **Charts from tables** - a fenced `chart` block (`chart bar col=B`, `chart spark col=Total`) renders a bar chart or sparkline from a table in the same note, using ratatui chart widgets; the block shows its source when the cursor is inside it.
7. **Computing query blocks** - a fenced `query` block that computes over notes, e.g. `sum(expense) where #food month:this` or open TODOs by tag. Builds on FTS and the cross-note variable index; results render as ghost rows and refresh off the input path.
8. **Dates, money, time**
   - Date arithmetic: `next friday + 3 days`, `deadline - today`.
   - Currency conversion with a local rate cache refreshed in the background.
   - Time tracking: `09:10-11:45` ranges summed per day.
9. **`slate calc "..."`** - one-shot evaluation from the shell (and `cmd | slate calc`), with access to exported note variables.
10. **Dependency view** - for the value under the cursor, list the lines and notes that use it and the values it depends on, so cross-note calculations are easy to trust.
11. **Outline and backlinks panel** - toggleable side panel (heading outline + notes linking here) using ratatui layout.
12. **Mouse support** - click to place the cursor, wheel scroll, click rows in lists (crossterm already reports mouse events).

Also on the list:
- **Performance:** scrolling into a new region spends about 3.8 ms in `ensure_calc_for_viewport`; move that evaluation off the draw path (show stale ghosts, refresh when ready). Frame painting itself is about 0.3 ms at 200x60, so render caching is not needed.
- **Cleanup:** move pure command execution (`crates/tui/src/editor_core/commands.rs`) into `editor-core`; replace the Node perf scripts with a Rust or shell runner; sticky goal column for screen-row motions.
- **Other candidates** (see `roadmap/features.md`): templates beyond the daily note, tags and ghost notes, runnable code blocks with captured output, fuzzy switcher via `nucleo`, per-note history and diff view.

## Product Intent

Slate is a terminal notes application with:

1. A powerful math and calculations engine, including variables and autocomplete.
2. Fast, lightweight, and highly usable table manipulation inspired by Excel workflows.
3. Markdown-style text editing as the primary content model.
4. Clean architecture boundaries where the terminal layer focuses on input and rendering, and can be replaced with limited core churn.
5. A shared editor core with vim-like behavior.
6. Performance-first modularity: core crates own semantics; the terminal layer keeps separate hot paths only when performance justifies it.
7. Extensible command surfaces (vim-like command model) for fast, scriptable editing workflows.

## Architecture Principles

- Core first for editing semantics.
- Thin terminal layer (input adapter + rendering).
- Prefer tested libraries (ratatui, crossterm) over custom terminal plumbing.
- Performance and responsiveness before convenience abstractions.
- Incremental computation over full-document recomputation.
- Small, reviewable, subsystem-scoped changes.

## Target Architecture

### Layering

- `editor-core`: canonical editing semantics, vim intent resolution, markdown/table/list transforms, folding, calc/variable planning, command planning.
- `app-core`: persistence, note sources, calc evaluation, cross-note index, config.
- `tui`: key/input mapping, terminal rendering, terminal UX, host side effects (export, backup, IMAP, notifications, clipboard).

### Ownership Rules

- If behavior changes document text, cursor/selection semantics, vim semantics, calc semantics, or fold semantics, it defaults to core ownership.
- The terminal layer may keep local implementation only for:
  - rendering and viewport concerns,
  - host/runtime side effects,
  - proven hot paths where measured latency would otherwise regress.

### Hot Path Policy

- Key dispatch and frame rendering must not do whole-document work.
- Render from cached derived state (tokens, table layout, calc results).
- Keep fast local paths when measured latency or responsiveness would otherwise regress.

## Subsystems

### Commands/Vim

- Vim step parsing/intents and command planning are core-owned.
- A substantial execution subset is core-owned for line/word/text-object actions; the rest still lives in `crates/tui/src/terminal/app/vim_actions.rs`.

Action points:

1. Move remaining `VimIntent -> EditOperation + register/selection delta` execution into `editor-core`.
2. Expand the replay corpus (`crates/tui/src/terminal/tests/golden/vim_replay.json`) for visual/linewise variants, command-bar interactions, and counts.

### Markdown/Table/List

- Markdown/table/list transforms live in `editor-core`; the terminal applies them natively.
- Table word-delete and continuation-row cleanup still live in `crates/tui/src/terminal/app/editing.rs`.

Action points:

1. Move remaining document-correctness semantics into `editor-core`.
2. Keep terminal-local only what is tied to viewport/event cadence.

### Folding

- Fold ranges are computed in `editor-core` (`folding::build_fold_ranges`); collapse state and rendering are terminal-local.

Action points:

1. Add fold transition fixtures (insert/delete near boundaries, heading-level interactions, fence transitions).

### Calc/Variables

- Trigger/eval-window/trailer-refresh planning is core-owned; evaluation lives in `app-core`.
- Scheduling and ghost rendering are terminal-local.

Action points:

1. Move TUI cross-note preload fully off the input/render loop, with visible pending/stale state.
2. Tighten invalidation/remap behavior for large-note edits to avoid stale calc state.

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

## Definition of Done

- Core crates are canonical for agreed semantic domains.
- Hot path performance remains equal or better.
- The terminal layer stays adapter- and rendering-focused.
- Behavior is covered by core tests or terminal replay fixtures.

## Performance Backlog

- [ ] Adaptive large-note mode (follow-up to the 30,000-line full-feature cutoff): viewport-first calc, lazy fold/indexing, bounded caches, memory-bounded undo spans.
- [ ] Large-note regression gates: `30k/100k/200k/400k` fixtures for edit latency and memory, p95 targets for keypress/render/save, fail perf checks on sustained regressions.

Measurement references: `roadmap/performance.md`, `roadmap/perf-multirow-table.md`.

## Architecture and Hardening Backlog

Merged from the former `todo.md` (verified against the code on 2026-09-26; done and GUI-only items dropped). Impact and difficulty are 1-10.

| Action | Status | Description | Impact | Difficulty |
| --- | --- | --- | ---: | ---: |
| Move remaining table delete semantics into `editor-core` | Partial | Boundary edits, structural merges, header deletion, and cursor movement are shared. Table word-delete and continuation-row cleanup (`prune_empty_table_continuation_row_at_cursor` and friends in `crates/tui/src/terminal/app/editing.rs`) are still terminal-local. | 10 | 8 |
| Consolidate pure command execution | Backlog | Move inline format, list conversion, format clear, and date insertion execution from `crates/tui/src/editor_core/commands.rs` into `editor-core`; keep only side effects in the terminal layer. | 9 | 7 |
| Command catalog conformance check | Backlog | Test that every `CommandId` in `command_catalog.rs` has a core executor, a terminal host handler, or an explicit unsupported state. | 9 | 5 |
| Versioned SQLite migrations | Postponed | Only `migrations/0001_init.sql` exists. Introduce `PRAGMA user_version` (or a `schema_migrations` table), ordered migrations, migration tests, and backup guidance for schema changes. | 10 | 6 |
| Backup restore validation | Backlog | Run `PRAGMA integrity_check` and verify the expected schema on a staged backup before restore; guard restore around open notes and background work. | 8 | 5 |
| Zeroize sensitive memory | Backlog | Zeroize passwords and derived encryption keys after use; avoid cloning key material. | 8 | 6 |
| Move cross-note preload off the event loop | Backlog | The terminal still waits (`wait_timeout_while` in `editing.rs`) for cross-note dependency evaluation; evaluate asynchronously and show pending/stale state instead. | 7 | 6 |
| Reduce undo memory spikes | Partial | Span-based history paths exist; extend them so remaining large-note edits avoid full line-vector snapshots. | 8 | 7 |
| Performance budgets in CI | Partial | CI runs the startup and table perf checks. Add budgets for frame render, calc delta evaluation, save, and large-note opening. | 8 | 6 |
| Large-note degradation | Backlog | See "Performance Backlog": replace the all-or-nothing 30,000-line cutoff with per-feature budgets and visible degraded-state indicators. | 8 | 7 |
| Unambiguous wiki-link resolution | Backlog | Resolve exact ids, aliases, or ask to disambiguate instead of taking the most recent note matching an 8-char prefix. | 6 | 5 |
| File-note trust boundary | Partial | Asset access is constrained in `app-core`; the terminal should make clear that a file-backed note is an external file saved directly to its path (e.g. a title-bar badge). | 7 | 4 |
| Terminal-layer semantics guardrail | Backlog | A check that flags new table/list/vim semantic helpers added under `crates/tui` unless allowlisted. | 7 | 5 |
| Host capability contract | Backlog | A small typed contract for host capabilities used by command planning: save, quit, export, backup, note security, collections, reminders, clipboard. | 6 | 4 |

## Bugs and Fixes Backlog

- [ ] Allow multiple rows in table cell
- [ ] Dedup cleanup: unify text-object methods, undo/redo semantics, and vim line-range behavior
- [ ] Calc engine enhancements and extensive testing
- [ ] Improve note switching behavior
- [ ] Ensure banner priority over H1 where intended
- [ ] Fix shortcuts in non-vim mode
- [ ] Fix `db` behavior
- [ ] Fix visual and visual-line large-count motions (for example `10k`, `10j`) while preserving selection
- [ ] Cross-platform notification handling: test and improve the flow
- [ ] Folding behavior improvements
- [ ] Fix auto-reordering checklist cursor behavior
- [ ] Fix border padding during floating mode and resize
- [ ] Align cut line behavior (`xx`)
- [ ] Add assignment-trailer evaluation support (`val := a - b = 44` style reconciliation on tab)
- [ ] Revisit insert-mode persistence after command execution where expected
- [ ] Table movement bugfixes
- [ ] Tab replaces value in table cell with calculated value
- [ ] Fix Markdown decoration around `**` before and after `,`, `(`, or `{...`, not only whitespace
- [ ] Fix memory leak
- [ ] 3-time password block
- [ ] Improve overflow handling
- [ ] Improve exports
- [ ] Fix modules to apply actual changes at runtime
- [ ] Do not follow cursor for a checkbox that is moved to the bottom because it was checked
- [ ] Improve encrypted notes (per-note passphrase, locked from search until unlock)
- [ ] Vault mode for hidden tagged notes

## Note Search Pass 2

Context: content search uses SQLite FTS5 in `app-core`, indexes only `access_mode = 'none'` notes, and powers the switcher.

Goal: make search robust and scalable while keeping semantics in `app-core` and the terminal as a thin adapter.

### Phase 2.1: Search contract and index lifecycle

- Shared request/response contract: query, limit, paging cursor, filters -> note id, title, snippet, rank, updated_at.
- Keep trigger-based incremental indexing; expose `rebuild_note_search_index` as a command; bounded startup consistency check.
- Bounded limit and query term count; optional `optimize` hook.

### Phase 2.2: Search quality

- Phrase-safe token normalization, quoted phrases, intentional prefix behavior, defined empty/short query behavior.
- `bm25` ranking with recency tie-break and optional title boost.
- Snippets derived only from indexed plaintext notes.

### Phase 2.3: Performance and safety

- Benchmarks for 1k/10k+ notes; p50/p95 query latency tracked in perf checks.
- Protected notes stay excluded; transitions to protected modes remove rows immediately; regression tests for leaks via results/snippets.

### Checklist

- [ ] Define and document the `search_notes` API contract in app-core.
- [ ] Fix search dialog typing issues.
- [ ] Expose index rebuild as a command.
- [ ] Phrase/prefix query parser and token normalization.
- [ ] Snippet extraction in the response type.
- [ ] Render snippets and ranking metadata in the terminal.
- [ ] Lifecycle tests for all note mutation/protection transitions.
- [ ] Search latency benchmarks and targets.

## Wiki Links

Syntax: `[[shortid]]`, `[[shortid#heading]]`, `[[shortid|title]]`, `[[shortid#heading|title]]`. Short id is the first 8 characters of the note id; display title and heading are cosmetic, resolution always goes through the id. Broken links render distinctly.

- `editor-core` (`markdown_tokens.rs`): `WikiLink*` tokens, purely syntactic.
- `app-core`: short-id resolution and heading lookup.
- Terminal: `[[` auto-close + autocomplete, `Ctrl+]` / `gd` navigation, `K` preview, valid/broken styling.

Open: heading scroll after navigation; unambiguous resolution when short-id prefixes collide.

## Image Support

Syntax: `![alt](./assets/image.png)`. Import copies files into note-scoped assets and inserts a relative markdown link; `app-core` owns path normalization, size/format limits, and traversal protection. Today the terminal collapses image links to a `[image: alt]` placeholder when the cursor is outside them.

### Inline rendering (sixel, kitty, iTerm2)

Goal: show images inline in terminals that support a graphics protocol, and keep the text placeholder everywhere else.

- **Protocol detection:** query the terminal once at startup (after entering raw mode) for sixel support (device attributes), the kitty graphics protocol, and iTerm2; honour a `[terminal] images = "auto" | "sixel" | "kitty" | "iterm2" | "off"` config override. Inside tmux, use passthrough when enabled, otherwise fall back to the placeholder.
- **Library:** use `ratatui-image` (sixel/kitty/iTerm2/half-block backends) if its release for ratatui 0.30 is usable; otherwise encode sixel ourselves with a small encoder crate and write it through the crossterm backend.
- **Layout:** an image line reserves N rows below it (N from image aspect ratio and a `max_rows` cap), counted by the wrap layout and the scroll fit-up like continuation rows, so cursor placement and scrolling stay exact. The markdown line stays editable; the cursor never enters the image rows.
- **Rendering:** images are drawn after the ratatui diff flush (graphics are not cells), only for images fully inside the viewport. When the frame changes under an image, clear and redraw it; skip redraws when the image rect and scroll position are unchanged.
- **Caching:** decode and resize off the input path into a per-path cache keyed by (path, mtime, cell size); show the placeholder until the image is ready.
- **Fallbacks:** unsupported terminal, missing file, or `images = "off"` keeps the current `[image: alt]` placeholder. Remote URLs are not fetched.

### Checklist

- [ ] Image token/match helpers in `editor-core` with tokenizer tests
- [ ] Path sanitization and traversal protection tests
- [x] Terminal placeholder rendering outside the caret, raw markdown inside the caret
- [ ] Protocol detection + `[terminal] images` config
- [ ] Reserved image rows in the wrap layout and scroll fit-up
- [ ] Sixel rendering (then kitty and iTerm2) after the frame flush, viewport-only
- [ ] Background decode/resize cache
- [ ] Open-image-at-cursor action (external viewer)
- [ ] Perf checks for notes with many image references
