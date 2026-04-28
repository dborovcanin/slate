# Slate / Notes App Agent Instructions

Read this file before making changes. For non-trivial work, also read `docs/plan.md`.

## Product overview

This project is a multiplatform note-taking app with:

- a GUI app built with Tauri
- a terminal/TUI app
- a shared core for editing behavior and text logic
- vim-like editing as a core capability
- markdown-style formatting and richer structured text behaviors
- current primary focus on Linux, while keeping architecture portable across platforms

The most important property of the system is architectural integrity:
UI and terminal must share core editing behavior wherever possible.

## Core principles

1. Shared core first
   - Editing semantics, motions, text objects, formatting behavior, parsing, calculations, and document transformations should live in shared core logic.
   - Do not reimplement core editing behavior separately in TUI and UI unless there is a very strong reason - for example, performance concern or different behavior between UI and TUI.

2. Keep front ends thin
   - Tauri UI and terminal UI are presentation layers and input adapters.
   - Front ends should translate user input and render state, not own document semantics.

3. Protect performance
   - Large notes must remain responsive.
   - Complex notes (tables, code blocks, lists, variables...) should stay snappy
   - Both UI and TUI should sip memory, but always feel snappy and fast
   - Prefer incremental work over full-document recomputation.
   - Avoid unnecessary allocations, repeated parsing, or rebuilding visible regions unless required.
   - Wasm size and runtime cost matter.

4. Preserve behavioral consistency
   - Vim-like behavior should remain consistent across front ends.
   - If behavior differs between TUI and UI, treat that as a design problem unless explicitly intended.

5. Prefer small, reviewable changes
   - Make targeted edits.
   - Do not refactor broadly unless the task requires it.
   - Avoid introducing abstractions that are not clearly justified.

## Architecture expectations

The architecture should stay roughly separated into:

- presentation layer
  - Tauri UI
  - terminal/TUI

- shared editor/render core
  - document model
  - selection/cursor model
  - vim-like motions and editing actions
  - formatting interpretation
  - text object handling
  - incremental rendering / derived display state where applicable

- specialized subsystems
  - table handling
  - list handling
  - code block handling
  - calculation / formula engine
  - search / indexing support
  - persistence / note management
  - calc engine (variables and calc expressions)

When adding a feature, first decide which layer owns it.
If a change touches editing semantics, it probably belongs in shared core.

## Product priorities

Optimize for the following, in order:

1. Correct architecture
2. Fast startup and snappy interaction
3. Large-note performance
4. TUI/UI behavioral parity
5. Linux quality first
6. Cross-platform compatibility without overengineering
7. New features

Do not trade architecture and responsiveness for feature speed unless explicitly asked.

## Editing model guidance

This is a vim-like editor, not just a text box with shortcuts.

Protect the following ideas:
- motions, objects, and actions should compose cleanly
- undo/redo should remain coherent across structured edits
- structured text operations should not corrupt formatting
- tables, lists, and markdown-like constructs are first-class editing domains
- normal-mode behavior in UI should stay aligned with terminal behavior where intended

## Performance guidance

When touching rendering or core editing paths:

- prefer incremental updates to full recomputation
- avoid scanning the whole document on each keystroke
- avoid rebuilding the whole visible range for localized changes
- cache or precompute derived state when it reduces repeated hot-path work
- measure before making risky complexity increases
- keep memory use reasonable for large notes

## Scope control

The plan contains many future ideas, including quick capture, backlinks, tags, reminders, multicursor, export, encrypted notes, aggregation features, and more.
Do not pull in adjacent features unless the task explicitly requires them.

Implement the smallest complete change that satisfies the current task.

## Workflow for non-trivial tasks

1. Read `docs/plan.md`
2. Summarize the task in terms of:
   - goal
   - affected layers
   - risks to architecture/performance/parity
3. Propose a minimal implementation approach
4. Make the change
5. Run relevant tests/checks
6. Report:
   - what changed
   - what architectural decisions were made
   - whether TUI/UI parity was affected
   - any known gaps or follow-up work

## Refactoring rules

Refactor only when it improves one of:
- shared-core separation
- performance
- correctness of editing semantics
- maintainability of structured editing behavior

Avoid cosmetic refactors.

## When adding new behavior

Before coding, identify which of these it belongs to:

- shared editor core
- UI-only presentation
- terminal-only presentation
- persistence / data model
- command surface
- performance optimization
- experimental feature

