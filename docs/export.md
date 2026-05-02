# Export Reference

Slate supports clipboard and file export from GUI and TUI.

## Export formats

- markdown (`.md`)
- plain text (`.txt`)
- PDF (`.pdf`)

## Command syntax

- `export pdf <path>`
- `export md [path]`
- `export txt [path]`

If path is omitted:

- `md` and `txt` export to clipboard
- `pdf` returns usage error and requires a path

Path behavior:

- `~` / `~/...` is expanded to `$HOME` in command-path exports
- relative paths are resolved from the current process working directory
- parent directory must already exist and be writable

## GUI export entry points

- `Ctrl+E`: export note body to clipboard
- `Ctrl+Shift+E`: open file export dialog
- command bar export via `export ...`

## TUI export entry points

- command bar export via `export ...`
- clipboard fallback for `export md` / `export txt` with no path

## PDF rendering details

PDF export is markdown-aware and currently includes:

- headings and paragraph flow
- ordered/unordered lists
- checklist rendering
- fenced/inline code styling
- markdown tables
- markdown images (`![alt](src)`)

Color behavior follows export palette rules used by host runtime.

## Image sources

Image resolution supports:

- db-backed image placeholders resolved through note source service
- file-backed markdown image paths
- data-url image payloads

## Error behavior

Typical export error classes:

- invalid/missing path for file export
- filesystem write failures
- unsupported/bad image payload during PDF decode
- clipboard write failure for clipboard exports

Host command status messages are returned to the status bar/toast layer.
