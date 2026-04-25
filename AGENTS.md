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

# [slate] recent context, 2026-04-25 12:54pm GMT+2

Legend: 🎯session 🔴bugfix 🟣feature 🔄refactor ✅change 🔵discovery ⚖️decision 🚨security_alert 🔐security_note
Format: ID TIME TYPE TITLE
Fetch details: get_observations([IDs]) | Search: mem-search skill

Stats: 48 obs (16,771t read) | 337,886t work | 95% savings

### Apr 25, 2026
1 12:13a 🔵 Slate Project Architecture: TUI/UI Decoupled Hybrid
S2 Assembly LOC estimate for a full-featured TUI editor (curiosity/hypothetical) (Apr 25, 12:13 AM)
S1 Slate Project Architecture: TUI/UI Decoupled Hybrid (Apr 25, 12:13 AM)
S5 ASM LOC estimate for TUI editor + render path SIMD optimization discussion (Apr 25, 12:15 AM)
S6 Apply "Slate" visual theme to existing TUI/web editor project — warm paper aesthetic, semantic token system, floating window shell, configurable typography (Apr 25, 12:23 AM)
2 12:35a 🔵 Slate TUI Editor project structure and existing theme system surveyed
S18 Add Slate theme family (slate, slate-dark, slate-light) with floating window shell layout and semantic CSS token system (Apr 25, 12:37 AM)
4 12:46a ⚖️ Default color scheme changed from gruvbox-light to slate
5 " 🟣 Slate floating window shell layout added to app.css
6 " 🟣 Slate semantic CSS token fallbacks added to global theme.css
7 12:47a 🔄 UI components migrated to Slate semantic token variables with fallbacks
8 " 🔄 Slate semantic token migration extended to dialogs, date picker, and switcher
9 12:49a 🔵 Slate project test and build script structure discovered
S20 Add Slate theme family (slate, slate-dark, slate-light) with floating window shell layout and semantic CSS token system — implementation complete, tests passing (Apr 25, 12:49 AM)
S21 Font token migration complete — switcher input updated, tests pass (Apr 25, 12:50 AM)
10 12:50a 🔵 Presets test suite passes after Slate theme additions
12 " 🔵 Vite build succeeds after Slate theme and CSS changes
13 1:00a 🟣 Google Fonts integration added to Slate TUI editor
14 " 🟣 Slate theme font tokens wired to Google Fonts with system fallbacks
15 1:01a 🟣 Slate status bar typography split — display font for title, mono for meta
16 " 🟣 Font token migration complete — switcher input updated, tests pass
S25 Fix "I see no difference" — Slate theme fonts not rendering due to missing Literata and Geist (Apr 25, 1:01 AM)
17 1:05a 🔵 Slate Tauri config has no CSP or font settings
18 1:07a 🔵 Slate font availability: JetBrains Mono present, Literata and Geist absent
19 1:08a 🔵 Chrome User-Agent required to get woff2 font URLs from Google Fonts
20 1:09a 🔵 Geist woff2 files are shared across weights 400/500/600
21 " 🔵 Literata latin woff2 files are also shared across weights 400/500/600
22 1:11a 🟣 Geist and Literata woff2 fonts bundled into Slate project
23 " 🟣 Self-hosted fonts.css created with local woff2 @font-face declarations
S27 Font names used in slate (3).html reference design — identifying display, body, and mono font choices (Apr 25, 1:12 AM)
24 1:19a 🔵 Font config setting has no effect — theme font not applied from config
S28 Font config setting has no effect — theme font not applied from config (Apr 25, 1:19 AM)
25 1:21a 🔵 Slate ThemeConfig defined in crates/app-core, not src-tauri
26 10:50a 🔵 Woff2 URLs obtained for Fraunces, Source Serif 4, and EB Garamond fonts
27 11:25a 🔵 Font not rendering — Fraunces woff2 files missing from public/fonts/
28 " 🔵 Slate config module is a thin re-export of app_core::config
29 11:26a 🔵 Slate theme loading pipeline traced from bootstrap to DOM application
30 " 🔵 Editor has no inline font-family or CodeMirror theme — fonts applied purely via CSS
31 " 🔵 Editor font driven by --display-font CSS variable, not data-font attribute directly
32 11:27a 🔵 Slate theme CSS uses data-theme attribute, not data-font — font variable binding unclear
33 " 🔵 Font override bug root cause — config uses "literata" key but fontMap expects "serif"
34 11:28a 🔵 Self-hosted fonts confirmed present in public/fonts directory
35 11:29a 🔴 Fix Slate theme font not rendering — editor.css font-family restructure
36 " 🔵 editor.css diff reveals --display-font only applied to headings, not body text
37 11:30a 🔵 editor.css font-family uses --font-mono, not --display-font, for body text
38 11:34a 🔵 Root cause: editor body text hardcoded to --font-mono, not --display-font
39 " 🔵 Backend DEFAULT_COLOR_SCHEME is "gruvbox-light", not "slate"
40 11:35a 🟣 Slate theme floating window layout implemented in app.css
41 11:36a 🟣 Slate theme editor typography overrides added to editor.css
42 " ✅ Production build succeeds after Slate CSS layout and typography changes
43 " ✅ All 157 TypeScript tests pass after Slate CSS changes
44 11:46a 🔵 Slate HTML theming architecture — applyState() and CSS variable pipeline
45 " 🟣 Added slate and slate-dark color schemes with custom accent picker to Slate (3).html
46 11:47a 🔵 Slate config.toml has display_font field and slate/slate-dark schemes already active
47 11:48a 🔄 Config refactor: removed variables_enabled field and flattened variable_autocomplete_min_chars into [editor]
48 11:49a 🔵 apply_patch fails on second hunk after write_file already applied the first hunk
49 " 🔄 Completed variables_enabled removal from TUI and TypeScript API layer
50 " ✅ Updated module-commands tests to remove legacy variables_enabled config override

Access 338k tokens of past work via get_observations([IDs]) or mem-search skill.
</claude-mem-context>