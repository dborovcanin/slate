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

# [slate] recent context, 2026-05-13 11:53am GMT+2

Legend: 🎯session 🔴bugfix 🟣feature 🔄refactor ✅change 🔵discovery ⚖️decision 🚨security_alert 🔐security_note
Format: ID TIME TYPE TITLE
Fetch details: get_observations([IDs]) | Search: mem-search skill

Stats: 50 obs (17,318t read) | 703,815t work | 98% savings

### May 6, 2026
S233 Review last 40 commits and explore potential improvements in the slate project (May 6, 12:12 AM)
### May 12, 2026
S234 Evaluate items 1, 4, and 6 from a task list; user challenged item 1 (macros), leading to code investigation and plan revision (May 12, 8:52 AM)
S235 Review last 40 commits and implement improvements — accurate per-character PDF glyph widths and FxHashMap migration (May 12, 8:56 AM)
S236 Investigate and fix pre-existing test failure, then plan export.rs refactor into three files (May 12, 9:10 AM)
S237 Code analysis and refactor planning for slate: vim text-objects, export.rs split, and decide_eval_window API consolidation (May 12, 12:02 PM)
S238 Refactor PDF unicode font visibility (pub(super)→pub(crate)) and plan consolidation of the 6-variant decide_eval_window API into a single params-struct function (May 12, 12:04 PM)
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
392 " ✅ Full slate test suite passes after PDF font visibility refactor
393 " 🔵 decide_eval_window function family in editor-core calc_plan.rs
394 12:44p 🔵 decide_eval_window callers across the Slate codebase
395 " 🔵 note-startup.rs benchmarks cached vs uncached eval window performance
396 " 🔵 Terminal editor eval window pipeline: sync index then decide window with cached index
397 12:45p 🔵 WASM eval window API uses decide_eval_window_with_mask without dependency index caching
398 12:46p 🔵 calc_plan.rs flag-computation helpers are already pub
S239 Fix Rust test compilation failures in the `slate` project caused by production API refactor not reflected in tests (May 12, 12:46 PM)
399 12:48p 🔵 decide_eval_window used in calc_plan.rs internal tests — additional call sites to update
400 " 🔄 Replaced 6 decide_eval_window variants with DecideEvalWindowParams struct + single function
401 1:43p 🔵 Test Suite Compilation Failures in editor-core and slate Crates
402 1:44p 🔵 Detailed Compiler Errors: decide_eval_window Signature Mismatch and Missing Function
403 " 🔵 Old Test API: decide_eval_window_with_mask Called with 8 Arguments Including CalcFeatureMask
404 " 🔴 Re-added decide_eval_window_with_mask Convenience Wrapper to Fix Test Compilation
405 " 🔴 Updated 5 Test Call Sites from Old decide_eval_window Signature to decide_eval_window_with_mask
406 1:45p 🔴 Made calc_feature_mask pub(super) to Fix E0624 Visibility Error in Tests
S240 Complete a 6-item refactor/improvement plan for the `slate` Rust project — all items now finished (May 12, 1:45 PM)
S241 Dead code cleanup in `calc_plan.rs` — remove `formula_dependency_window` thin wrapper, verify clean build (May 12, 1:47 PM)
### May 13, 2026
439 10:05a 🔵 Variable Color Inconsistency Between UI and TUI in Default Dark Theme
440 " 🔵 Root Cause: UI `.md-variable` Uses `--code-token-type` Instead of a Dedicated Variable Color
441 10:06a 🔵 Full Architecture Map: Why UI Variable Color Is Greenish and TUI Is Golden
442 " 🔴 Added `--code-token-variable` CSS Property to UI Theme Engine, Synced with TUI Palette
443 10:07a 🔴 Fixed `.md-variable` in `editor.css` to Use New `--code-token-variable` with Fallback
444 " 🔵 `theme.css` Default `:root` Block Has No `--code-token-variable`; Dynamic Derivation Is Sole Source
445 " 🔴 Added `--code-token-variable: #d7af5f` Static Default to `theme.css`
446 " 🔴 Variable Color UI/TUI Consistency Fix Verified: TypeScript Compiles Clean, Tests Pass
447 10:09a 🔵 `src/app.ts` `buildPdfExportPalette` Still Reads `--code-token-type` for `variable` Field — Not Yet Updated
448 10:10a 🔵 Variable Color Fix Changes Committed — Working Tree Clean
S255 Add right-boundary cursor reveal for regular markdown [text](url) links — same behavior already supported for wiki-links [[...]] (May 13, 10:11 AM)
449 10:13a 🟣 Right-boundary cursor reveal extended to all link types (UI + TUI)
450 10:22a 🔵 Primary session in idempotent patch loop due to stale chunk cache
451 " 🔴 Image right-boundary test used wrong cursor position (14 vs 15)

Access 704k tokens of past work via get_observations([IDs]) or mem-search skill.
</claude-mem-context>

