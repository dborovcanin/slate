# Project identity

This project is a multiplatform note-taking editor with:
- Tauri UI
- terminal/TUI mode
- shared vim-like editing core
- markdown-style structured editing
- Linux-first development focus

Primary product goals:
- maintain a strong shared architecture
- support large notes efficiently
- keep UI and terminal fast and lightweight
- preserve consistent core editing semantics across front ends
- remain portable across platforms without sacrificing simplicity

Non-goals:
- becoming a bloated workspace app
- duplicating core behavior in each front end
- prioritizing feature breadth over responsiveness and architecture

## Architecture-first refactor plan (ordered by architecture quality and correctness)

### Target ownership
- Shared Rust core owns editing semantics: vim intent resolution, markdown/table/list transforms, folding model, calc/variable semantics, and undo/redo intent boundaries.
- Tauri UI and terminal/TUI own input translation and rendering only.
- Storage/config/notifications stay in backend command layer.

### Ordered action points
1. Define a canonical Rust `EditorEngine` contract before moving logic.
   - Input: document state + selection + mode + intent/command.
   - Output: deterministic delta (text edits, cursor/selection, fold/calc/variable semantic updates, status/diagnostics).
2. Freeze current behavior with golden replay tests.
   - Cover vim motions/actions, markdown/table edits, folding, calc/variables, undo/redo.
3. Add a cross-frontend parity harness.
   - Run the same replay corpus against GUI adapter and TUI adapter.
   - Require identical resulting doc, selection, and semantic state.
4. Consolidate command/motion semantics into shared core first.
   - Migrate high-risk drift areas from UI/TUI adapters into Rust core.
5. Replace fragmented wasm helper usage with batched transaction calls.
   - Keep hot path in-process; reduce boundary crossings per edit.
6. Move folding to an incremental shared core index.
   - Keep frontend fold rendering local, but make fold range computation/state transitions core-owned.
7. Normalize calc/variable behavior ownership.
   - Keep rendering local; move trigger/range/commit semantics to shared core.
8. Refactor TUI into adapter shape.
   - Pipeline: input -> intent -> engine -> terminal render.
9. Refactor Tauri UI into adapter shape.
   - Keep CodeMirror visual mechanics local; semantics come from shared core.
10. Enforce CI gates for architecture and correctness.
   - Parity suite required.
   - Startup and hot-path perf checks required.
   - No new duplicated editing semantics in frontends.
11. Migrate subsystem-by-subsystem behind feature flags, not big-bang.
   - Suggested order: commands/vim -> markdown/table/list -> folding -> calc semantics.

### Progress snapshot (2026-04-23)
- [x] 1. Canonical `EditorEngine` contract introduced in shared Rust core and re-exported for adapters.
- [x] 2. Golden replay fixtures and replay test harness added for command, module command, and vim stepping.
- [ ] 3. Cross-frontend parity harness remains next and intentionally deferred.
- [x] 4. Command/motion semantics consolidated into shared core for command resolution, note-security parsing, vim stepping, and module command planning in both Tauri UI and TUI command paths.
- [x] 5. UI markdown hot-paths now route through batched wasm markdown transactions (single boundary call per candidate sequence), reducing fragmented rule dispatch calls.
- [x] 6. Folding state transitions now consume a shared-core incremental fold index API (line-edit mapping + rebuild decision in Rust), while rendering remains frontend-owned.
- [x] 7. Calc/variable trigger, eval-scope, and trailer-refresh eligibility semantics are now shared-core decisions consumed by both UI (wasm bridge) and TUI (native core calls), while UI/TUI keep rendering and scheduling mechanics local.
- [x] 8. TUI input now flows through a dedicated adapter pipeline (terminal key -> vim intent translation -> shared-core `EditorEngine::step_vim` -> terminal action application), removing duplicated frontend stepping paths.
- [x] 9. Tauri UI now routes Vim key handling through a dedicated adapter pipeline (DOM key event -> vim key intent translation -> shared-core wasm vim step -> CodeMirror action application), while keeping CodeMirror rendering mechanics local.

### Definition of done for this refactor track
- Shared core is the canonical source for editing behavior.
- GUI and TUI parity checks pass for replay corpus.
- Large-note responsiveness is not regressed.
- Frontends remain thin adapters with no semantic drift.

