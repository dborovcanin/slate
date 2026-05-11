# Feature Guide

This document lists major Slate capabilities and how they behave.

## Core editing

- fast plain-text editing with markdown-aware visual behavior
- undo/redo
- command bar in both editor and vim command modes
- per-note module toggles (`math`, `table`, `variables`, `style`)

## Markdown editing helpers

- heading/list/quote/code/link styling while preserving raw text editability
- list continuation on Enter
- checklist support (`- [ ]` / `- [x]`)
- optional checklist auto-reorder
- markdown table autoformat/alignment
- table-aware cursor movement and boundary editing
- wiki-link rendering that keeps raw source editable when cursor is inside the link

## Vim mode (optional)

- normal/insert/visual/visual-line workflow
- count prefixes for motions and actions
- ex commands (`:w`, `:q`, `:wq`, calc/list/module/export commands)
- yank/delete/paste workflows with clipboard integration
- fold toggle at cursor (`za`)
- wiki-link navigation with `gd`
- macros across normal + insert flows (`q<register>` start, `q` stop, `@<register>` replay, counted replay)
- macro pending cancel: `Esc` cancels pending `q`/`@` register input
- macro status summary: `Q` in normal mode shows recorded registers and step counts

## Terminal mode (optional)

- full-screen terminal app with editor + note switcher
- dedicated TUI binary: `slight`
- `slate --terminal` selector remains supported
- supports opening and editing markdown files
- command bar and module behavior aligned with GUI
- clipboard watch and command workflows
- wiki-link navigation with `Ctrl+]` and `gd`
- wiki-link autocomplete for note ids and headings

## Inline calculations

- ghost result rendering for evaluable expressions
- `Tab` accepts variable completion when active, otherwise applies calc result
- works in normal lines, lists, and table formula cells
- unit conversions and fend-compatible expressions supported

## Variables

- `name := expression` assignment syntax
- note-local scope
- case-insensitive names with normalized spacing
- reactive updates across dependent expressions
- autocomplete popup with configurable threshold

Detailed semantics: [Variables Specification](./variables.md).

## Tables

- markdown table alignment support
- per-cell formula marker: `:=...`
- row/column aggregate helpers (`sum_row`, `avg_col`, etc.)
- table references `(row,col)` with error markers for invalid references
- rich table keyboard behavior:
  - `ArrowLeft/Right` stay within cell content before leaving the cell
  - `ArrowUp/Down` move by column inside table and only exit at table boundaries
  - `Ctrl+ArrowLeft/Right` jump to previous/next cell anchor
  - `Shift+Enter` splits cell content into multiline continuation rows (`|>`)
  - header-aware structural edits for column insert/delete/merge workflows

## Modules

Per-note module state is persisted and controls runtime behavior.

- `math`: enables calc engine
- `table`: enables table formula behavior
- `variables`: enables variable features
- `style`: enables markdown style helpers

## Security features

- `note lock`: app-level lock (note remains plaintext at rest)
- `note encrypt`: encrypted-at-rest note body
- password-bearing note-security commands are redacted from command history

## Export

- quick clipboard export
- file export to `.md`, `.txt`, `.pdf`
- command-based export with optional path for md/txt clipboard fallback
- markdown-aware PDF rendering with:
  - headings/lists/code blocks
  - markdown tables
  - markdown images
  - inline emphasis and checklist rendering

Details: [Export Reference](./export.md).

## Search and note navigation

- fuzzy note switcher
- note create/open/delete workflows
- first non-empty line title derivation

## Wiki links

- supported syntax:
  - `[[shortid]]`
  - `[[shortid#heading]]`
  - `[[shortid|title]]`
  - `[[shortid#heading|title]]`
- short id is an 8-character alphanumeric note-id prefix
- autocomplete suggestions for notes and headings
- GUI navigation via Ctrl/Cmd+click and vim `gd`
- TUI navigation via `Ctrl+]` and `gd`
- broken-link rendering with automatic re-resolution

## IMAP sync

- one-off sync command and optional background polling
- incremental UID checkpointing
- daily special note rotation for imported mail

## Frontend parity model

Slate targets semantic parity between GUI and TUI for shared-core-owned behavior.

- command parsing/planning: shared
- vim stepping: shared
- text semantics: shared
- rendering and host side effects: frontend/runtime specific
