# Slate Plan

## Direction (2026-09)

Slate is a terminal-only application. The Tauri GUI, CodeMirror front end, wasm bridge, and Node build toolchain were removed; there is no compatibility layer for them.

- `crates/tui` (package `slate`, binary `slate`): terminal app, CLI entry, host side effects.
- `crates/editor-core`: editing semantics.
- `crates/app-core`: storage, note sources, calc engine, config.

Keeping semantics in the core crates remains a goal: it keeps behavior testable without a terminal and leaves room for another front end later (for example a ratatui buffer rendered in a native window).

### Next steps (ordered)

1. **Ratatui + crossterm port of `crates/tui`.** Done: crossterm input, ratatui terminal/diff, renderer paints buffer cells, soft wrap. Remaining: move overlays to ratatui widgets where it simplifies code, per-line render caching (frame composition is ~4 ms at 200x60), sticky goal column for screen-row motions.
2. **Features** (candidates, see `roadmap/features.md`):
   - templates beyond the daily note (`:template meeting`)
   - tags, backlinks panel, ghost notes
   - live query blocks (TODO aggregation, saved searches over FTS)
   - runnable code blocks with captured output
   - charts/sparklines from table data
   - inline images via `ratatui-image` (kitty/sixel/half-blocks)
   - fuzzy switcher via `nucleo`
   - per-note history/diff view
3. **Cleanup:** move pure command execution (`crates/tui/src/editor_core/commands.rs`) into `editor-core`; replace the Node perf scripts with a Rust or shell runner.

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

Syntax: `![alt](./assets/image.png)`. Import copies files into note-scoped assets and inserts a relative markdown link; `app-core` owns path normalization, size/format limits, and traversal protection.

- [ ] Image token/match helpers in `editor-core` with tokenizer tests
- [ ] Path sanitization and traversal protection tests
- [ ] Terminal placeholder rendering outside the caret, raw markdown inside the caret
- [ ] Open-image-at-cursor action (external viewer)
- [ ] Inline rendering via `ratatui-image` (kitty/sixel/half-blocks), viewport-only and cached
- [ ] Perf checks for notes with many image references
