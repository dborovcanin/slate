# Slate / Notes App Agent Instructions

Read this file before making changes. For non-trivial work, also read `roadmap/plan.md`.

## Product overview

This project is a terminal note-taking app with:

- a terminal UI (`crates/tui`, binary `slate`)
- a shared core for editing behavior and text logic (`crates/editor-core`)
- an app core for persistence, calc, config, and note sources (`crates/app-core`)
- vim-like editing as a core capability
- markdown-style formatting and richer structured text behaviors
- current primary focus on Linux, while keeping architecture portable across platforms

There is no GUI. The Tauri/CodeMirror front end was removed on the `rework` branch.

The most important property of the system is architectural integrity:
editing semantics live in the core crates, and the terminal layer stays a thin presentation/input adapter.

## Core principles

1. Shared core first
   - Editing semantics, motions, text objects, formatting behavior, parsing, calculations, and document transformations should live in shared core logic.
   - Do not implement editing semantics in `crates/tui` unless there is a very strong reason, for example a measured performance concern.

2. Keep the terminal layer thin
   - `crates/tui` is a presentation layer and input adapter.
   - It should translate user input and render state, not own document semantics.
   - Prefer tested libraries (ratatui/crossterm) over custom terminal plumbing.

3. Protect performance
   - Large notes must remain responsive.
   - Complex notes (tables, code blocks, lists, variables...) should stay snappy
   - The app should sip memory, but always feel snappy and fast
   - Prefer incremental work over full-document recomputation.
   - Avoid unnecessary allocations, repeated parsing, or rebuilding visible regions unless required.
   - Render only from cached derived state; do not re-tokenize or re-parse per frame.

4. Preserve behavioral consistency
   - Vim-like behavior should be coherent and match vim where intended.
   - Keep behavior in the core so it can be tested without a terminal.

5. Prefer small, reviewable changes
   - Make targeted edits.
   - Do not refactor broadly unless the task requires it.
   - Avoid introducing abstractions that are not clearly justified.

## Architecture expectations

The architecture should stay roughly separated into:

- presentation layer
  - terminal UI (`crates/tui`)

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
4. New, useful features (over UI polish)
5. Linux quality first
6. Cross-platform compatibility without overengineering

Do not trade architecture and responsiveness for feature speed unless explicitly asked.

## Editing model guidance

This is a vim-like editor, not just a text box with shortcuts.

Protect the following ideas:

- motions, objects, and actions should compose cleanly
- undo/redo should remain coherent across structured edits
- structured text operations should not corrupt formatting
- tables, lists, and markdown-like constructs are first-class editing domains

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
   - risks to architecture/performance
3. Propose a minimal implementation approach
4. Make the change
5. Run relevant tests/checks
6. Report:
   - what changed
   - what architectural decisions were made
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
- terminal presentation
- persistence / data model
- command surface
- performance optimization
- experimental feature

State that choice explicitly in your summary.

## Guardrails

Do not:

- put editing semantics in the terminal layer
- hand-roll terminal plumbing a tested library already provides
- add heavy dependencies casually
- introduce slow full-document passes in hot paths
- weaken vim-like semantics for convenience
- break Linux workflows in the name of premature cross-platform abstractions

## Definition of done

A task is not done unless:

- the ownership of logic is in the right layer
- behavior is covered by core tests where it lives in the core
- performance is not worsened
- the change is scoped and reviewable
- relevant checks pass

<claude-mem-context>
# Memory Context

# [slate] recent context, 2026-05-19 9:44pm GMT+2

No previous sessions found.
</claude-mem-context>

