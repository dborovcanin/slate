# Export Reference

Slate supports clipboard and file export from GUI and TUI.
It also supports full database backup and restore commands for moving all notes together.

## Export formats

- markdown (`.md`)
- plain text (`.txt`)
- PDF (`.pdf`)

## Command syntax

- `export pdf <path>`
- `export md [path]`
- `export txt [path]`
- `backup export <path.zip>`
- `backup load <path.zip>`
- `backup <path.zip>` (legacy alias for `backup export`)
- `backup notes <path.zip>` (legacy alias for `backup export`)

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

## Database Backup

`backup export <path.zip>` writes a portable zip containing:

- `notes.db`: a consistent SQLite snapshot of the full Slate notes database
- `manifest.json`: backup metadata
- `README.txt`: manual restore notes

The zip uses stored (uncompressed) entries so the file can be opened by any standard zip tool.

## Database Restore

`backup load <path.zip>` restores notes from a Slate backup zip without requiring any manual file operations:

1. Slate reads `notes.db` out of the zip and stages it at `<data_dir>/notes.db.staged-restore`.
2. The TUI session exits.
3. On exit, Slate atomically replaces the live `notes.db` with the staged file.
4. Re-open Slate to use the restored notes.

The original `notes.db` is moved aside during the swap and removed once the restore succeeds, so a failed rename cannot leave the database in a broken state.

Manual restore is still possible: close Slate, unzip the backup, and replace the active `notes.db` with the extracted one.
