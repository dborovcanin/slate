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

# [slate] recent context, 2026-05-12 12:43pm GMT+2

Legend: 🎯session 🔴bugfix 🟣feature 🔄refactor ✅change 🔵discovery ⚖️decision 🚨security_alert 🔐security_note
Format: ID TIME TYPE TITLE
Fetch details: get_observations([IDs]) | Search: mem-search skill

Stats: 50 obs (16,799t read) | 701,091t work | 98% savings

### May 5, 2026
S228 Implement Obsidian-like table cursor navigation for markdown tables in UI and TUI modes, with arrow keys behaving like normal text movement but constrained by table structure (May 5, 10:46 PM)
S229 Table cursor navigation overhaul — Obsidian-like behavior for UI editor with cross-cell arrow wrapping, shift+arrow cell selection, and scroll/artifact fixes (May 5, 10:55 PM)
S230 Obsidian-style table cursor navigation in rich table UI — all secondary fixes completed, tests passing (May 5, 11:23 PM)
S231 Table cursor navigation UX overhaul — Obsidian-style arrow key behavior with character-level movement and cell/table boundary wrapping (May 5, 11:41 PM)
### May 6, 2026
S232 Table UI cursor behavior bugs — comprehensive code review to identify and fix issues with cursor position, entering/exiting table, and Obsidian-like arrow key navigation (May 6, 12:02 AM)
S233 Review last 40 commits and explore potential improvements in the slate project (May 6, 12:12 AM)
### May 12, 2026
S234 Evaluate items 1, 4, and 6 from a task list; user challenged item 1 (macros), leading to code investigation and plan revision (May 12, 8:52 AM)
S235 Review last 40 commits and implement improvements — accurate per-character PDF glyph widths and FxHashMap migration (May 12, 8:56 AM)
S236 Investigate and fix pre-existing test failure, then plan export.rs refactor into three files (May 12, 9:10 AM)
342 11:58a 🔵 Pre-Existing Test Failure in execute_terminal_perf_on_and_status_toggle_runtime_trace_state
343 " 🔵 Failing Test: app_with_note Initializes perf_trace.enabled as True
344 11:59a 🔵 Perf Test Failure Root Cause: App Reads Live perf Config from Disk at Init
345 " 🔵 Perf Test Failure: Developer's Local Config File Has perf.enabled=true
346 " 🔵 Perf Test Failure Confirmed Pre-Existing Across All Recent Commits
347 " 🔵 WIP Changes on main Branch: AGENTS.md and vim.rs Modified
348 " 🔵 app_with_note Test Helper Defined in Parent Test Module via use super::*
349 12:00p 🔵 Terminal App Test Suite File Structure
350 " 🔵 app_with_note Helper Calls TerminalApp::new_with_startup_metrics Without Overriding perf_trace
351 " 🔴 Fixed Perf Trace Test Isolation in app_with_note Helper
352 " 🔴 Perf Trace Test Isolation Fix Verified — Test Now Passes
353 12:01p 🔴 Full Test Suite Now Green — 328 Passed, 0 Failed
354 12:03p 🔵 VimIntent Has ChangeTill, ChangeInner, ChangeAround Already Defined
355 " 🔵 Vim Change Text-Objects Fully Wired in vim.rs State Machine
356 12:04p 🔵 decide_eval_window API Family in calc_plan.rs — Incremental Calc Dependency System
357 " 🔵 decide_eval_window Callers: WASM GUI, Terminal App Editing, and Startup Benchmark
S237 Code analysis and refactor planning for slate: vim text-objects, export.rs split, and decide_eval_window API consolidation (May 12, 12:04 PM)
358 12:05p 🔵 PDF Unicode Font Loading: OnceLock Cache with SLATE_PDF_UNICODE_FONT Env Override
359 12:07p 🔵 serialize_pdf Calls resolve_unicode_pdf_font_asset Directly — Key Cross-Module Dependency
360 " 🔵 commands/mod.rs: export Module Is Always Compiled (No gui Feature Gate)
361 " 🔵 render_markdown_to_pages Signature and Internal Dependencies
362 12:08p 🔵 char_draw_width Calls resolve_unicode_pdf_font_asset — Confirms Cross-Module Dependency in Layout
363 " 🔵 Test Module in export.rs Spans All Three Planned Split Layers
364 12:10p 🔵 build_markdown_pdf Is the Orchestrator Between All Three Split Layers
365 12:11p 🔄 export.rs Split Begun: Converted to export/mod.rs Directory Module
366 12:12p 🔄 pdf_style.rs Created — First File of export.rs Split
367 12:19p 🔄 Created pdf_layout.rs — second module of the export.rs split
368 12:23p 🔵 Serializer section of mod.rs confirmed to remain in mod.rs after split
369 12:26p 🔄 PDF Export Module Split into pdf_style and pdf_layout Submodules
370 12:31p 🔵 Build Error: `slight` Binary References Deleted `export.rs`
371 " 🔵 `slight` Binary Uses Explicit `#[path]` Attribute to Include `commands/export.rs`
372 " 🔴 Fixed `slight` Binary Build: Updated `#[path]` for Refactored Export Module
373 " 🔴 Slate Build Now Succeeds After Export Module Refactor
374 " 🔵 Flagged "Unused" Imports in `export/mod.rs` Are Actually Used in Test Code Only
375 12:33p 🔵 Top-Level Imports in `export/mod.rs` Are Unused in Production Code — Tests Already Import Via Globs
376 " 🔵 Confirmed Which `pdf_layout` Imports Are Used in Production vs. Tests Only
377 12:34p 🔵 Previous Grep Had a Filtering Bug — `PDF_PAGE_WIDTH_PT` and `PDF_PAGE_HEIGHT_PT` ARE Used in Production
378 " 🔴 Cleaned Up Stale Top-Level Imports in `export/mod.rs` After Module Refactor
379 " ⚖️ Attempted Workaround for `PdfRgbColor` Unused Import Warning Using Dummy Re-export
380 12:35p 🔵 `PdfRgbColor` Is Part of the Tauri API Contract Used by the TypeScript Frontend
381 " 🔵 Three Remaining Compiler Warnings After Import Cleanup
382 " 🔵 `render_plain_wrapped_text` Is Defined but Never Called Anywhere
383 " 🔵 `render_plain_wrapped_text` Is a Thin Wrapper Around `render_styled_block` With No Callers
384 12:41p ✅ Widened visibility of PdfUnicodeFontAsset to pub(crate)
385 12:42p ✅ PdfUnicodeFontAsset fields widened to pub(crate)
387 " ✅ glyph_advances field promoted to pub(crate) on PdfUnicodeFontAsset
386 " ⚖️ Tags and Note Creation: Keep Simple and Backward Compatible
388 " ✅ PdfUnicodeFontAsset fully promoted to pub(crate) — all fields now crate-visible
389 " ✅ resolve_unicode_pdf_font_asset promoted to pub(crate) in export/mod.rs
390 " ✅ Suppressed unused import warning for PdfRgbColor re-export in export/mod.rs
391 12:43p ✅ slate crate builds cleanly with zero warnings after PDF font visibility refactor

Access 701k tokens of past work via get_observations([IDs]) or mem-search skill.
</claude-mem-context>

