# Note

A minimal, fast, keyboard-first scratchpad for Linux. Inspired by [Antinote](https://antinote.io/).

Open fast, type, close. Notes are autosaved locally. No accounts, no cloud, no bloat.

## Features

**Editor**
- Plain-text editing powered by CodeMirror 6 (undo/redo, IME, clipboard — all browser-grade)
- Autosave with 500ms debounce + flush on blur and close
- Restores last-open note on startup

**Inline calculations**
- Type a math expression and see the result as a ghost annotation to the right of the line
- Press Tab to apply the result inline
- Supports arithmetic, unit conversions (`50 kg to lbs`), percentages, and everything [fend](https://github.com/printfn/fend) can evaluate
- Non-math lines are ignored — no noise

```
200 * 1.19              → 238
50 kg to lbs            → 110.231 lb
sqrt(144) + 3^2         → 21
```

**Multiple notes**
- Create, switch, and delete notes with keyboard shortcuts
- Fuzzy search switcher (Ctrl+P) with match highlighting
- Titles derived from the first non-empty line — no extra fields to fill

**Keyboard shortcuts**

| Shortcut         | Action                  |
|------------------|-------------------------|
| Ctrl+N           | New note                |
| Ctrl+P           | Fuzzy note switcher     |
| Ctrl+↑ / Ctrl+↓ | Previous / next note    |
| Ctrl+Backspace   | Delete current note     |
| Tab              | Apply calc result       |
| Ctrl+Z / Ctrl+Y | Undo / redo             |

**Storage**
- SQLite with WAL mode in `~/.local/share/note/notes.db`
- No config files, no setup

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

Binary output: `src-tauri/target/release/note` (≈13MB, self-contained).

Install it wherever you like:

```sh
cp src-tauri/target/release/note ~/.local/bin/
```

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

Debug binary: `src-tauri/target/debug/note`

To see WebView devtools, right-click inside the app window during `cargo tauri dev`.

### Sway integration

Add to your Sway config:

```
# Float the window
for_window [app_id="note"] floating enable

# Toggle with a keybind (launches if not running, toggles visibility if running)
# Requires note-msg CLI (coming in a future milestone)
# bindsym $mod+n exec note-msg toggle || note
```

## Architecture

Tauri v2 app: Rust backend + vanilla TypeScript frontend.

- **Frontend:** CodeMirror 6 editor, fuzzy switcher overlay, zero-framework vanilla TS. ~92KB gzipped.
- **Backend:** Rust with rusqlite (bundled SQLite, WAL mode), fend-core (calc engine), Tauri IPC commands.
- **Storage:** Single SQLite database, ULID-keyed notes, ISO 8601 timestamps.

```
Frontend (WebView)              Backend (Rust)
┌────────────────────┐          ┌─────────────────┐
│ CodeMirror editor  │──invoke──│ Note CRUD        │
│ Calc decorations   │          │ SQLite + WAL     │
│ Fuzzy switcher     │          │ fend-core calc   │
│ Keyboard shortcuts │          │ Tauri commands   │
└────────────────────┘          └─────────────────┘
```

## Project structure

```
src/                          # Frontend (TypeScript)
  main.ts                     # Entry point
  app.ts                      # App shell, shortcuts, note switching
  api.ts                      # Typed Tauri invoke wrappers
  state.ts                    # App state, note list, events
  editor/
    editor.ts                 # CodeMirror setup, autosave
    calc-decoration.ts        # Inline calc ghost annotations
  switcher/
    switcher.ts               # Fuzzy search overlay
    fuzzy.ts                  # Fuzzy match scoring
  styles/                     # CSS

src-tauri/                    # Backend (Rust)
  src/
    lib.rs                    # Tauri app setup
    commands/                 # Tauri IPC commands
      notes.rs                # Note CRUD
      calc.rs                 # Line evaluation
    storage/                  # SQLite layer
      sqlite.rs               # Connection, migrations, queries
      models.rs               # Note struct
    calc/
      engine.rs               # fend-core wrapper
  migrations/
    0001_init.sql             # Schema
```

## License

TBD