## Features to be added after refactoring include:
### Global features
- [ ] Global quick-capture — slate capture "thought" CLI flag that appends to today's inbox note without opening the UI. Append-only target = no lock contention with a  running TUI.
- [ ] Clipboard-watch into a named note (you have watch — extend to route captures to a specific note/section)
- [ ] Pipe-in — cmd | slate append so terminal output flows into notes. Natural for aTUI.
- [ ] Email forwarding address — SMTP receiver that drops mail into inbox note
- [ ] Ability to open markdown files
### Time & recall                                             
- [ ] Daily note auto-create with a configurable template (date, weather, TODO rollover)
- [ ] "On this day" — show notes/lines dated N years ago today
- [ ] Random note — serendipity, surfaces old knowledge
- [ ] Recurring reminders — you have reminders; add cron-style recurrence
- [ ] Snooze — punt a reminder forward by a keystroke                                 
- [ ] Agenda digest — dail summary of active reminders, dues, open tasks

### Structure (without turning it into Obsidian)                                      
- Tags (#tag) — first-class with a tag-index page listing occurrences
- Backlinks ([[note]]) — wiki-style references, auto-resolved, with a "mentions" footer per note
- [ ] Templates — :template meeting inserts a named template
- [ ] Pinned notes — always at top of switcher
- [ ] Archive — moves notes out of active set without deleting
                                                                                    
### Aggregation (plays well with your calc mindset)                                                                                                                   
- [ ] TODO aggregator — virtual note listing all - [ ] across all notes, clickable to jump
- [ ] Saved searches — name a query, run it with :q name
- [ ] Inline query blocks — ```query tag:book ``` renders a live list inside a note
- [ ] Habit tracker — checklist lines with streak counts computed inline (fits your calc engine)
- [ ] Time tracking — :track start / :track stop logs sessions to a note; daily totals available

### External                                                            
- [ ] Export per note — markdown/html/pdf via one command
- [ ] URL unfurl on paste — fetch title/description, inline as [title](url)
- [ ] Calendar sync (one-way) — reminders appear in Google/iCal via ICS feed          
- [ ] Web clipper — bookmarklet POSTs to a local HTTP endpoint slate exposes          

### Privacy
- [ ] Encrypted notes — per-note passphrase, stored encrypted in DB; hidden from search unless unlocked
- [ ] Vault mode — hide tagged notes from switcher/search by default

### Personal data
- [ ] Contacts as notes — @person references, person-page auto-aggregates mentions    
- [ ] Book/movie notes — lightweight structured frontmatter (author:, rating:) with a library view
- [ ] Journal mode — simple mood/energy number; calc engine already supports charting these

### Reflective
- [ ] Weekly review — scheduled prompt (Sunday evening) that opens a review template pre-filled with the week's notes/todos
- [ ] Writing streak — words/day stats, gentle streak indicator

### Other
- [ ] *Multicursor support*
- [ ] Context menu - choose simple formatting and options to convert
- [ ] Select - right click for context menu - convert to checklist, ordered list, unordered list
- [ ] Allow assigning variables to a := sum_column() kind of values so later it can be reused.
- [ ] UI settings page
- [ ] Code folding
- [ ] Search notes content
- [ ] Auto backups
- [ ] Add support for multiple formulas in a row/column
- [ ] Automatic link handling in the form of [link](link) with optional [link] text update. Show only [link] by default.
- [ ] Add support for currency conversions (store conversion rate locally, sync on startup in the background)
- [ ] Allow multiple formulas per row in tables
- [ ] Create shortcuts for everything + add menues to the UI, and also enable all of that in command version
- [ ] Add export command
- [ ] Micro-optimize further by precomputing all derived UI styles once (instead of recomputing small bits each draw).
- [ ] Improve overflow to full soft-wrap
- [ ] Improve date vs list handling

## Bugs and Fixes

- [ ] **Stability and performance**
- [ ] Fix dedup ghost:
5. Dedup: Unify the text-object methods, undo/redo, and VimIntent line-range      
- [ ] **Calc engine enhancements and testing**
- [ ] Improve swithcing between notes
- [ ] Pasting deleted text in UI
- [ ] Banner as the top priority over H1
- [ ] Beautify command selection and add some nice animation in UI mode, also window decoration in the UI
- [ ] Optimize checkbox rebuild - now it rebuilds the whole visible range
- [ ] Checkbox UI style performance check
- [ ] Fix TUI shortcuts in non-vim
- [ ] Fix "db" command in TUI
- [ ] Visual and v-line modes do not accept 10k 10j commands keeping selection
- [ ] Notification handling (cross platform)
- [ ] Improve folding
- [ ] Fix terminal handling of auto-reordering checklist (cursor should not follow the list)
- [ ] Fix padding between window border and TUI (check in floating mode and resize)
- [ ] Fix UI cutting selection (cutting char works), terminal xx for cut line,
- [ ] Fix ui cursor movement when hit esc after moving around
- [ ] Add option to evaluate expression at the end: e.g. val := value1 - value2 = 44 (remove ghost = 44 and add it tot the result on tab)
- [ ] Always stay in insert mode after command
