# Slate

A minimal, fast, keyboard-first scratchpad for Linux. Inspired by [Antinote](https://antinote.io/).

Open fast, type, close. Notes are autosaved locally. No accounts, no cloud, no bloat.

## Features

**Editor**
- Plain-text editing powered by CodeMirror 6 (undo/redo, IME, clipboard — all browser-grade)
- Autosave with 500ms debounce + flush on blur and close
- Restores last-open note on startup
- Dark theme with Catppuccin-inspired colors
- Shared command panel in both Vim and non-Vim modes
  - Vim mode: open with `:`
  - Non-Vim mode: open with `Ctrl+Shift+;` (Ctrl+colon)
  - Built-in commands: `sum`, `sum list`, `sum row`, `sum column`, `sum doc`, `avg`, `avg list`, `avg row`, `avg column`, `avg doc`, `date`, `format`, `clip-watch`, `clip-watch-stop`, `clist`, `ulist`, `olist`
- Modular editor core (`src/editor/core`): snapshot-based context + operation-based command/rule engine, designed for GUI/terminal parity
- Markdown-style rich editing (live visual styling for headings, quotes, lists, inline code, links, bold/italic/strike while keeping raw markdown editable)
- Checklist markdown (`- [ ]`, `- [x]`) gets dedicated visual styling (`☐` / crossed `☒` + done-item strike style)
- Markdown editing helpers:
  - `Ctrl/Cmd+B` bold, `Ctrl/Cmd+I` italic, `Ctrl/Cmd+Shift+X` strikethrough, `Ctrl/Cmd+K` link
  - List continuation on Enter
  - End a list/checklist line with ` /x` to toggle checkbox state (`- item /x` -> `- [x] item`, `- [x] item /x` -> `- [ ] item`)
  - Checklist auto-reorder (when enabled): checking an item moves it to the bottom, unchecking moves it to the top
  - Markdown table autoformat/alignment while editing
- Controlled by `[editor] markdown_autoformat` (defaults to `true`) and `[editor] checklist_auto_reorder` (defaults to `true`)

**Themes and backgrounds**
- 12 built-in color schemes: `catppuccin-mocha`, `catppuccin-latte`, `gruvbox-dark`, `gruvbox-light`, `dracula`, `dark`, `white`, `solarized-dark`, `solarized-light`, `nord`, `tokyo-night`, `one-dark`
- 5 background patterns: `plain`, `lines`, `squares`, `dots`, `diagonal`
- Configurable via TOML (`$XDG_CONFIG_HOME/slate/config.toml` or `~/.config/slate/config.toml`)
- Live reload while the app is running (polls config changes automatically)

**Optional Vim mode**
- Enable with `[editor] vim_mode = true`
- Insert/normal modes (`Esc` to normal, `i` to insert)
- Cursor changes by mode (insert: bar cursor, normal: block cursor + `NORMAL` badge)
- Count prefixes for movement/actions: `4k`, `2j`, `3w`, `5x`, `3dd`
- Visual modes: `v` (visual), `V` (visual line), `Ctrl+v` (visual block)
- Yanking to system clipboard: `y` in visual modes, `yy` in normal mode (supports counts like `3yy`)
- Ex commands: `:sum` (paragraph default), `:sum list`, `:sum row`, `:sum column`, `:sum doc`, `:avg`, `:avg list`, `:avg row`, `:avg column`, `:avg doc`, `:date`, `:format`, `:clip-watch`, `:clip-watch-stop`, `:clist`, `:ulist`, `:olist`, `:q`
- `:sum` and `:avg` compute from the selected scope, insert only `<value>` at cursor/selection, and copy the value to clipboard
- Supported motions/actions: `h j k l`, `w b`, `0 $`, `gg`, `G`, `x`, `dd`, `u`, `Ctrl+r`, `o`, `O`, `a`, `A`, `I`

**Optional terminal mode**
- Enable with `[editor] terminal_mode = true`
- Or run explicitly with `slate --terminal`
- Runs as a standalone full-screen terminal app (no Tauri window)
- Built-in editor with note list/switcher and status bar
- Terminal flags: `--new`, `--id <note-id>`, `--list`, `--gui`, `--terminal`
- Terminal shortcuts: `Ctrl+N` new, `Ctrl+P` switch notes, `Ctrl+S` save, `Ctrl+Q`/`Ctrl+W` quit
- `vim_mode` is optional and independent (GUI-only behavior)

**Inline calculations**
- Type a math expression and see the result as a ghost annotation to the right of the line
- Press Tab to apply the result inline
- Uses note-context evaluation with line-aware extraction (normal lines, list bodies, single calc table cell)
- In markdown tables, lists, and checklists, calc applies to the expression part and `Tab` replaces that expression in place
- Supports arithmetic, unit conversions (`50 kg to lbs`), percentages, and everything [fend](https://github.com/printfn/fend) can evaluate
- Non-math lines are ignored — no noise
- Date-like lines (`YYYY-MM-DD`, `DD.MM.YYYY`, `MM/DD/YYYY`) are ignored to avoid false numeric ghost suggestions

**Variables (Antinote-style, note-local)**
- Define variables with `:=` assignment syntax: `name := expression`
- Variable names are case-insensitive and may include spaces (letters/digits/underscore/space)
- Variables are note-local and reactive: dependent lines recompute as definitions change
- Conversion assignments are normalized to numeric values for reuse (`len := 3 m to km`, then `len + 2`)
- Assignment lines do not render calc ghosts
- Unresolved/cyclic variable expressions fail silently (no ghost noise)
- Variable autocomplete popup appears while typing (default after 3 chars); `Tab` accepts completion when popup is open, otherwise `Tab` applies calc ghost
- Full behavior contract: `docs/variables.md`

```
200 * 1.19              → 238
50 kg to lbs            → 110.231 lb
sqrt(144) + 3^2         → 21
```

**Multiple notes**
- Create, switch, and delete notes with keyboard shortcuts
- Fuzzy search switcher (Ctrl+P) with match highlighting
- Titles derived from the first non-empty line — no extra fields to fill

**Export**
- Ctrl+E copies the current note to clipboard
- Ctrl+Shift+E opens a native file dialog to save as `.md` or `.txt`
- Toast feedback on export

**Keyboard shortcuts**

| Shortcut                | Action                        |
| ----------------------- | ----------------------------- |
| Ctrl+N                  | New note                      |
| Ctrl+P                  | Fuzzy note switcher           |
| Ctrl+↑ / Ctrl+↓         | Previous / next note          |
| Ctrl+Backspace          | Delete previous word          |
| Ctrl+Shift+Backspace    | Delete current note (confirm) |
| Ctrl+Shift+;            | Open command picker           |
| Tab                     | Apply calc result             |
| Ctrl+E                  | Copy note to clipboard        |
| Ctrl+Shift+E            | Export note to file           |
| Ctrl+Shift+D            | Open calendar date picker     |
| Ctrl+B                  | Toggle bold (`**...**`)       |
| Ctrl+I                  | Toggle italic (`*...*`)       |
| Ctrl+Shift+X            | Toggle strikethrough          |
| Ctrl+K                  | Insert/wrap markdown link     |
| Ctrl++ / Ctrl+-         | Increase / decrease font size |
| Ctrl+Alt++ / Ctrl+Alt+- | Next / previous font family   |
| Ctrl+W                  | Hide window                   |
| Ctrl+Z / Ctrl+Y         | Undo / redo                   |
| Escape                  | Close switcher                |

**Storage**
- SQLite with WAL mode in `~/.local/share/slate/notes.db`
- Config file is auto-generated on first run (`~/.config/slate/config.toml`)

## Requirements

- Arch Linux (primary target) or any Linux distro with:
  - WebKitGTK 4.1+
  - GTK 3
- Sway / Wayland (tested) — X11 should work via XWayland

## Build

### Prerequisites

```sh
# Arch Linux
sudo pacman -S webkit2gtk-4.1 gtk3 base-devel

# Rust toolchain
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# Node.js (v18+)
# Use your preferred method (nvm, pacman, etc.)

# Tauri CLI
cargo install tauri-cli --version "^2"
```

### Production build

```sh
npm install
cargo tauri build
```

Output binary:
- `src-tauri/target/release/slate` (14MB) — the app

Install them:

```sh
cp src-tauri/target/release/slate ~/.local/bin/
```

Or use:

```sh
make install
```

### Cross-platform release artifacts

```sh
make release
```

`make release` writes artifacts into `build/`:
- `slate-linux`
- Windows NSIS installer (`*.exe`) when run on a native Windows host (MSVC toolchain)
- macOS binaries when run on macOS, or when an osxcross toolchain is configured

Notes:
- Windows builds require `src-tauri/icons/icon.ico` (auto-generated from `icons/256x256.png` when `magick` is available).
- Windows installer uses WebView2 `offlineInstaller` mode, so the runtime is bundled in the installer (larger installer size, no internet required at install time).
- Build Windows installers from a native Windows shell with MSVC tools (`cl.exe`) available (e.g. "x64 Native Tools Command Prompt for VS"), not from Linux/WSL.
- macOS cross-build from Linux is skipped unless `o64-clang` and `oa64-clang` are installed.

### Development

```sh
npm install
cargo tauri dev
```

This starts the Vite dev server with HMR for the frontend and compiles the Rust backend in debug mode. Changes to `.ts`/`.css` files hot-reload instantly. Rust changes trigger a recompile on save.

### Debug build

```sh
# Rust backend only (fast iteration on backend changes)
cd src-tauri && cargo check

# TypeScript type-checking only
npx tsc --noEmit

# Full debug build (no optimizations, includes debug symbols)
cargo tauri build --debug
```

Debug binary: `src-tauri/target/debug/slate`

To see WebView devtools, right-click inside the app window during `cargo tauri dev`.

## Testing

Run all tests:

```sh
npm run test
```

Run frontend-only tests:

```sh
npm run test:ts
```

Run backend-only tests:

```sh
npm run test:rust
```

Record startup baseline metrics:

```sh
npm run perf:startup:record
```

Check startup regressions against the baseline:

```sh
npm run perf:startup:check
```

Current automated coverage:
- TypeScript unit tests for fuzzy search scoring and app state updates
- TypeScript unit tests for theme preset inventory
- Rust unit tests for calc evaluation behavior
- Rust integration-style unit tests for SQLite CRUD + ordering logic

## Configuration (TOML)

Config file location:
- `$XDG_CONFIG_HOME/slate/config.toml`
- fallback: `~/.config/slate/config.toml`

The file is generated automatically on first run.
Theme changes are picked up live while the app is running (typically within ~1-2 seconds).

Example:

```toml
[theme]
color_scheme = "gruvbox-light"
background = "plain"
font = "jetbrains-mono"
font_size = 14

[editor]
markdown_autoformat = true
checklist_auto_reorder = true
format_on_save = false
terminal_mode = false
vim_mode = false
date_format = "%Y-%m-%d"

[editor.variables]
enabled = true
autocomplete_min_chars = 3
```

Available `color_scheme` values:
- `catppuccin-mocha`
- `catppuccin-latte`
- `gruvbox-dark`
- `gruvbox-light`
- `dracula`
- `dark`
- `white`
- `solarized-dark`
- `solarized-light`
- `nord`
- `tokyo-night`
- `one-dark`

Available `background` values:
- `plain`
- `lines`
- `squares`
- `dots`
- `diagonal`

Available `font` values:
- `jetbrains-mono`
- `fira-code`
- `cascadia-code`
- `iosevka`
- `hack`
- `source-code-pro`

`font_size` range:
- `11` to `28`

Editor options:
- `markdown_autoformat = true` enables Enter list continuation and table auto-alignment while editing markdown
- `checklist_auto_reorder = true` automatically moves checked checklist items to the bottom and unchecked items to the top
- `format_on_save = false` runs `:format` before save when enabled (Ctrl+S and autosave flush path)
- `terminal_mode = true` makes `slate` default to terminal runtime (when launched from a TTY)
- `vim_mode = true` enables modal Vim-style key mappings in GUI
- `date_format` controls date insertion format for `Ctrl+Shift+D` and `:date`

Variable options:
- `[editor.variables] enabled = true` turns note-local variable resolution/autocomplete on or off
- `[editor.variables] autocomplete_min_chars = 3` controls the minimum typed characters before variable suggestions appear

Supported `date_format` tokens:
- `%Y` year (4 digit), `%y` year (2 digit)
- `%m` month (01-12), `%d` day (01-31)
- `%b` short month (`Jan`), `%B` full month (`January`)

## Architecture

Tauri v2 app: Rust backend + vanilla TypeScript frontend.

- **Frontend:** CodeMirror 6 editor, fuzzy switcher overlay, zero-framework vanilla TS
- **Backend:** Rust with rusqlite (bundled SQLite, WAL mode), fend-core (calc engine), Tauri IPC commands, Unix socket IPC server
- **Storage:** Single SQLite database, ULID-keyed notes, ISO 8601 timestamps

```
                    Unix socket IPC
┌─────────────────────────────────┐
│          slate (Tauri v2)       │
│                                 │
│  Frontend        Backend        │
│  ┌────────────┐  ┌────────────┐ │
│  │ CodeMirror │  │ SQLite+WAL │ │
│  │ Calc ghost │  │ fend-core  │ │
│  │ Switcher   │  │ IPC server │ │
│  │ Export     │  │ Export I/O │ │
│  └────────────┘  └────────────┘ │
└─────────────────────────────────┘
```

## Project structure

```
src/                          # Frontend (TypeScript)
  main.ts                     # Entry point
  app.ts                      # App shell, shortcuts, status bar, toast
  api.ts                      # Typed Tauri invoke wrappers
  state.ts                    # App state, note list, events
  editor/
    editor.ts                 # CodeMirror setup, autosave
    calc-decoration.ts        # Inline calc ghost annotations
  switcher/
    switcher.ts               # Fuzzy search overlay
    fuzzy.ts                  # Fuzzy match scoring
  styles/                     # CSS (theme, editor, switcher, app)

src-tauri/                    # Backend (Rust)
  src/
    lib.rs                    # Tauri app setup, plugin registration
    main.rs                   # Binary entry point
    commands/
      notes.rs                # Note CRUD
      calc.rs                 # Batch line evaluation
      export.rs               # File export
    storage/
      sqlite.rs               # Connection, migrations, WAL, queries
      models.rs               # Note struct
    calc/
      engine.rs               # fend-core wrapper + heuristics
    ipc/
      server.rs               # Unix socket listener
  migrations/
    0001_init.sql             # Schema
```

## License

TBD
