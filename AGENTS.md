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

# [slate] recent context, 2026-05-12 11:27am GMT+2

Legend: 🎯session 🔴bugfix 🟣feature 🔄refactor ✅change 🔵discovery ⚖️decision 🚨security_alert 🔐security_note
Format: ID TIME TYPE TITLE
Fetch details: get_observations([IDs]) | Search: mem-search skill

Stats: 50 obs (19,060t read) | 1,667,414t work | 99% savings

### Apr 30, 2026
S210 Architecture and performance review of the Slate project — code only, ignoring markdown and tests (Apr 30, 12:23 AM)
### May 5, 2026
S227 Table cursor navigation improvements — implement Obsidian-like behavior for arrow keys in/out of tables with vim insert mode, including cross-cell and table boundary navigation (May 5, 12:19 PM)
S228 Implement Obsidian-like table cursor navigation for markdown tables in UI and TUI modes, with arrow keys behaving like normal text movement but constrained by table structure (May 5, 10:46 PM)
S229 Table cursor navigation overhaul — Obsidian-like behavior for UI editor with cross-cell arrow wrapping, shift+arrow cell selection, and scroll/artifact fixes (May 5, 10:55 PM)
S230 Obsidian-style table cursor navigation in rich table UI — all secondary fixes completed, tests passing (May 5, 11:23 PM)
S231 Table cursor navigation UX overhaul — Obsidian-style arrow key behavior with character-level movement and cell/table boundary wrapping (May 5, 11:41 PM)
### May 6, 2026
S232 Table UI cursor behavior bugs — comprehensive code review to identify and fix issues with cursor position, entering/exiting table, and Obsidian-like arrow key navigation (May 6, 12:02 AM)
S233 Review last 40 commits and explore potential improvements in the slate project (May 6, 12:12 AM)
### May 11, 2026
285 1:11p 🟣 TableFormatCache Gains Hit/Miss Instrumentation for Parsed Row Cache
287 1:12p 🔵 Slate Project Architecture: Tauri App with Shared Rust Core
288 " 🟣 New table-perf Binary: Comprehensive Multi-Scenario Table Performance Benchmark
290 " 🔵 Table Implementation: UI is Pure WASM Delegation, TUI Has Thread-Local Cache Layer
291 " 🔵 Table Core: Multi-Level Caching for Parse, Format, and Logical Row Structure
292 " 🔵 TUI Table Cell Cache Uses Linear Scan on Cache Miss Path
289 1:13p ✅ serde_json Promoted from dev-dependencies to dependencies in src-tauri
### May 12, 2026
293 8:48a 🔵 Slate Project: Last 40 Commits Overview
294 8:49a 🔵 Slate Project Architecture: Tauri + Rust Backend + TypeScript Frontend
295 " 🔵 Slate Hotspot Files: Most Frequently Changed in Last 40 Commits
296 8:50a 🔄 Table Cell Parsing Refactored: Escaped Pipe Support and Stable Autoformat Deferral
297 " 🔄 calc_plan.rs: VariableDependencyGraph Replaces Simple Struct with Full Incremental Graph
298 " 🟣 PDF Export Engine: Custom Rust Implementation with Full UTF-8 and Table Support
299 " 🔵 Vim Mode: Full CodeMirror Integration with WASM Core and Macro/Undo Support
300 8:51a 🟣 Vim Core: Change Operator, Macros (qa/@), and C Keymap Added
301 " 🟣 Macro System Implemented in Both TUI (Rust) and UI (TypeScript) Layers
302 " 🟣 TUI Calc: Pathological Window Detection Forces Periodic Full Recomputes
303 " 🔵 PDF Export Color System: Luma-Based Normalization for Print-Safe Palette
304 " 🔵 Remaining std::collections::HashMap Usage — Potential FxHashMap Migration Opportunity
S234 Evaluate items 1, 4, and 6 from a task list; user challenged item 1 (macros), leading to code investigation and plan revision (May 12, 8:52 AM)
305 8:56a 🔵 Macros Behave Better in TUI Than in UI
306 " 🔵 Vim Macro System Lives Entirely in TUI Layer of Slate
307 8:57a 🔵 rustc_hash / FxHashMap Not Yet a Dependency in Any Slate Crate
308 " 🔵 rustc-hash Is a Transitive Dep; std::collections::HashMap Used in 4 TUI Files
309 " 🔵 FxHashMap Migration Scope: 4 Files in src-tauri, Dependency Only Missing There
310 8:58a 🔵 FxHashMap Migration Scope Wider Than Expected: app-core and export.rs Also Use HashMap
311 " 🔵 PDF Glyph Width Uses Font Factor Scaling, Not Real ttf-parser Advance Widths
312 " 🔵 PDF Export Uses Flat DW=1000 for All Glyphs; No Per-Glyph Width Array Emitted
313 8:59a 🔵 editor-core/markdown_tokens.rs Uses std::collections::HashSet
314 " 🔵 engine.rs Explicitly Uses DefaultHasher — Cannot Be Fully Swapped to FxHash
315 9:00a 🔵 command_search_switcher.rs Uses Inline std::collections::HashSet in collect()
316 9:01a ✅ Added rustc-hash = "2" to src-tauri/Cargo.toml
317 " 🟣 folding_state.rs Fully Migrated to FxHashSet; rustc-hash Added to app-core
318 " 🟣 render.rs InlineTokenCache Migrated to FxHashMap; One ::new() → ::default() Remains
319 " 🟣 render.rs and imap.rs Fully Migrated to Fx Collections
320 9:03a 🔄 Migrated calc engine HashMap usage to FxHashMap/FxHasher
321 9:04a 🔄 Replaced std::collections::HashSet with rustc_hash::FxHashSet in markdown_tokens.rs
322 " 🔵 FxHashMap lacks ::new() — must use ::default() after rustc_hash migration
323 " 🔵 Exact compilation errors after FxHashMap migration: export.rs and command_search_switcher.rs
324 " 🔄 Migrated reminder_helpers.rs imports from std HashMap/HashSet to FxHashMap/FxHashSet
325 " 🔄 Fully migrated reminder_helpers.rs to FxHashMap/FxHashSet — one HashMap::with_capacity remains
326 9:05a 🔄 Fixed all FxHashMap::new() calls in export.rs — replaced with ::default()
327 " 🔴 FxHashMap/FxHashSet migration compiles cleanly across workspace
328 9:07a 🟣 PDF Export: Accurate Per-Character Width Measurement Using AFM Metrics and Font Advances
329 " 🔴 Fixed Invalid std::collections::FxHashMap Path in calc/engine.rs Tests
330 " 🔴 PDF Export Test: FxHashMap::<String, f32>::new() Turbofish Fails at Compile Time
S235 Review last 40 commits and implement improvements — accurate per-character PDF glyph widths and FxHashMap migration (May 12, 9:10 AM)
331 9:17a 🔵 Slate Vim Mode: VimPending Enum and State Machine Architecture
332 " 🔵 Slate Tauri Commands Module Structure
333 9:18a 🔵 Vim Dispatch in vim.rs: Massive Boilerplate Pattern Identified as Refactor Target
334 " 🟣 Added ChangeTill, ChangeInner, ChangeAround Pending States to Vim Mode
335 " 🟣 Completed Change Text Object Dispatch: ciw, ca|, ctX, etc. Now Fully Wired

Access 1667k tokens of past work via get_observations([IDs]) or mem-search skill.
</claude-mem-context>

