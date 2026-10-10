# Desktop App (GPUI)

`crates/gui` (package `slate-gui`, binary `slate-gui`) is an experimental desktop
front end built with [GPUI](https://crates.io/crates/gpui). It is a second
adapter over the same core crates as the terminal app: notes open through
`note-session`, calculations run through the session's calc provider and line
styling comes from `note_session::display`. The GUI only lays out and paints
that output.

Status: work in progress on the `ui` branch. The terminal app (`slate`) is
unaffected.

## What works

- **Notes:** opens the most recent note (or `--id`) from the same database as
  `slate`; sidebar to switch notes; `Ctrl+N` new note; `:today`; autosave
  after 1.5 s of quiet and on quit or note switch.
- **Display:** styled-in-place markdown, live calc results, variable
  highlighting, tables with formula values, checklists, reminder chips, soft
  wrapping of long lines. `--light` uses the light theme.
- **Editing modes:** vim (default) or standard, switched in View › Editing
  mode. Vim uses the shared vim engine (`note_session::input`): motions,
  operators, text objects, registers, undo/redo, Visual modes, counts.
  Standard mode has Ctrl+Z/Y, Ctrl+C/X/V, Ctrl+A, Ctrl+B/I, Shift-selection.
- **Mouse:** click places the cursor at the character, drag selects (Visual
  mode in vim), double click selects a word, triple click a line, right click
  opens the context menu.
- **Menus:** File, Edit, View, Format, Calc and Help menus and the right-click
  menu show the vim keys (in vim mode) and standard shortcuts.
- **Command palette:** `:` in vim mode or `Ctrl+Shift+P`, with the core's
  command catalog, completion and descriptions. Which-key strip after a
  pending prefix (`g`, `d`, `y`, `c`, text objects).
- **Commands:** all core commands (sum/avg, formatting, lists, `:format`),
  plus host commands: write/quit/reload, today, date, modules, collections
  (create, delete, purge, join, leave), reminders (`:remind`), scripts
  (`:run`), export to Markdown or text, note encryption and decryption.
- **Collection browser** (`Ctrl+O`) and **note history** (`Ctrl+Shift+H`)
  with diff preview and restore.
- **Encrypted notes:** locked notes show a placeholder and ask for the
  password; `:encrypt` / `:decrypt` prompt for it.
- **Images:** a line that is only `![alt](src)` is drawn as the image (scaled to
  fit, decoded in the background) until the cursor is on it. `Ctrl+V`/`p`
  and `:paste-image` import a clipboard image into the note.
- **Wiki links:** `[[id]]` shows the target note's title away from the cursor
  (via `note_session::display::wiki`); `Ctrl+]` and `gd` follow them.
- **Tables:** hover shows + bars to add a row or column.
- **Clipboard:** yanks go to the system clipboard; `p` with an empty register
  and Ctrl+V paste from it.

## Shortcuts

The global keys are the terminal app's (see [Keymaps](./keymaps.md)):

| Key      | Action                                                      |
| -------- | ----------------------------------------------------------- |
| `Ctrl+P` | Note switcher (fuzzy title search, `Ctrl+L` scope, `Ctrl+N`, `Ctrl+G`, `Ctrl+R`) |
| `Ctrl+G` | Collection picker: sets the working collection              |
| `Ctrl+B` | Collection browser                                          |
| `Ctrl+E` | Command line (`:` in vim Normal mode)                       |
| `Ctrl+]` | Follow the wiki link at the cursor (`gd` also follows variables) |
| `Ctrl+N` | New note (in the working collection)                        |
| `Ctrl+S` / `Ctrl+Q` | Save / quit                                      |
| `F1`     | Keys and commands                                           |
| `Ctrl+\` | Toggle the sidebar (desktop only)                          |

Standard editing mode adds the usual desktop keys: `Ctrl+Z` / `Ctrl+Y`,
`Ctrl+C` / `Ctrl+X` / `Ctrl+V`, `Ctrl+A`, `Shift`+arrows to select, and
`Ctrl+Shift+B` / `Ctrl+Shift+I` for bold and italic (`Ctrl+B` stays the
browser, as in the terminal).

## Settings

View › Command bar chooses where the command line appears: a **popup**
(default) or the **bottom** of the window, as in the terminal app. The choice,
the editing mode, the theme and the sidebar are remembered in
`gui-settings.json` in the Slate data directory.

## Not in the desktop app yet

These are reported in the status bar when invoked:

- PDF export, `:backup`, `:web-search`, `:currency refresh`, clip-watch,
  folding, in-note search (`/`), macros.
- Vertical motion follows logical lines, not wrapped rows (`gj`/`gk` too).
- IME composition and accessibility are untested.

Terminal-only behavior that the shared modules do not own yet (fold-aware
motion, table-cell motion, autoformat after typing, autocomplete popups) is
the main gap between the two front ends; see `roadmap/arch-refactor.md`.

## Why a separate workspace

`crates/gui` has its own `[workspace]` and `Cargo.lock` and is listed under
`exclude` in the root `Cargo.toml`. Terminal builds, `cargo test --workspace`
and CI never fetch or compile GPUI. Build it with `make` or with
`--manifest-path crates/gui/Cargo.toml`.

## Build

Linux needs the X11/Wayland, Vulkan and font development packages. On
Debian or Ubuntu:

```sh
make ui-deps      # apt-get install of the packages GPUI needs
```

Then:

```sh
make ui           # release build: crates/gui/target/release/slate-gui
make ui-run       # build and start it
make ui-run ARGS="--id <note-id>"   # open a specific note
make ui-test      # unit tests for the GUI crate
```

The first build compiles GPUI and its dependencies and takes several minutes.

The window opens the same notes database as the terminal app (the Slate data
directory's `notes.db`), so both see the same notes.

## Logging

GPUI reports renderer and platform problems through `log`:

```sh
RUST_LOG=info make ui-run
```

## Notes

- `libc` is pinned to 0.2.188 in `crates/gui/Cargo.lock`: `xattr 0.2.3`, which
  GPUI pulls in through its HTTP client, does not compile against later
  `libc` releases. Avoid a blanket `cargo update` in `crates/gui` until GPUI
  moves off it.
- A GPU with Vulkan support is required on Linux (Mesa's drivers are enough).
  Under Xvfb, Mesa's software renderer (`mesa-vulkan-drivers`, llvmpipe) works
  for screenshots and scripted tests.
- Fonts: IBM Plex Sans/Mono are used when installed, else Inter, DejaVu or
  Liberation.
