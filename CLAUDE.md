# Project guidance

Read `roadmap/plan.md` before substantial work.

## What this project is

A terminal note-taking app with:
- a terminal UI (`crates/tui`, binary `slate`)
- a shared editing core (`crates/editor-core`)
- an app/persistence core (`crates/app-core`)
- shared markdown table structure (`crates/table-syntax`), used by both cores
- vim-like editing behavior
- markdown-style structured editing
- Linux as the current focus

There is no GUI. The Tauri/CodeMirror front end was removed on the `rework` branch.

## What matters most

Prioritize:
1. architecture
2. responsiveness
3. performance
4. new, useful features (over UI polish)
5. Linux-first quality
6. portable design

## Architectural rule

Keep the terminal layer thin.

`crates/tui` is a presentation/input layer.
Core editing semantics live in `editor-core`:
- document edits
- motions
- text objects
- selections
- markdown-like structure handling
- tables/lists/code-block logic
- calculation behavior
- undo/redo semantics

Persistence, calc evaluation, config, and note sources live in `app-core`.
Keeping semantics out of the terminal layer keeps them testable and leaves room for another front end later.

## How to approach changes

For non-trivial tasks:
- identify the owning layer first
- keep changes small
- prefer incremental updates over full recomputation
- avoid broad refactors unless necessary
- prefer tested libraries (e.g. ratatui/crossterm) over custom terminal plumbing

## Important constraints

- This is a vim-like editor, not just a notes textbox.
- Structured editing behavior must stay correct.
- Large notes must remain fast.
- Hot paths (key dispatch, render) must stay cheap: no whole-document work per keystroke.

## When responding

Before coding, briefly state:
- goal
- affected layer(s)
- likely risk areas

After coding, summarize:
- what changed
- whether core/terminal separation improved or was preserved
- whether performance may still need follow-up
