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
background = "plain"
font = "jetbrains-mono"
font_size = 14

[editor]
markdown_autoformat = true
checklist_auto_reorder = true
autosave = true
format_on_save = false
terminal_mode = false
vim_mode = false
date_format = "%Y-%m-%d"
date_time_format = "%Y-%m-%d %H:%M"
variable_autocomplete_min_chars = 3

[editor.modules]
math = true
table = true
variables = true
style = true

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
```

## Theme settings

`[theme]` keys:

- `color_scheme`
- `accent`
- `background`
- `font`
- `font_size`

### color_scheme values

- `slate`
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

### background values

- `plain`
- `lines`
- `squares`
- `dots`
- `diagonal`

### font values

- `jetbrains-mono`
- `fira-code`
- `cascadia-code`
- `iosevka`
- `hack`
- `source-code-pro`

`font_size` range: `11` to `28`.

## Editor settings

`[editor]` keys:

- `markdown_autoformat`
- `checklist_auto_reorder`
- `autosave`
- `format_on_save`
- `terminal_mode`
- `vim_mode`
- `date_format`
- `date_time_format`
- `variable_autocomplete_min_chars`

Behavior notes:

- `autosave = true` enables implicit save flows in GUI and TUI.
- with `autosave = false`, explicit writes persist body changes.
- `format_on_save = true` runs format before save.
- `terminal_mode = true` makes `slate` default to terminal runtime when possible.
- `vim_mode = true` enables GUI vim key mappings.

## Per-note module defaults

`[editor.modules]` sets defaults for new notes:

- `math`
- `table`
- `variables`
- `style`

Module state is then persisted per note and can diverge note-by-note.

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

## Date format tokens

Common supported tokens:

- `%Y`, `%y`
- `%m`, `%d`
- `%b`, `%B`
