# Features Roadmap

Planned and candidate features across all tracks. Implementation backlog and architecture work lives in `roadmap/plan.md`.

---

## UX and feature priorities

Candidate priorities from the September 2026 UX review:

1. **Calculation inspection** — make expressions, substituted values and dependencies explainable, with jumps to definitions.
2. **Navigation history** — move back and forward through notes, searches, links and variable definitions without losing context.
3. **Quick-capture destination** — extend the existing capture workflow with a configurable inbox note or section.

Keep expensive indexing asynchronous and panels driven by cached state. Text-changing behavior belongs in the shared core. Make loading states and large-note feature reductions visible rather than silently changing behavior.

## Interaction and feedback

- [ ] **High: Save and calc status** — quiet indicators for unsaved, saving, saved, save conflict and calculations pending; preserve actionable errors without distracting notifications.
- [ ] **High: Context-sensitive help** — a keyboard-accessible popup of actions available at the cursor, including table operations, link navigation and calculation actions.
- [ ] **High: Navigation history** — back/forward across note opens, searches, links and variable definitions; preserve each note's cursor, scroll position and folds.
- [ ] **Medium: Informative undo** — describe the undone action (for example, table row deletion or formatting) and briefly highlight the restored area.
- [ ] **Medium: Search context** — show match count and position, preview destinations, and restore the original cursor and viewport when search is cancelled.
- [ ] **Large-note feedback** — distinguish features still loading from features unavailable at the current note size.

## Capture and daily workflow

- [x] Global quick-capture: `slate capture "thought"` or piped stdin appends to today's daily note without opening the editor.
- [ ] Configurable quick-capture destination: append to an inbox note or named section without interrupting the current editing session.
- [ ] Clipboard-watch into a named note/section.
- [ ] Email forwarding address that ingests into inbox note.
- [ ] Templates (`:template meeting`) — reusable project, meeting, budget and calculation notes with a few prompted fields.
- [x] Daily notes — `slate today` / `:today` create today's note from a configurable template. Still open: optional Google Calendar / ICS integration pulls the day's events into the scaffold as a starting structure.
- [ ] "On this day" recall view.
- [ ] Random note resurfacing.
- [ ] MCP for AI tools

## Organization

- [ ] Tags (`#tag`) with tag index views.
- [ ] Backlinks (`[[note]]`) with mentions footer and an optional keyboard-driven outline/backlinks panel for heading navigation and incoming references.
- [ ] Pinned notes.
- [ ] Archive mode — read-only, special search.
- [ ] Workspaces and workspace encryption.

## Reminders and agenda

- [ ] Recurring reminders (cron-like).
- [ ] Reminder snooze.
- [ ] Agenda digest of active reminders and open tasks.

## Aggregation

- [ ] TODO aggregator across all notes — show unchecked tasks and jump to the original checkbox; keep note text as the source of truth rather than adding a separate task database.
- [ ] Saved searches.
- [ ] Inline query blocks.
- [ ] Habit tracker using checklist + calc semantics.
- [ ] Time tracking commands with daily totals.

## Research and knowledge

- [ ] **Ghost Notes** — `[[wiki-links]]` targeting non-existent notes create ghost entries that accumulate a reference pressure score. A dedicated view surfaces them ranked by pressure, making knowledge gaps explicit. Promoted to real notes on first write.
- [ ] **Peripheral Context** — while writing, Slate passively monitors the current paragraph and surfaces relevant excerpts from other notes in a slim, non-blocking side column. Appears automatically via the same link layer as Ghost Notes; dismissed with a single keypress.
- [ ] **Reasoning Timeline** *(requires AI integration)* — each note accumulates a semantic changelog: section-level snapshots recording how the meaning of a passage shifted over time, not just its characters. Navigable section-by-section with vim motions. Requires an LLM (external API or local model such as Ollama); opt-in.

## Personal data

- [ ] Contacts as notes via `@person` references.
- [ ] Book/movie note schema with lightweight metadata.
- [ ] Journal mode with mood/energy metrics.

## Reflective

- [ ] Weekly review prompt/template.
- [ ] Writing streak tracking.

## External integrations

- [ ] URL unfurl on paste.
- [ ] Web clipper endpoint.
- [ ] Per-note export enhancements (markdown/html/pdf).

## Extensibility

- [ ] WASM plugin system — design in `roadmap/wasm-plugin-system.md`. Shared host runtime, `HostPlugin` command dispatch, capability prompts, lifecycle hooks, conformance suites.

## Variables

- [ ] **High: Calculation inspection** — open a panel at a result showing the expression, substituted values, units and dependencies; jump directly to a variable's definition.
- [ ] **High: Calc error navigation** — next/previous error commands with explanations for unknown variables, circular dependencies and missing linked notes.
- [ ] **Calculation scenarios** — temporarily override selected variables, such as price, tax or exchange rate, and compare results without modifying the original definitions.
- [ ] **Persistent variable name and dependency index** — store `note_variable_exports(note_id, variable_name)` and `note_cross_refs(from_note_id, to_short_id)` in the DB, populated by cheap text scans (`scan_variable_assignments`, `scan_cross_note_refs`) on note save. Load both into `CrossNoteVarIndex` on startup to warm name and dep maps without any `CalcEngine` work. Fixes cold-start autocomplete lag for cross-note variables. Values stay computed lazily in-memory as now — persisting values would require invalidation logic across concurrent edits.

## Editor

- [ ] **Table range operations** — rectangular selection, TSV paste from spreadsheets, fill-down and applying formulas across columns. Start with rectangular paste and fill-down, preserving coherent undo.
- [x] **Local note history** — versions per editing session stored as reverse line deltas; `:history`, `H` in the browser, `Ctrl+R` in the switcher; preview and restore a whole version. Still open: restore a single paragraph or table instead of the entire note.
- [ ] Multicursor support.
- [ ] Support variable assignment from formula helpers like `a := sum_column()`.
- [ ] Code folding UX improvements.
- [ ] Add highlight.
- [ ] Improve search style and content search.
- [ ] Trie search.
- [ ] Keyboard shortcut expansion and menu coverage.
- [x] Improve overflow to full soft-wrap.
- [ ] GD to go to link, header, tag.
- [ ] Add `dt{char}`.
- [ ] Consolidate commands autocompletion.
- [ ] Link handling polish for `[text](url)` display behavior.
- [ ] Currency conversion support with local cache and startup sync.
- [ ] Store format version in note.
- [ ] Improve date-vs-list parsing edge cases.
- [ ] Support multiple formulas in row/column contexts.
- [ ] Auto backups.
- [ ] Export command enhancements.
