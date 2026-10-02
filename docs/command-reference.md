# Command Reference

Every command-bar command with its aliases. For what each feature does and its keys, see the [Feature Guide](./features.md).

## Command modes

- Command bar: `:` in Normal/Visual mode, `Ctrl+E` in Insert or Normal mode.
- The leading `:` is optional; matching is case-insensitive.
- `Tab` / `Shift+Tab` complete command words; `ArrowUp` / `ArrowDown` walk command history.
- Commands run from Visual mode apply to the selection.
- `w`, `wq`, `q` (and their `!` forms) are available only when the bar was opened from Normal/Visual mode.

## Calculation

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

## Dates, reminders and daily notes

| Command | Aliases | Purpose |
| --- | --- | --- |
| `today` | `daily` | Open today's daily note, creating it from the `[daily]` template |
| `browse` | `explore`, `files` | Open the collection browser |
| `history` | `versions` | Browse and restore older versions of the open note |
| `date` | | Insert a picked date |
| `remind` | `alarm` | Set a reminder for the current line |
| `remind toggle` | | Remove the line's reminder, or set one if there is none |

## Formatting and lists

| Command | Aliases | Purpose |
| --- | --- | --- |
| `format` | `fmt` | Format markdown document |
| `format bold` | `bold` | Toggle `**` around selection |
| `format italic` | `italic` | Toggle `*` around selection |
| `format strike` | `strike`, `strikethrough` | Toggle `~~` around selection |
| `format code` | `icode`, `inline-code` | Toggle `` ` `` around selection |
| `format clear` | `clear-format`, `unformat`, `plain` | Strip inline formatting from selection |
| `paragraph title` | `title`, `paragraph heading`, `paragraph-title` | Convert selected lines to a heading |
| `paragraph clist` | `clist`, `checklist`, `checkbox`, `checkboxes`, `todo`, `format checklist`, `format clist`, `paragraph checklist` | Convert selected lines to a checklist |
| `paragraph ulist` | `ulist`, `unordered-list`, `unordered`, `format unordered`, `format ulist`, `paragraph unordered` | Convert selected lines to an unordered list |
| `paragraph olist` | `olist`, `ordered-list`, `ordered`, `format ordered`, `format olist`, `paragraph ordered` | Convert selected lines to an ordered list |

## Modules

| Command | Purpose |
| --- | --- |
| `module status` | Show the note's module state (aliases: `module`, `modules`, `modules status`) |
| `module math on/off/toggle` | Change math module |
| `module table on/off/toggle` | Change table module |
| `module variables on/off/toggle` | Change variables module |
| `module style on/off/toggle` | Change style module |
| `module cross_note on/off/toggle` | Change cross-note variables module |

`modules` works in place of `module`, and the action may come first: `module on math`, `modules off variables`.

## Folding

| Command | Aliases | Purpose |
| --- | --- | --- |
| `fold` | `closefold`, `zc` | Fold block at cursor |
| `unfold` | `openfold`, `zo` | Unfold block at cursor |
| `fold-toggle` | `fold_toggle`, `za` | Toggle fold at cursor |

## Clipboard watch

| Command | Aliases | Purpose |
| --- | --- | --- |
| `clip-watch on` | `clip-watch start` | Paste each new clipboard text at the cursor |
| `clip-watch off` | `clip-watch stop` | Stop clipboard watch |

## Web search

| Command | Aliases | Purpose |
| --- | --- | --- |
| `web [query]` | `search-web`, `lookup` | Open the web search overlay, running `query` if given |

## Note security

| Command | Aliases | Purpose |
| --- | --- | --- |
| `note encrypt` | `note-encrypt`, `encrypt-note` | Encrypt the note with its own password; on an unlocked encrypted note, change its password |
| `note decrypt` | `note-decrypt`, `decrypt-note` | Store the note unencrypted (a note that left an encrypted collection takes the collection's password) |

Both ask for the password in a masked dialog (encrypting asks twice); a password typed on the command line is ignored. Collections are encrypted from the browser (`e`).

A note in an encrypted collection stays encrypted with the collection's password while it belongs to it: `note decrypt` is refused, and so is `note encrypt` for a note the collection's password protects. Remove the note from the collection (`:collection leave`) or decrypt the whole collection (`e` in the browser) first.

## Export and backup

| Command | Aliases | Purpose |
| --- | --- | --- |
| `export pdf <path>` | | Export note to PDF |
| `export md [path]` | `export markdown` | Markdown to file, or clipboard without a path |
| `export txt [path]` | `export text` | Plain text to file, or clipboard without a path |
| `backup export <path.zip>` | | Back up the full notes database |
| `backup load <path.zip>` | | Stage a restore; applied after Slate quits |

## Collections

| Command | Purpose |
| --- | --- |
| `collection create <name>` | Create a collection |
| `collection delete <name>` | Delete a collection only |
| `collection purge <name>` | Delete a collection and all its notes |
| `collection choose <name>` | Set the working collection for this session |
| `collection choose none` / `collection clear` | Clear the working collection |
| `collection join <name>` | Add current note to a collection |
| `collection leave <name>` | Remove current note from a collection |
| `collection update <name>` | Edit name, description and default tags |

## Write and quit (Normal/Visual command bar)

| Command | Aliases | Purpose |
| --- | --- | --- |
| `w` | `write` | Write/save |
| `w!` | | Force write over a newer revision |
| `wq` | `writequit` | Write and quit |
| `wq!` | | Force write and quit |
| `q` | | Quit |
| `q!` | | Quit without saving |

## Diagnostics

| Command | Purpose |
| --- | --- |
| `perf status` | Show perf tracing state (also `perf`, `profile`, `profiler`) |
| `perf on` / `perf off` / `perf toggle` | Enable / disable / toggle tracing |
| `perf dump [top]` | Write the slowest buckets to the perf log |
| `perf where` | Show the perf log path |
| `perf clear` | Drop collected samples |
| `perf cap <n>` | Samples kept per bucket |
