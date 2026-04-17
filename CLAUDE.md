# Project guidance

Read `docs/plan.md` before substantial work.

## What this project is

A multiplatform note-taking app with:
- Tauri GUI
- terminal/TUI mode
- shared editing core
- vim-like editing behavior
- markdown-style structured editing
- Linux as the current focus

## What matters most

Prioritize:
1. architecture
2. responsiveness
3. performance
4. shared behavior across UI and TUI
5. Linux-first quality
6. portable design

## Architectural rule

Keep front ends thin.

UI and terminal should be presentation/input layers.
Core editing semantics should live in shared code where possible:
- document edits
- motions
- text objects
- selections
- markdown-like structure handling
- tables/lists/code-block logic
- calculation behavior
- undo/redo semantics

## How to approach changes

For non-trivial tasks:
- identify the owning layer first
- keep changes small
- preserve parity between front ends
- prefer incremental updates over full recomputation
- avoid broad refactors unless necessary

## Important constraints

- This is a vim-like editor, not just a notes textbox.
- Structured editing behavior must stay correct.
- Large notes must remain fast.
- Wasm size and hot-path runtime matter.
- Do not duplicate logic between UI and TUI without a strong reason.

## When responding

Before coding, briefly state:
- goal
- affected layer(s)
- likely risk areas

After coding, summarize:
- what changed
- whether shared-core integrity improved or was preserved
- whether performance or parity may still need follow-up