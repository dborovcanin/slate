# Desktop App (GPUI)

`crates/gui` (package `slate-gui`, binary `slate-gui`) is an experimental desktop
front end built with [GPUI](https://crates.io/crates/gpui). It is a second
adapter over the same core crates as the terminal app: notes open through
`note-session`, calculations run through the session's calc provider and line
styling comes from `note_session::display`. The GUI only lays out and paints
that output.

Status: work in progress on the `ui` branch. The terminal app (`slate`) is
unaffected.

What works now:

- Opens the most recent note (or `--id`), from the same database as `slate`.
- Notes sidebar; click a note to open it.
- Styled-in-place markdown: headings, bold/italic/code, checklists, hidden
  markers revealed on the cursor line.
- Live calc results, variable highlighting, and tables with formula values.
- Status bar with mode, dirty mark, module chips, calc result and position.
- Click a line to move the cursor there. `--light` uses the light theme.

Not yet: keyboard input and editing (vim and standard modes), menus, the
command palette, collection browser, history and saving. Wiki links show their
raw `[[...]]` text because that display rule still lives in the terminal app.

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
