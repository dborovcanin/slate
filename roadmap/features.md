# Features Roadmap

Planned and candidate features across all tracks. Implementation backlog and architecture work lives in `roadmap/plan.md`.

---

## Capture and daily workflow

- [ ] Global quick-capture: `slate capture "thought"` appends to today's inbox note without opening the editor.
- [ ] Clipboard-watch into a named note/section.
- [ ] Email forwarding address that ingests into inbox note.
- [ ] Templates (`:template meeting`).
- [ ] Daily notes — auto-create on open with a configurable template; optional Google Calendar / ICS integration pulls the day's events into the scaffold as a starting structure.
- [ ] "On this day" recall view.
- [ ] Random note resurfacing.

## Organization

- [ ] Tags (`#tag`) with tag index views.
- [ ] Backlinks (`[[note]]`) with mentions footer.
- [ ] Pinned notes.
- [ ] Archive mode — read-only, special search.
- [ ] Workspaces and workspace encryption.

## Reminders and agenda

- [ ] Recurring reminders (cron-like).
- [ ] Reminder snooze.
- [ ] Agenda digest of active reminders and open tasks.

## Aggregation

- [ ] TODO aggregator across all notes.
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

- [ ] **Persistent variable name and dependency index** — store `note_variable_exports(note_id, variable_name)` and `note_cross_refs(from_note_id, to_short_id)` in the DB, populated by cheap text scans (`scan_variable_assignments`, `scan_cross_note_refs`) on note save. Load both into `CrossNoteVarIndex` on startup to warm name and dep maps without any `CalcEngine` work. Fixes cold-start autocomplete lag for cross-note variables. Values stay computed lazily in-memory as now — persisting values would require invalidation logic across concurrent edits.

## Editor

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
