# Configuration

Slate reads config from:

- `$XDG_CONFIG_HOME/slate/config.toml`
- fallback: `~/.config/slate/config.toml`

The file is auto-generated on first run.

## Example

```toml
[theme]
color_scheme = "gruvbox-light"
accent = "auto"
icons = "nerd"

[editor]
markdown_autoformat = true
checklist_auto_reorder = true
autosave = true
reload_outside_changes = true
format_on_save = false
vim_mode = false
wrap = true
date_format = "%Y-%m-%d"
date_time_format = "%Y-%m-%d %H:%M"
variable_autocomplete_min_chars = 3

[editor.modules]
math = true
table = true
variables = true
style = true

[editor.security]
encrypt_notes = false
password_env = "SLATE_NOTES_PASSWORD"

[special_notes]
email_note_prefix = "inbox-email"
email_rotation = "daily-local"

[imap]
host = "imap.example.com"
port = 993
username = ""
password_env = "SLATE_IMAP_PASSWORD"
folder = "INBOX"
poll_seconds = 60
auto_sync_on_startup = false
initial_sync_max_messages = 200
initial_sync_past_days = 1
max_message_bytes = 8388608
max_body_bytes = 524288

[mcp]
enabled = false
allow_delete = false
```

## Theme settings

`[theme]` keys:

- `color_scheme`
- `accent`
- `icons`

### color_scheme values

- `slate-light`
- `slate-dark`
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

### accent values

- `auto`
- `amber`
- `sage`
- `rose`
- `plum`
- `cobalt`
- `slate`
- custom hex like `#4f7bd9`

### icons values

- `nerd` (default): Nerd Font glyphs, as in yazi or lsd. Needs a [Nerd Font](https://www.nerdfonts.com/) in the terminal.
- `unicode`: symbols from standard Unicode blocks, for any font.
- `ascii`: plain ASCII markers.

## Editor settings

`[editor]` keys:

- `markdown_autoformat`
- `checklist_auto_reorder`
- `autosave`
- `reload_outside_changes`
- `format_on_save`
- `vim_mode`
- `wrap`
- `date_format`: inserted dates (`:date`), daily note titles, and the dates of notes older than a week in the browser and switcher
- `date_time_format`: inserted dates with time, reminders, and a note's modified and added times in the browser preview
- `variable_autocomplete_min_chars`

Behavior notes:

- `autosave = true` enables implicit save flows (idle flush + save on exit/switch).
- with `autosave = false`, explicit writes persist body changes. Switching notes or quitting with unsaved changes is refused once (vim's E37); repeating it, `:q!` or `:e!` discards them.
- a failed save (for example a note changed elsewhere) keeps the editor open with the changes; autosave waits for the next edit. `:w!` overwrites, `:e!` reloads.
- `reload_outside_changes = true` (default) checks the open note about once a second while idle and loads changes made elsewhere (`slate mcp`, `slate append`, `slate capture`, IMAP sync, another Slate) as one undoable edit when there are no unsaved changes. Set it to `false` to skip the check; such changes then show up as a save conflict, as before.
- `format_on_save = true` runs format before save.
- `vim_mode = true` starts the editor in vim normal mode.
- `wrap = true` (default) soft-wraps long lines at word boundaries. Tables, code fences, code blocks, and code files keep horizontal scrolling so columns stay aligned. With `wrap = false` every line scrolls horizontally.
- `variable_autocomplete_min_chars` is clamped to `1..8`.

## Per-note module defaults

`[editor.modules]` sets defaults for new notes:

- `math`
- `table`
- `variables`
- `style`

Module state is then persisted per note and can diverge note-by-note.

## Note security defaults

`[editor.security]` keys:

- `encrypt_notes`: whether newly created notes default to encrypted-at-rest storage
- `password_env`: environment variable name used for default encryption password

Behavior notes:

- if `encrypt_notes = true`, `password_env` must resolve to a non-empty environment variable at runtime
- a new note in an encrypted working collection is encrypted with the collection's key instead
- note encrypt/decrypt commands are not supported for file-backed markdown notes (`slate path/to/file.md`)

## Daily notes

`[daily]` keys:

- `note_prefix` (default `daily`): daily note ids are `<prefix>-YYYY-MM-DD`
- `template` (default `"# {date}\n\n"`): body of a new daily note; `{date}` uses `[editor] date_format`

Daily notes are opened with `:today` or `slate today`, and `slate capture <text>` (or piped stdin) appends a `- HH:MM text` entry without opening the editor.

## Special notes

`[special_notes]` keys:

- `email_note_prefix`
- `email_rotation` (currently `daily-local`)

## IMAP

`[imap]` keys:

- connection: `host`, `port`, `username`, `password_env`, `folder`
- polling: `poll_seconds`, `auto_sync_on_startup`
- bootstrap and limits:
  - `initial_sync_max_messages`
  - `initial_sync_past_days`
  - `max_message_bytes`
  - `max_body_bytes`

Runtime validation constraints:

- `poll_seconds`: `10..86400`
- `initial_sync_max_messages`: `1..100000`
- `initial_sync_past_days`: `0..3650`
- `max_body_bytes <= max_message_bytes`
- `host`, `username`, `password_env`, and `folder` must be non-empty

## MCP server

`[mcp]` keys:

- `enabled` (default `false`): lets `slate mcp` serve notes to MCP clients such as AI assistants. While it is `false`, `slate mcp` refuses to start.
- `allow_delete` (default `false`): also offers `delete_note`, which deletes a note and its history for good. Without it, clients can only move notes to the `Archive` collection.

Notes created over MCP use `[editor.modules]` and `[editor.security]` like notes created in the editor. See [MCP Server](mcp.md) for client setup.

## Date format tokens

Common supported tokens:

- `%Y`, `%y`
- `%m`, `%d`
- `%b`, `%B`
