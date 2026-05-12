# Command Reference

This page documents canonical command-bar commands and common aliases.

## Command modes

Slate supports two command modes:

- editor mode command bar (non-vim)
- vim ex command mode

Most commands are available in both modes.

Vim-only commands:

- `q` / `q!`
- `w` / `w!`
- `wq` / `wq!`

## Syntax notes

- leading `:` is optional in parser normalization
- command matching is case-insensitive after normalization
- note security and export commands support extra arguments (password/path)

## Calculation commands

| Command | Purpose |
| --- | --- |
| `sum` | Sum paragraph (default scope) |
| `sum list` | Sum list at cursor |
| `sum row` | Sum markdown table row |
| `sum column` | Sum markdown table column |
| `sum doc` | Sum whole document |
| `avg` | Average paragraph (default scope) |
| `avg list` | Average list at cursor |
| `avg row` | Average markdown table row |
| `avg column` | Average markdown table column |
| `avg doc` | Average whole document |

Common aliases:

- `sum_row`, `sum_column`, `sum all`, `sum_all`
- `avg_row`, `avg_column`, `avg all`, `avg_all`

## Date and reminders

| Command | Purpose |
| --- | --- |
| `date` | Insert picked date |
| `notify` | Set reminder for current line |
| `notify-delete` | Delete reminder on current line |

Common reminder aliases:

- `alarm`
- `remind`

## Module commands

| Command | Purpose |
| --- | --- |
| `module status` | Show active-note module state |
| `module math on/off/toggle` | Change math module |
| `module table on/off/toggle` | Change table module |
| `module variables on/off/toggle` | Change variables module |
| `module style on/off/toggle` | Change style module |

Common aliases:

- `module`, `modules`, `modules status`
- ordering variants like `module on math`, `modules off variables`

## Formatting and list transforms

| Command | Purpose |
| --- | --- |
| `format` | Format markdown document |
| `clist` | Convert selected lines to checklist |
| `ulist` | Convert selected lines to unordered list |
| `olist` | Convert selected lines to ordered list |

Common aliases:

- `fmt`
- `checklist`, `checkbox`, `checkboxes`, `todo`
- `unordered-list`, `ordered-list`

## Clipboard watch

| Command | Purpose |
| --- | --- |
| `clip-watch on` | Start clipboard watch |
| `clip-watch off` | Stop clipboard watch |

Common aliases:

- start: `clip-watch`, `clip_watch`, `clip-watch start`
- stop: `clip-watch-stop`, `clip_watch_stop`, `clip-watch stop`

## Folding

| Command | Purpose |
| --- | --- |
| `fold` | Fold at cursor |
| `unfold` | Unfold at cursor |
| `fold-toggle` | Toggle fold at cursor |

Common aliases:

- `zc` for fold
- `zo` for unfold
- `za` for toggle

## Note security

| Command | Purpose |
| --- | --- |
| `note lock <password>` | App-level lock note |
| `note unlock <password>` | Unlock note |
| `note encrypt <password>` | Encrypt note at rest |
| `note decrypt <password>` | Decrypt note |
| `note unprotect <password>` | Remove lock/encryption protection |

Alias examples:

- `note-lock`, `lock-note`
- `note-encrypt`, `encrypt-note`
- `note-unprotect`, `note unencrypt`, `unencrypt-note`

## Export

| Command | Purpose |
| --- | --- |
| `export pdf <path>` | Export note to PDF file |
| `export md [path]` | Export markdown to file, or clipboard if path omitted |
| `export txt [path]` | Export plain text to file, or clipboard if path omitted |

Aliases:

- `export markdown`
- `export text`

## Collections

| Command | Purpose |
| --- | --- |
| `collection create <name>` | Create a collection |
| `collection delete <name>` | Delete a collection only |
| `collection purge <name>` | Delete a collection and all associated notes |
| `collection choose <name>` | Set working collection for this session |
| `collection choose none` / `collection clear` | Clear working collection |
| `collection join <name>` | Add current note to collection |
| `collection leave <name>` | Remove current note from collection |
| `collection update <name>` | Open collection update dialog (name/description/default tags) |

Common aliases:

- `choose_collection`
- `add_to_collection`
- `remove_from_collection`

## Vim write/quit commands

| Command | Purpose |
| --- | --- |
| `w` | Write/save |
| `w!` | Force write |
| `wq` | Write and quit |
| `wq!` | Force write and quit |
| `q` | Quit |
| `q!` | Force quit |
