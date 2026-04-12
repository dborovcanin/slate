# Handoff Notes

This document captures what was implemented after milestones M1-M4, plus what should be done next.

## Theme system pass (latest)

- Added config-driven theme loading from TOML:
  - `src-tauri/src/config/mod.rs`
  - command: `get_theme_config`
  - file path: `$XDG_CONFIG_HOME/note/config.toml` (fallback `~/.config/note/config.toml`)
  - default file is auto-created on first run
- Added frontend theme engine with presets:
  - color schemes: 12 built-ins
  - backgrounds: `plain`, `lines`, `squares`, `dots`, `diagonal`
  - fonts: `jetbrains-mono`, `fira-code`, `cascadia-code`, `iosevka`, `hack`, `source-code-pro`
  - startup application happens in `src/main.ts` before app initialization
- Added live theme reload:
  - frontend polling watcher in `src/theme/theme.ts`
  - started during bootstrap in `src/main.ts`
  - re-applies only when resolved selection changes
- Added keyboard font controls:
  - `Ctrl + +` / `Ctrl + -` adjusts font size
  - `Ctrl + Alt + +` / `Ctrl + Alt + -` cycles font family
- Added optional Vim key mapping via config:
  - `[editor] vim_mode = true` in `config.toml`
  - current subset: insert/normal modes, hjkl, w/b, 0/$, gg/G, x, dd, u, Ctrl+r, o/O, i/a/I/A
  - count prefixes supported for movement/actions (examples: `4k`, `2j`, `3w`, `5x`, `3dd`)
  - cursor style differs by mode (insert bar cursor vs normal/visual block cursor)
  - visual selections: `v`, `V`, `Ctrl+v` (block)
  - yanking to system clipboard: `y` in visual modes, `yy` in normal mode (with counts like `3yy`)
- Added optional terminal mode:
  - `[editor] terminal_mode = true` in `config.toml`
  - `note --terminal` runs standalone terminal UI (no Tauri window)
  - default mode follows config when launched from a TTY
  - built-in full-screen editor with switcher and status bar
  - `vim_mode` remains independent/optional and applies to GUI mode only
- Added unified command system (single command engine + shared command panel UI):
  - command engine lives in `src/editor/command-engine.ts`
  - command panel UI lives in `src/editor/command-picker.ts`
  - Vim mode opens panel with `:` and enables Vim-only commands such as `:q`
  - non-Vim mode opens panel with `Ctrl+Shift+;` and exposes edit-only commands
  - current shared commands: `sum`, `sum list`, `sum table`, `sum doc`, `date`, `format`
  - `:sum` results are inserted at cursor/selection as `<value>` (value-only), copied to clipboard, and shown in command status
  - new `:format` command applies document-wide markdown formatting (table alignment, heading/list normalization, trailing-space cleanup)
- Added date insertion with calendar picker:
  - shortcut: `Ctrl+Shift+D`
  - Vim ex command: `:date`
  - config key: `[editor] date_format = "%Y-%m-%d"`
- Added markdown-style rich editing decorations:
  - headings, quotes, lists, horizontal rules
  - inline bold/italic/strikethrough/code/link highlighting
  - checklist rendering polish for `- [ ]` and `- [x]` lines
  - fenced code block styling
  - implemented as lightweight CodeMirror decorations in `src/editor/markdown-decoration.ts`
- Added markdown editing helpers:
  - configurable autoformat flag: `[editor] markdown_autoformat` (default `true`)
  - keymaps: `Ctrl/Cmd+B`, `Ctrl/Cmd+I`, `Ctrl/Cmd+Shift+X`, `Ctrl/Cmd+K`
  - Enter list continuation behavior
  - ` /x` suffix toggles checklist state on the active line (`- item /x` -> `- [x] item`, `- [x] item /x` -> `- [ ] item`)
  - table autoformat/alignment on edit in markdown table blocks
- Added theme tests:
  - Rust parser/normalization tests in config module
  - TypeScript preset inventory tests

## What was polished in this pass

- Added confirmation prompt before note deletion (`Ctrl+Shift+Backspace`)
- Restored `Ctrl+Backspace` to word-delete behavior in the editor
- Added safe async action wrapper for keyboard-triggered commands to avoid unhandled promise rejections
- Made shortcuts case-robust (`Ctrl+Shift+E`, etc.)
- Startup optimization:
  - removed duplicate frontend config fetch during bootstrap
  - app init now consumes the same in-flight config promise used for first theme application
