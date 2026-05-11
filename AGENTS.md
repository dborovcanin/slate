# Slate / Notes App Agent Instructions

Read this file before making changes. For non-trivial work, also read `roadmap/plan.md`.

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

1. Read `roadmap/plan.md`
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

# [slate] recent context, 2026-05-11 9:37pm GMT+2

Legend: 🎯session 🔴bugfix 🟣feature 🔄refactor ✅change 🔵discovery ⚖️decision 🚨security_alert 🔐security_note
Format: ID TIME TYPE TITLE
Fetch details: get_observations([IDs]) | Search: mem-search skill

Stats: 50 obs (21,072t read) | 838,394t work | 97% savings

### Apr 30, 2026
S171 Wiki-link tokenizer passes all 5 tests and full editor-core test suite (158 tests) passes (Apr 30, 12:01 AM)
S174 Implement Obsidian-style wiki-links in Slate — broken link rendering, variable-like styling, and [[ autoclose before picker — starting with editor-core tokenizer (Apr 30, 12:09 AM)
S192 inline_tokens_to_js() uses generic serde serialization — WikiLink* variants auto-serialize via kebab-case (Apr 30, 12:10 AM)
S210 Architecture and performance review of the Slate project — code only, ignoring markdown and tests (Apr 30, 12:23 AM)
### May 5, 2026
S227 Table cursor navigation improvements — implement Obsidian-like behavior for arrow keys in/out of tables with vim insert mode, including cross-cell and table boundary navigation (May 5, 12:19 PM)
241 5:33p 🔵 Table widget uses Decoration.replace + sourceModeLine to toggle between widget and raw-edit modes
242 5:34p 🔴 Rich table UI — stripped conflicting TUI cursor keymap and tableCursorGuards when richTableUi=true
243 " 🔴 Editor CSS blur fix — text-rendering changed from optimizeLegibility to auto on cm-content
245 6:22p ⚖️ Table cursor navigation UX — simplified arrow key behavior confirmed
247 6:25p 🔵 Table cursor navigation code structure traced in markdown-editing.ts
248 6:26p 🟣 Rich table arrow-key entry implemented — direct cell focus without whole-table selection
249 10:39p ⚖️ Table cursor navigation UX — Obsidian-style arrow key behavior in UI
250 " 🔵 Table cursor navigation code structure fully traced in markdown-editing.ts
251 10:40p 🔵 Rich table widget textarea keydown already implements Obsidian-style arrow wrapping
252 " 🔵 moveOutsideTable places CodeMirror cursor at line before/after table block, then calls view.focus()
253 " 🔵 richTableUi is true whenever markdown decorations are enabled; Vim mode detected via view.dom.dataset.vimMode
254 10:41p 🔵 Table widget replaces entire block with Decoration.replace — source mode toggled via StateField
255 " 🔵 tableCursorGuards only runs in non-rich mode; Vim mode set via CSS classes and data-vimMode on view.dom
S228 Implement Obsidian-like table cursor navigation for markdown tables in UI and TUI modes, with arrow keys behaving like normal text movement but constrained by table structure (May 5, 10:46 PM)
256 10:53p 🔄 Add isTableDelimiterLine utility for table structure detection
257 " 🟣 Refactor table arrow key navigation for Obsidian-like cursor behavior
258 10:54p 🟣 Add rich table entry from adjacent prose and fix last-cell selection
259 " ✅ Bind arrow keys to rich table entry in tableCursorKeymap
260 10:55p ✅ Table cursor navigation implementation validated and tested
S229 Table cursor navigation overhaul — Obsidian-like behavior for UI editor with cross-cell arrow wrapping, shift+arrow cell selection, and scroll/artifact fixes (May 5, 10:55 PM)
261 11:12p ⚖️ Table cursor navigation UX — Obsidian-style arrow key behavior for UI
262 11:13p 🔵 Table cell navigation code structure traced in markdown-decoration.ts
263 " 🔵 Table widget DOM structure, blur commit, and cell selection mechanics traced
264 11:18p 🔵 Table widget DOM construction — buildCellInput call sites and header vs body cell distinction
265 11:19p 🔴 moveOutsideTable now scrolls cursor into view after table exit
266 " 🟣 buildCellInput gains onShiftArrow callback parameter for Shift+arrow cell range selection
S230 Obsidian-style table cursor navigation in rich table UI — all secondary fixes completed, tests passing (May 5, 11:23 PM)
267 11:35p ⚖️ Table cursor navigation UX — Obsidian-style arrow key behavior confirmed
268 11:36p 🔵 Table cell update and logical row architecture traced in markdown-decoration.ts
269 11:37p 🔵 Full table arrow navigation implementation traced in markdown-editing.ts
270 " ✅ TABLE_CELL_EVENT annotation constant added to applyTableBlockRows in markdown-decoration.ts
271 " 🟣 TABLE_CELL_EVENT userEvent tag wired into applyTableBlockRows dispatch
S231 Table cursor navigation UX overhaul — Obsidian-style arrow key behavior with character-level movement and cell/table boundary wrapping (May 5, 11:41 PM)
272 11:59p ⚖️ Table cursor UX redesign — Obsidian-style arrow navigation
273 " 🔵 Table cursor dispatch locations traced in markdown-editing.ts and markdown-decoration.ts
### May 6, 2026
274 12:04a 🔵 Table widget cursor navigation code structure traced in markdown-decoration.ts
275 12:05a 🔵 Table cursor navigation split between rich UI mode and source mode with distinct implementations
276 " 🔵 Rich table UI widget includes multi-cell selection, drag-reorder, and formula evaluation display
277 12:11a 🔵 TableWidgetModel interface and multi-line cell continuation row format traced
278 " 🔵 tableCursorGuards ViewPlugin clamps source-mode cursor to valid cell content positions
279 12:12a 🔵 richTableUi flag controls plugin set — tableCursorGuards disabled in rich mode, tablePipeInputHandler added
S232 Table UI cursor behavior bugs — comprehensive code review to identify and fix issues with cursor position, entering/exiting table, and Obsidian-like arrow key navigation (May 6, 12:12 AM)
280 12:19a 🔴 Table cell update cursor jump — pin selection outside replaced range
281 " 🔴 Table cell commit reentrancy — prevent double-dispatch on blur
### May 11, 2026
282 1:10p ✅ Roadmap Updated to Reflect Recent Updates and Performance Tweaks
283 " 🔵 Slate Project Structure and Roadmap Coverage Confirmed
284 1:11p 🔵 Calc Decoration Architecture: Dual Result Fields, Formula Cache, and Commit Marks
285 " 🟣 TableFormatCache Gains Hit/Miss Instrumentation for Parsed Row Cache
286 " ✅ Standalone table-perf Binary Deleted
287 1:12p 🔵 Slate Project Architecture: Tauri App with Shared Rust Core
288 " 🟣 New table-perf Binary: Comprehensive Multi-Scenario Table Performance Benchmark
290 " 🔵 Table Implementation: UI is Pure WASM Delegation, TUI Has Thread-Local Cache Layer
291 " 🔵 Table Core: Multi-Level Caching for Parse, Format, and Logical Row Structure
292 " 🔵 TUI Table Cell Cache Uses Linear Scan on Cache Miss Path
289 1:13p ✅ serde_json Promoted from dev-dependencies to dependencies in src-tauri

Access 838k tokens of past work via get_observations([IDs]) or mem-search skill.
</claude-mem-context>

