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

# [slate] recent context, 2026-05-05 8:01pm GMT+2

Legend: 🎯session 🔴bugfix 🟣feature 🔄refactor ✅change 🔵discovery ⚖️decision 🚨security_alert 🔐security_note
Format: ID TIME TYPE TITLE
Fetch details: get_observations([IDs]) | Search: mem-search skill

Stats: 50 obs (18,861t read) | 859,147t work | 98% savings

### Apr 28, 2026
S94 Change table formula cell prefix from bare = to := to align with variable assignment syntax, update docs (Apr 28, 3:52 PM)
S108 Create slate.desktop XDG desktop entry file for the Slate application (Apr 28, 3:52 PM)
S109 Calc engine and tables architecture review — identifying optimizations for performance without sacrificing code quality (Apr 28, 10:44 PM)
### Apr 29, 2026
S160 Design discussion for Obsidian-style wiki links [[notename#heading]] and [[notename^block]] in notes editor (Apr 29, 9:29 AM)
S161 Note title vs ID storage — how Slate handles title storage, uniqueness, and duplicate title resolution for wiki-style links (Apr 29, 11:52 PM)
183 11:56p 🔵 Slate note title is derived from body and stored as denormalized column
S163 Wiki-link syntax: short 8-char ID + mandatory autocomplete picker (Apr 29, 11:56 PM)
### Apr 30, 2026
184 12:01a ⚖️ Wiki-link syntax: short 8-char ID + mandatory autocomplete picker
S171 Wiki-link tokenizer passes all 5 tests and full editor-core test suite (158 tests) passes (Apr 30, 12:01 AM)
185 12:02a ⚖️ Wiki-link UX decisions: broken links, variable-style syntax, and autoclose behavior
186 12:03a 🔵 Slate Pass 2 implementation checklist and project architecture confirmed
188 12:04a 🟣 Wiki-link feature plan documented in docs/plan.md with full architecture spec
189 " 🔵 markdown_tokens.rs InlineTokenType enum and tokenize_inline_markdown structure mapped
190 12:05a 🔵 tokenize_inline_markdown uses sequential single-pass-per-type pattern with protect() guards
192 12:08a 🟣 Added WikiLink token types to InlineTokenType enum in markdown_tokens.rs
193 " 🟣 Extended is_inline_marker_token_kind() to recognize WikiLinkMarker tokens
194 12:09a 🟣 Implemented wiki-link parser in tokenize_inline_markdown() with validation and token emission
195 " 🟣 Added comprehensive test suite for wiki-link tokenizer with five test cases
197 " 🟣 Wiki-link tokenizer passes all 5 tests and full editor-core test suite (158 tests) passes
S174 Implement Obsidian-style wiki-links in Slate — broken link rendering, variable-like styling, and [[ autoclose before picker — starting with editor-core tokenizer (Apr 30, 12:09 AM)
S192 inline_tokens_to_js() uses generic serde serialization — WikiLink* variants auto-serialize via kebab-case (Apr 30, 12:10 AM)
198 12:10a 🔵 app-core sqlite.rs public API mapped — get_note_meta and NoteSummary confirmed as insertion point for resolve_wiki_link
200 12:11a 🟣 Implemented resolve_wiki_link() in app-core sqlite.rs using prefix LIKE query
201 12:15a ⚖️ Wiki-link broken link, styling, and autoclose UX finalized
202 " 🟣 Integration tests added for resolve_wiki_link() in sqlite.rs
203 12:16a 🔴 Compile error in resolve_wiki_link test — create_note_with_defaults called with wrong arity
204 " 🔴 resolve_wiki_link test fixed — create_note_with_defaults called with correct arguments
205 " 🔵 Note IDs use ULID format — short_id slice of [..8] is valid for ULID strings
206 12:17a 🔵 Note IDs generated as Ulid::new().to_string() without lowercasing in Tauri/TUI layer
207 " 🔴 Wiki-link tokenizer ID validation changed from hex to alphanumeric to match ULID format
209 " 🔴 Wiki-link tokenizer tests updated to use real ULID short IDs instead of hex short IDs
211 12:20a 🔵 All tests pass after wiki-link fixes — 158 unit tests + 3 golden replay
212 " 🔵 wasm_markdown_find_inline_tokens already exists — exposes inline tokenizer to UI
213 " 🔵 inline_tokens_to_js defined at line 647 — must be read to check WikiLink* coverage
214 12:21a 🟣 Obsidian-style extended wiki-link syntax requested — heading and block anchors
215 12:23a 🔵 inline_tokens_to_js() uses generic serde serialization — WikiLink* variants auto-serialize via kebab-case
216 " 🔵 Tauri notes command surface mapped — no resolve_wiki_link command exists yet
217 1:39p 🟣 Wiki-link heading autocomplete in UI and TUI
218 " 🔵 extract_markdown_headings lives in editor-core, not app-core
219 1:43p 🔵 Wiki-link navigation and TUI autocomplete bugs identified
### May 5, 2026
225 12:19p 🟣 Broken image inline placeholder — [Image #n: alt] widget with undo/redo support
S210 Architecture and performance review of the Slate project — code only, ignoring markdown and tests (May 5, 12:19 PM)
229 12:20p ⚖️ Architecture review converted to four prioritized action tasks
230 12:21p 🔵 calc_plan.rs FxHashMap migration scope confirmed — 9 sites across 4 functions
231 5:09p 🟣 Table selection cancels on outside click or typing
232 5:16p 🔴 Table cell selection now cancels on outside click or any keypress
233 " ✅ Slate light theme colors shifted warmer and slightly deeper
234 " 🔴 Editor wheel scroll snaps to whole CSS pixels, removing subpixel jitter
235 5:18p 🟣 Table cell selection cancels on focus-out, typing, and outside click
236 " 🔵 markdown-decoration.ts has significant dead code accumulation causing TypeScript errors
237 5:24p 🔄 Table row controls repositioned as absolute overlay over last cell
238 5:25p ✅ Session changes committed to main branch — 5 files, 4 distinct fixes
239 5:33p ⚖️ Table cursor navigation UX — enter selects all, exit restores normal cursor
240 " 🔵 Table cursor navigation architecture traced in TUI editor (CodeMirror/markdown-editing.ts)
241 " 🔵 Table widget uses Decoration.replace + sourceModeLine to toggle between widget and raw-edit modes
242 5:34p 🔴 Rich table UI — stripped conflicting TUI cursor keymap and tableCursorGuards when richTableUi=true
243 " 🔴 Editor CSS blur fix — text-rendering changed from optimizeLegibility to auto on cm-content
245 6:22p ⚖️ Table cursor navigation UX — simplified arrow key behavior confirmed
247 6:25p 🔵 Table cursor navigation code structure traced in markdown-editing.ts
248 6:26p 🟣 Rich table arrow-key entry implemented — direct cell focus without whole-table selection

Access 859k tokens of past work via get_observations([IDs]) or mem-search skill.
</claude-mem-context>

