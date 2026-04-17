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