- Improved live UI updates:
  - Status/title updates while typing
  - Switcher list refreshes while open as notes change
  - Active edited note moves to top in local state
- Fixed calc annotation race condition (stale async results are dropped and re-evaluated)
- Reduced calc false positives by ignoring date-like lines in calc detection (`YYYY-MM-DD`, `DD.MM.YYYY`, `MM/DD/YYYY`)
- Extended calc inline apply for markdown structures:
  - table cell expressions (`| ... | 4+2 |`) now evaluate and `Tab` replaces the cell value in place
  - ordered/unordered/checklist item expressions now evaluate and `Tab` replaces item expression in place
- Improved calc performance:
  - frontend calc decoration now uses incremental diff planning and evaluates only changed lines
  - unchanged prefix/suffix results are reused and remapped across insert/delete edits
  - backend calc evaluation is line-local (fresh fend context per line), matching inline-per-line behavior
- Command/rule context polish:
  - introduced `src/editor/core` modular editor core for presentation-independent behavior
  - `EditorContextSnapshot` + lazy `ResolvedContext` now drive command/rule execution
  - CodeMirror now acts as an adapter (`snapshot -> core -> edit operations`) instead of owning command/rule logic
  - text rules moved into core pipeline (`doc_change` and `key_enter`) with behavior-preserving order:
    checklist `/x` toggle first, then table autoformat when enabled
  - command engine moved to core pipeline with operation-based results and mode-gated command catalog
  - sum scope resolution now runs via context-layer line/block resolvers (except explicit `sum doc`)
- Added IPC socket parent directory creation before bind (more robust fallback path behavior)
- Fixed a critical SQLite deadlock in `save_note` caused by nested mutex locking

## Tests added

### TypeScript (`node --test --experimental-strip-types`)

- `src/state.test.ts`
  - title derivation behavior
  - state updates and note reordering
  - adjacent navigation boundaries
- `src/switcher/fuzzy.test.ts`
  - fuzzy match success/failure behavior
  - fuzzy sort ordering
- `src/editor/markdown-editing.test.ts`
  - table alignment formatting
  - missing-cell normalization
  - empty input behavior
- `src/editor/core/context.test.ts`
  - line/column and block range resolution (paragraph/list/table)
  - word resolution around cursor
- `src/editor/core/text-rules.test.ts`
  - checklist toggle rule behavior
  - table autoformat rule behavior
  - Enter list continuation/exit behavior
- `src/editor/core/commands.test.ts`
  - mode-aware command suggestions
  - core command execution outputs (sum/date/q)
- `src/editor/ex-commands.test.ts`
  - sum scope and number parsing coverage
- `src/editor/command-engine.test.ts`
  - mode-gated command availability (`vim` vs `editor`)
  - suggestion filtering behavior
- `src/editor/markdown-format.test.ts`
  - markdown normalization and table formatting coverage

### Rust (`cargo test`)

- `src-tauri/src/calc/engine.rs`
  - arithmetic evaluation
  - non-expression skipping
  - date-like line skipping for ghost calc suppression
  - re-evaluation of lines already containing ` = result`
  - batch line evaluation
- `src-tauri/src/storage/sqlite.rs`
  - CRUD lifecycle
  - most-recent/list ordering by `updated_at`
  - delete idempotency behavior

## Commands

```sh
# Build frontend bundle + typecheck (used by Tauri build)
npm run build

# Run all tests
npm run test

# TS tests only
npm run test:ts

# Rust tests only
npm run test:rust
```

## Known gaps / suggested next work

1. Add a proper confirmation modal (current implementation uses `window.confirm`).
2. Add frontend tests around calc decoration apply behavior and switcher keyboard navigation.
3. Add integration tests for Tauri commands (`notes`, `calc`, `export`, `get_theme_config`) via command layer.
4. Consider moving clipboard export from `navigator.clipboard` to a Tauri clipboard plugin for stricter Linux/WebView reliability.
5. Add CI (`npm run build`, `npm run test`) so regressions are blocked automatically.