State that choice explicitly in your summary.

## Guardrails

Do not:
- duplicate editor logic across UI and TUI unless necessary
- move core logic into Tauri-only code
- add heavy dependencies casually
- introduce slow full-document passes in hot paths
- weaken vim-like semantics for convenience
- break Linux workflows in the name of premature cross-platform abstractions

## Definition of done

A task is not done unless:
- the ownership of logic is in the right layer
- behavior remains coherent across front ends where expected
- performance is not worsened
- the change is scoped and reviewable
- relevant checks pass

<claude-mem-context>
# Memory Context

# [slate] recent context, 2026-04-28 4:51pm GMT+2

Legend: 🎯session 🔴bugfix 🟣feature 🔄refactor ✅change 🔵discovery ⚖️decision 🚨security_alert 🔐security_note
Format: ID TIME TYPE TITLE
Fetch details: get_observations([IDs]) | Search: mem-search skill

Stats: 50 obs (18,412t read) | 677,919t work | 97% savings

### Apr 25, 2026
S28 Font config setting has no effect — theme font not applied from config (Apr 25, 1:17 AM)
S48 Note Search Pass 2 implementation — baseline survey and design decisions before coding begins (Apr 25, 1:19 AM)
44 11:46a 🔵 Slate HTML theming architecture — applyState() and CSS variable pipeline
45 " 🟣 Added slate and slate-dark color schemes with custom accent picker to Slate (3).html
46 11:47a 🔵 Slate config.toml has display_font field and slate/slate-dark schemes already active
47 11:48a 🔄 Config refactor: removed variables_enabled field and flattened variable_autocomplete_min_chars into [editor]
48 11:49a 🔵 apply_patch fails on second hunk after write_file already applied the first hunk
49 " 🔄 Completed variables_enabled removal from TUI and TypeScript API layer
50 " ✅ Updated module-commands tests to remove legacy variables_enabled config override
51 3:51p 🟣 FTS search backend passes all 74 tests including new search tests
52 3:53p 🟣 Full test suite green after FTS search + switcher content search integration
53 3:55p ⚖️ Migration script cleanup — beta environment fully migrated
55 3:56p 🟣 Note Search Pass 2 plan documented in docs/plan.md
56 4:01p 🔵 Note Search Pass 2 plan and last commit reviewed before implementation
57 " 🔵 FTS5 search implementation baseline surveyed before Pass 2
S50 FTS5 Note Search Pass 2 — five design decisions confirmed (Apr 25, 4:02 PM)
58 4:06p ⚖️ FTS5 Note Search Pass 2 — five design decisions confirmed
S73 Fix frozen search input and slow/blocking results when searching notes in the terminal app (Apr 25, 4:06 PM)
59 4:17p 🟣 FTS5 index extended to include note_title column
60 " 🟣 NoteSearchResult struct added to models.rs
61 4:18p 🟣 DB startup sequence gains migration runner and search index health check
62 4:19p 🟣 search_notes_content rewritten to return ranked NoteSearchResult with FTS5 snippets
63 4:21p 🟣 Note Search Pass 2 — Full frontend integration with NoteSearchResult and snippet rendering
64 " 🔴 Rust E0597 lifetime error in search_notes_content — stmt dropped while borrowed
65 " ✅ Switcher CSS updated for snippet row layout
66 4:22p 🔴 Fixed E0597 lifetime error in search_notes_content by binding query_map result to local
67 " 🔵 FTS index not re-populated on unlock — search_index_restores_note_after_unlock_and_decrypt test fails
68 4:23p 🔵 unlock_note does not write to notes table — FTS trigger never fires on unlock
69 " 🔵 decrypt_note uses SQL UPDATE so FTS trigger fires correctly; unlock_note is the only broken path
70 4:24p ⚖️ Locked notes intentionally stay unsearchable after session unlock — test expectations corrected
71 " 🟣 Note Search Pass 2 Phase 2.1 — full test suite green, 82/82 passing
### Apr 26, 2026
72 1:52p 🔵 Content search freezes input — synchronous FTS5 query on every keystroke in blocking event loop
73 1:53p 🔴 Content search freeze fix — added content_search_pending flag to TerminalApp struct
74 1:54p 🔴 Content search freeze fix — deferred FTS5 query execution via maybe_autosave idle tick
75 " 🔴 Content search freeze fix — all key handlers migrated to set pending flag instead of blocking DB call
76 1:57p 🟣 Slate accent color chooser — session intent logged
S79 Fix UI/TUI blocking during content search — search must never block typing at any point (Apr 26, 2:01 PM)
78 2:06p ✅ Add Arc import to sqlite.rs for concurrent access
79 " ✅ Wrap Db fields in Arc for cloneable concurrent sharing
80 2:07p ✅ Add async search result channel to TUI app state
81 " 🔴 Implement async background search to prevent UI blocking during content search
82 " ✅ Integrate result collection into main event loop for non-blocking search completion
S84 Note search input frozen and results blocking in TUI/UI (Apr 26, 2:07 PM)
### Apr 28, 2026
84 3:22p 🔵 Note search input frozen and results blocking in TUI/UI
S85 Fix frozen input and blocking search results in TUI note search (Apr 28, 3:22 PM)
S86 Full expression support planned for table cells (Apr 28, 3:23 PM)
85 3:23p 🟣 Full expression support planned for table cells
86 3:24p 🟣 Full expression support planned for table formula cells
87 3:29p ⚖️ Full expression support planned for table cells
S92 Table formula := prefix feature fully shipped — all tests green (Apr 28, 3:29 PM)
88 3:46p ⚖️ Table formula syntax changed from = to :=
89 " 🔵 Table formula syntax parsing lives in editor-core calc_plan.rs
91 " 🔵 Existing table formula tests use = prefix — all must be migrated to :=
92 3:49p 🟣 contains_assignment_operator updated to distinguish := table prefix from variable assignment
94 " 🟣 CalcEngine updated to evaluate := prefix table formula cells
95 3:51p 🟣 New tests added for := table formula prefix in engine.rs and calc_plan.rs
96 " 🔵 Old = prefix tests still pass alongside new := prefix tests
97 3:52p ✅ docs/plan.md updated with Table Formula Expressions syntax documentation
98 " 🟣 Table formula := prefix feature fully shipped — all tests green
S94 Change table formula cell prefix from bare = to := to align with variable assignment syntax, update docs (Apr 28, 3:52 PM)
**Investigated**: - calc_plan.rs: contains_assignment_operator, find_single_calc_table_cell_range, find_table_formula_segments
    - engine.rs: table_expression_segments, multi-cell eval trigger condition, evaluate_table_formula
    - Existing tests in both files (still using old = prefix, still passing due to backward compat)
    - src-tauri/Cargo.toml test suite for TUI calc_table tests

