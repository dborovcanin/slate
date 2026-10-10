# Desktop App (GPUI)

`crates/gui` (package `slate-gui`, binary `slate-gui`) is an experimental desktop
front end built with [GPUI](https://crates.io/crates/gpui). It is a second
adapter over the same core crates as the terminal app: notes open through
`note-session`, calculations run through the session's calc provider and line
styling comes from `note_session::display`. The GUI only lays out and paints
that output.

Status: work in progress on the `ui` branch. The terminal app (`slate`) is
unaffected.

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

## Notes

- `libc` is pinned to 0.2.188 in `crates/gui/Cargo.lock`: `xattr 0.2.3`, which
  GPUI pulls in through its HTTP client, does not compile against later
  `libc` releases. Avoid a blanket `cargo update` in `crates/gui` until GPUI
  moves off it.
- A GPU with Vulkan support is required on Linux (Mesa's drivers are enough).