**Learned**: - The key disambiguation: := as variable assignment always has a word char (alphanumeric/_) before it; := as table formula prefix has nothing (or only whitespace/pipe) before it. This is enforced in contains_assignment_operator.
    - builtin_formula_label() still strips a leading bare = before normalizing, so old = prefix remains functional (backward compat preserved).
    - evaluate_table_formula must strip := before passing to fend; when no builtin calls are present but had_prefix is true, the stripped expression evaluates as plain arithmetic/variable — enabling :=var and :=multiplier*2+1 style cells.
    - `:=expr` in a table cell never registers as a variable definition — confirmed by test and by parse_variable_assignment requiring a non-empty valid name before :=.

**Completed**: - calc_plan.rs: contains_assignment_operator requires word char before := (fixes false positive on formula prefix)
    - calc_plan.rs: find_single_calc_table_cell_range treats := prefix cells as formula_candidates
    - calc_plan.rs: find_table_formula_segments includes := prefix cells (labels may be empty)
    - engine.rs: table_expression_segments pushes := prefix cells into formula_segments
    - engine.rs: multi-cell eval trigger fires on := prefix cells, not just builtin calls
    - engine.rs: evaluate_table_formula strips := prefix; evaluates plain expr when no builtins present
    - New tests in engine.rs: colon_eq with builtin+arithmetic+variable, no-builtin expression, variable-only, does-not-define-variable
    - New tests in calc_plan.rs: contains_assignment_operator disambiguation, find_table_formula_segments with := and builtin, find_table_formula_segments with := and no builtin
    - docs/plan.md: new "Table Formula Expressions" section with examples and syntax rules
    - Full test suite: 483 tests passing, 0 failures (app-core + editor-core + src-tauri)

**Next Steps**: Feature is complete and verified. Potential follow-up: migrate old existing tests that still use bare = prefix (e.g. "| =avg_col() |") to := for consistency, though this is cosmetic since both syntaxes work.


Access 678k tokens of past work via get_observations([IDs]) or mem-search skill.
</claude-mem-context>