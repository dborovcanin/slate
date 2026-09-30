# Feature Guide

Every Slate feature in one place, grouped by area, with the keys and commands that drive it.

Conventions:

- **Insert** is the text-editing mode (also the only mode when you never press `Esc`). **Normal** is vim normal mode.
- Commands are typed in the command bar: `:` in Normal/Visual, `Ctrl+E` in Insert or Normal. The leading `:` is optional and matching is case-insensitive. `Tab` in the command bar completes command words.
- Full alias lists: [Command Reference](./command-reference.md). Key tables by mode: [Keymaps](./keymaps.md).

## Contents

1. [Notes and switching](#notes-and-switching)
2. [Saving](#saving) and [note history](#note-history)
3. [Command line entry points](#command-line-entry-points)
4. [Daily notes, capture and pipe append](#daily-notes-capture-and-pipe-append)
5. [Editing and markdown helpers](#editing-and-markdown-helpers)
6. [Inline formatting and list commands](#inline-formatting-and-list-commands)
7. [Vim mode](#vim-mode)
8. [Macros](#macros)
9. [Search](#search)
10. [Wiki links](#wiki-links)
11. [Images and image preview](#images-and-image-preview)
12. [Inline calculations](#inline-calculations)
13. [Variables and cross-note variables](#variables-and-cross-note-variables)
14. [Tables](#tables)
15. [Folding](#folding)
16. [Dates and reminders](#dates-and-reminders)
17. [Web search](#web-search)
18. [Clipboard and clip-watch](#clipboard-and-clip-watch)
19. [Collections](#collections)
20. [Per-note modules](#per-note-modules)
21. [Note security](#note-security)
22. [Export and backup](#export-and-backup)
23. [IMAP email sync](#imap-email-sync)
24. [Themes, wrap and other settings](#themes-wrap-and-other-settings)
25. [Diagnostics](#diagnostics)

## Notes and switching

- Notes live in SQLite (`~/.local/share/slate/notes.db`, WAL mode). The last-open note is restored on start.
- A note's title is its first non-empty line (heading markers dropped).
- Markdown files open directly: `slate path/to/file.md` (`.md`, `.markdown`, `.mdown`, `.mkd`); edits are saved back to the file.

The note switcher, its content search and the collection picker are popups styled like the [collection browser](#collection-browser): a search bar over a list with icons, ages (or match lines) and note counts, and key hints at the bottom. The title shows whether the list covers the working collection or all notes.

| Key / command | Action |
| --- | --- |
| `Ctrl+N` (Insert, switcher) | New note (in the working collection, if one is set) |
| `Ctrl+P` | Fuzzy note switcher (type to filter by title) |
| `Tab` in the switcher | Switch to full-text content search; `Tab` again goes back to title search |
| `Enter` in the switcher / content search | Open the note (content search jumps to the matching line) |
| `Ctrl+L` in the switcher / content search | Toggle between the working collection and all notes |
| `Ctrl+R` in the switcher | [History](#note-history) of the selected note |
| `Delete` / `Ctrl+Backspace` in the switcher | Delete the selected note (confirm with `y`/`Enter`; protected notes ask for the password) |
| `Ctrl+W` in the switcher | Delete the last word of the query |
| `Esc` / `Ctrl+P` in the switcher | Close it |
| `Ctrl+Q` | Quit (from any mode) |

Locked or encrypted notes ask for their password before opening. Content search only indexes unprotected notes.

## Saving

| Key / command | Action |
| --- | --- |
| autosave (`[editor] autosave = true`, default) | Saves after a short idle pause on a background thread, and before switching notes or quitting |
| `Ctrl+S` | Save now (in Insert only while autosave is on; otherwise use `:w`) |
| `:w` / `:w!` | Write / force write (overwrites a newer revision on disk) |
| `:wq` / `:wq!` | Write and quit |
| `:q` / `:q!` | Quit / quit without saving |

`[editor] format_on_save = true` runs `:format` before every save. If the note changed elsewhere since it was loaded, a save stops with `note changed since last load; use :w! to force save`.

### Note history

Slate keeps older versions of every stored note. Saves less than five minutes apart form one editing session, and each session keeps the text from before it as a version (a session longer than half an hour is split). Versions are stored as the line changes between them, with an occasional full copy, so a small edit to a large note costs a few bytes; the history of an encrypted note is encrypted with it. File-backed notes (`slate file.md`) keep no history.

Old versions are thinned out when a new one is added: every version of the last day stays, then the newest of each day for a month, then the newest of each week, up to 500 versions per note.

`:history` (or `:versions`), `H` on a note in the browser, or `Ctrl+R` in the note switcher opens the history in the browser: the current text first, then each version with its time and the lines its session added and removed. The preview shows what restoring would change (`-` current lines, `+` lines of the version), or the version's text.

| Key | Action |
| --- | --- |
| `j` / `k`, arrows, `gg` / `G`, `Ctrl+D` / `Ctrl+U` | Move between the current text and older versions |
| `Tab`, `t` | Preview what restoring would change, or the version's full text |
| `Enter`, `l` | Restore the version (`y` confirms); the text before it is kept as a version |
| `h`, `-`, `Esc` | Back to the notes |
| `q`, `Ctrl+B` | Close the browser |

## Command line entry points

```text
slate                        open the last note
slate --new | -n             create and open a new note
slate --id <note-id>         open a specific note
slate --list | -l            list notes and exit (works without a TTY)
slate <file.md>              open a markdown file as a note
slate today                  open today's daily note
slate capture [text...]      add a timestamped entry to today's daily note and exit
cmd | slate capture          capture piped stdin
cmd | slate append [--id <note-id>]   append stdin to a note and exit
slate imap-sync              pull new mail once and exit
slate --help | -h
```

## Daily notes, capture and pipe append

- **Daily notes**: `slate today` or `:today` (alias `:daily`) opens today's note, creating it from the `[daily]` template (`note_prefix`, default `daily`; `template`, default `"# {date}\n\n"`). The cursor lands at the end, ready to type.
- **Quick capture**: `slate capture buy milk` appends a `- HH:MM buy milk` line to today's daily note without opening the editor. With no text it reads piped stdin, so any command output can be captured:

  ```sh
  slate capture call the bank at 3
  git log -1 --oneline | slate capture
  ```

- **Pipe append**: `cmd | slate append` appends stdin verbatim to the most recently edited note (email inbox notes are skipped); `--id <note-id>` picks the target note.

  ```sh
  curl -s wttr.in/?format=3 | slate append
  make test 2>&1 | slate append --id 01HX4VHR...
  ```

## Editing and markdown helpers

Markdown stays raw text; styling is drawn in place (no preview pane).

- live styling for headings, quotes, lists, checklists, inline code, code fences, bold/italic/strike, markdown links and wiki links
- list continuation on `Enter` (bullets, numbers, checklists)
- checklists `- [ ]` / `- [x]`; a trailing ` /x` toggles an item; with `[editor] checklist_auto_reorder = true` checked items move below unchecked ones
- markdown table autoformat and alignment while typing (see [Tables](#tables))
- soft wrap for prose (`[editor] wrap = true`); tables and code keep horizontal scrolling
- undo/redo with cursor restore

| Key | Action (Insert) |
| --- | --- |
| `Enter` | New line / continue list / accept popup selection |
| `Tab` / `Shift+Tab` | Accept completion, else apply calc result, else next/previous table cell, else indent/outdent list item, else insert two spaces |
| `Ctrl+W`, `Ctrl+Backspace` | Delete word before the cursor (table-aware) |
| `Ctrl+Delete` | Delete forward (table-aware) |
| `Ctrl+ArrowLeft` / `Ctrl+ArrowRight` | Previous / next word (previous / next cell in tables) |
| `Home` / `End`, `PageUp` / `PageDown` | Line start / end, page up / down |
| `ArrowUp` / `ArrowDown` | Move by screen row on wrapped lines; move popup selection when a popup is open |
| `Esc` | Close popup, else switch to Normal mode |

## Inline formatting and list commands

Select text first (`v` / `V`, then `:`); the command applies to the selection.

| Command | Aliases | Action |
| --- | --- | --- |
| `format bold` | `bold` | Toggle `**bold**` |
| `format italic` | `italic` | Toggle `*italic*` |
| `format strike` | `strike`, `strikethrough` | Toggle `~~strike~~` |
| `format code` | `icode`, `inline-code` | Toggle `` `code` `` |
| `format clear` | `clear-format`, `unformat`, `plain` | Strip inline formatting |
| `paragraph title` | `title`, `paragraph heading` | Turn lines into a heading |
| `paragraph clist` | `clist`, `checklist`, `todo`, ... | Turn lines into a checklist |
| `paragraph ulist` | `ulist`, `unordered-list`, `unordered` | Turn lines into a bullet list |
| `paragraph olist` | `olist`, `ordered-list`, `ordered` | Turn lines into a numbered list |
| `format` | `fmt` | Format the whole markdown document |

## Vim mode

Slate is modal: `Esc` leaves Insert for Normal mode. `[editor] vim_mode = true` starts in Normal mode instead of Insert.

| Keys | Action |
| --- | --- |
| `h` `j` `k` `l`, arrows | Move (with counts) |
| `w` / `b` | Next / previous word |
| `0` / `$` (`Home` / `End`) | Line start / end |
| `gg` / `G`, `{n}G` / `{n}gg` | First / last line, line `n` |
| `gj` / `gk` | Down / up one screen row on wrapped lines |
| `i` `a` `I` `A` `o` `O` | Enter Insert (before, after, line start, line end, new line below/above) |
| `x` | Delete character |
| `dd` `yy` `cc` | Delete / yank / change line (`3dd`, `2yy`) |
| `d` `y` `c` + `w` `b` `0` `$` | Operator + motion (`d2w`, `y$`); `de` / `ce` to word end |
| `dt{c}` / `ct{c}` | Delete / change up to character `c` |
| `diw` `daw` `yiw` `yaw` `ciw` `caw` | Word text objects |
| `di\|` `da\|` `yi\|` `ya\|` `ci\|` | Table cell text objects |
| `di(` `di[` `di{` `di"` ``di` `` `di*` `di~` `di_` (and `da`, `ci`, `ca`) | Delimiter text objects |
| `C` | Change to line end |
| `p` | Paste after |
| `u` / `Ctrl+R` | Undo / redo |
| `v` / `V` | Visual / visual-line mode; then `y` yank, `d`/`x` delete, `:` run a command on the selection |
| `/`, `Ctrl+F` | Search in note (see [Search](#search)) |
| `:` / `Ctrl+E` | Command bar |
| `za` | Toggle fold |
| `gd`, `Ctrl+]` | Follow wiki link; `gd` on a variable goes to its definition |
| `K` | Preview linked note |
| `gx` | Preview image at cursor |
| `?` | Web search |

Yanks are copied to the system clipboard as well as the register.

## Macros

Macros record Normal-mode actions and Insert-mode typing, and replay them.

| Keys | Action |
| --- | --- |
| `q{r}` | Start recording into register `r` (`a`-`z`, `0`-`9`; case-insensitive) |
| `q` | Stop recording (status shows `recorded @r (N steps)`) |
| `@{r}` | Replay register `r` |
| `{n}@{r}` | Replay `n` times (`3@a`) |
| `Esc` | Cancel a pending `q` / `@` before the register key |

Replays are capped at 10,000 steps; a longer replay stops with `replay aborted: step budget exceeded`.

## Search

In-note search (`/` or `Ctrl+F`) is case-insensitive and highlights every match.

| Key | Action |
| --- | --- |
| type | Jump to the nearest match below the start position |
| `Tab` / `ArrowDown` / `Ctrl+N` | Next match |
| `Shift+Tab` / `ArrowUp` / `Ctrl+P` | Previous match |
| `Enter` | Keep the position (status shows `/query (i/n)`) |
| `Esc` | Cancel and restore the original cursor and scroll |
| `n` / `N` (Normal) | Next / previous match after `Enter` |

Across notes: `Ctrl+P` then `Tab` opens content search (SQLite FTS), see [Notes and switching](#notes-and-switching).

## Wiki links

Syntax: `[[SHORTID]]`, `[[SHORTID#Heading]]`, `[[SHORTID|Title]]`, `[[SHORTID#Heading|Title]]`. `SHORTID` is the first 8 characters of the target note id; heading and title are display-only.

- Links render collapsed (title only) until the cursor enters them; broken links render distinctly and resolve when the note appears.
- Typing `[[` auto-closes to `[[]]` and opens a note picker; typing `#` inside a link suggests the target note's headings.

| Key | Action |
| --- | --- |
| `[[` | Insert link and open note autocomplete |
| `ArrowUp` / `ArrowDown`, `Enter` / `Tab`, `Esc` | Pick, accept, cancel a suggestion |
| `Ctrl+]` (any editing mode), `gd` (Normal) | Open the linked note |
| `K` (Normal) | Toggle a preview popup of the linked note; moving the cursor closes it |

Markdown links `[text](url)` are styled in place.

## Images and image preview

Syntax: `![alt](./assets/image.png)`. In the editor an image shows as a compact `[image: alt]` placeholder; the raw markdown appears while the cursor is inside it.

- Pasting an image file path imports the image into the note's assets and inserts a relative image link.
- The preview opens in a bounded dialog using sixel, kitty or iTerm2 graphics (half-blocks as fallback); decoding happens off the input thread and is cached. Inside tmux, passthrough is used when `allow-passthrough` is on.
- Remote (`http://...`) images are never fetched.

| Key | Action |
| --- | --- |
| `gx` (Normal), `Ctrl+O` (Insert, vim mode off) | Preview the image at the cursor |
| `o` in the preview | Open the image in the system viewer |
| `Esc` in the preview | Close it |

Config (`[terminal]`): `images = "auto" | "sixel" | "kitty" | "iterm2" | "halfblocks" | "off"` (`off` opens the external viewer directly) and `image_max_rows` (1-100, default 15).

## Inline calculations

Lines that evaluate show a ghost result on the right; non-math lines stay quiet.

```text
200 * 1.19              → 238
50 kg to lbs            → 110.231 lb
sqrt(144) + 3^2         → 21
```

- Arithmetic, percentages, unit conversions and everything [fend](https://github.com/printfn/fend) evaluates.
- Works on plain lines, list/checklist bodies, and table formula cells.
- Date-like lines (`2026-09-29`, `29.09.2026`, `09/29/2026`) are ignored.
- Large notes compute only the visible region and fill values in off the input thread.

| Key / command | Action |
| --- | --- |
| `Tab` (Insert) | Replace the expression with its result (after any open completion popup) |
| `:sum` / `:avg` | Sum / average the paragraph at the cursor |
| `:sum list` / `:avg list` | ... the list at the cursor |
| `:sum row` / `:avg row` | ... the table row |
| `:sum column` / `:avg column` | ... the table column |
| `:sum doc` / `:avg doc` | ... the whole note |

## Variables and cross-note variables

- Assign with `name := expression`; reference by name anywhere in the note. Names are case-insensitive and may contain spaces.
- Dependents update as definitions change; conversions store numeric values (`len := 3 m to km`).
- `gd` (Normal) on a variable jumps to its definition (the last assignment, the one in effect).
- Variable names are highlighted; an autocomplete popup appears after `[editor] variable_autocomplete_min_chars` (default 3) characters.
- Another note's variable: `[[SHORTID]].name` (needs the `cross_note` module); typing `[[SHORTID]].` suggests that note's variables.

| Key | Action |
| --- | --- |
| `ArrowUp` / `ArrowDown` | Move in the variable popup |
| `Tab` / `Enter` | Accept the suggestion |
| `Esc` | Close the popup |

Full rules: [Variables Specification](./variables.md).

## Tables

Markdown tables align as you type, and the cursor moves by cell.

- Formula cells start with `:=`: `:=sum_col()`, `:=avg_row() * 2`, `:=tax_rate * subtotal`, `:=(1,2) + (2,2)` (1-based data-row references).
- Helpers: `sum_row()`, `sum_col()`, `avg_row()`, `avg_col()`.
- Reference errors render as `!ERROR#out_of_bounds`, `!ERROR#non_numeric`, `!ERROR#self_reference`, `!ERROR#cycle`.
- Multiline cells use continuation rows starting with `|>`.

| Key (Insert) | Action |
| --- | --- |
| `\|` | Start a row / add a column where applicable |
| `Tab` / `Shift+Tab` | Next / previous cell |
| `Ctrl+ArrowLeft` / `Ctrl+ArrowRight` | Previous / next cell |
| `ArrowLeft` / `ArrowRight` | Move within the cell, then cross into the next |
| `ArrowUp` / `ArrowDown` | Same column, previous / next row |
| `Shift+Enter` | Split the cell into a `\|>` continuation row |
| `Backspace` / `Delete` | Cell-aware delete at cell boundaries |
| `Ctrl+Backspace` / `Ctrl+Delete` / `Ctrl+W` | Structural edit: merge cells or delete a column from the header |

Vim: `di|`, `ci|`, `yi|`, `da|` work on cells. `:sum row`, `:avg column`, etc. aggregate tables.

## Folding

Fold and unfold the block at the cursor.

| Key / command | Action |
| --- | --- |
| `za` (Normal) | Toggle fold at cursor |
| `:fold` (`:zc`, `:closefold`) | Fold |
| `:unfold` (`:zo`, `:openfold`) | Unfold |
| `:fold-toggle` (`:za`) | Toggle |

## Dates and reminders

| Command | Action |
| --- | --- |
| `:date` | Pick a date and insert it (`[editor] date_format`; `date_time_format` with time) |
| `:remind` (`:alarm`) | Set a reminder on the current line (date + time) |
| `:remind toggle` | Remove the line's reminder, or set one if there is none |
| `:today` (`:daily`) | Open today's daily note |

Date picker keys: arrows move by day/week, `Ctrl+ArrowLeft`/`Ctrl+ArrowRight` by month, `h`/`l` (`Home`/`End`) hour, `j`/`k` (`PageUp`/`PageDown`) minute, `Tab`/`t` toggle time, `Enter` confirm, `Esc` cancel.

Reminders show as a ghost on their line and fire a desktop notification (`notify-send`) when due; setting and removing them is undoable.

## Web search

| Key / command | Action |
| --- | --- |
| `?` (Normal), `:web [query]` (`:search-web`, `:lookup`) | Open the web search overlay (runs the query if given) |
| type, `Enter` | Run the search |
| `ArrowUp` / `ArrowDown` | Select a result |
| `Enter` on a result | Open it in the browser (HTTP(S) only) |
| `Shift+Enter` on a result | Insert it as a markdown link at the cursor |
| `Esc` | Close |

Config `[web_search]`: `provider = "auto" | "brave" | "google" | "duckduckgo"`, `api_key`, `search_engine_id`, `max_results` (1-10). Keyed providers fall back to DuckDuckGo on failure.

## Clipboard and clip-watch

- Vim yanks and `:export md` / `:export txt` without a path go to the system clipboard (status shows the backend used).
- Pasted text is inserted as-is (no autoformat pass).

| Command | Action |
| --- | --- |
| `:clip-watch on` (`start`) | Watch the clipboard and paste each new text at the cursor |
| `:clip-watch off` (`stop`) | Stop watching |

## Collections

Collections group notes; the working collection is session-scoped and applies to new notes, the switcher and content search.

| Key / command | Action |
| --- | --- |
| `Ctrl+G` | Collection picker (type to filter, `Enter` set working collection, `Ctrl+E` edit, `Esc` close) |
| `:collection create <name>` | Create |
| `:collection choose <name>` / `choose none` / `clear` | Set / clear the working collection |
| `:collection join <name>` / `leave <name>` | Add / remove the current note |
| `:collection update <name>` | Edit name, description and default tags (`Tab` between fields, `Enter` save) |
| `:collection delete <name>` | Delete the collection only |
| `:collection purge <name>` | Delete the collection and its notes |

### Collection browser

`Ctrl+B`, `-` in Normal mode, or `:browse` (`:explore`, `:files`) opens a full-screen browser in three columns, like the yazi file manager: collections on the left, the notes of the open collection in the middle and a preview of the hovered note (title, age, collections, tags and the start of the body) on the right. `All notes` and `Unsorted` (notes in no collection) sit above the collections. It opens on the working collection with the current note hovered.

`Ctrl+F` (or `Ctrl+/`) searches note text inside the open collection. Hits show the line they matched on, the preview scrolls to that line with the search terms highlighted, and `Enter` opens the note there.

A note can belong to several collections, so notes are copied into collections rather than moved like files: `y` then `p` adds a note to another collection, `x` then `p` moves it out of the collection it was cut from, and `d` removes it from the open collection without deleting it. `u` undoes the last of these.

| Key | Action |
| --- | --- |
| `j` / `k`, arrows, `gg` / `G`, `Ctrl+D` / `Ctrl+U` | Move |
| `l`, `Enter`, `o` | Open the collection or note (asks for the password of a locked note) |
| `h`, `-`, `Backspace` | Back to the collections list |
| `/` | Filter the list (fuzzy); `Esc` clears it |
| `Ctrl+F`, `Ctrl+/` | Search note text in the open collection (or the hovered one): type to search, `↑` `↓` / `Ctrl+N` `Ctrl+P` move, `Enter` opens the note at the match, `Esc` goes back |
| `Space` / `Ctrl+A` | Mark the note and move down / mark all |
| `y` / `x` | Copy / cut the marked (or hovered) notes |
| `p` | Paste into the open or hovered collection: copy adds the notes to it, cut moves them |
| `d` | Remove the notes from the open collection (the notes stay) |
| `u` | Undo the last paste or remove |
| `D` | Delete the notes, or the hovered collection (its notes stay); `y` confirms |
| `a` / `r` | New note (in the open collection) or collection / rename the hovered one |
| `s` | Sort notes by modification time or title |
| `w` | Make the collection the working collection (`All notes` clears it) |
| `R` | Reload |
| `q`, `Esc`, `Ctrl+B` | Close |

Icons use Nerd Font glyphs by default; set `[theme] icons = "unicode"` or `"ascii"` for other fonts.

## Per-note modules

Each note stores its own module switches; `[editor.modules]` sets defaults for new notes.

| Module | Controls |
| --- | --- |
| `math` | Master calc switch |
| `table` | Table formulas |
| `variables` | Variable assignments, references, autocomplete |
| `style` | Markdown styling helpers |
| `cross_note` | `[[SHORTID]].name` references |

Commands: `:module status` (`:modules`), `:module <name> on|off|toggle` (also `:module on <name>`).

## Note security

| Command | Action |
| --- | --- |
| `:note lock <password>` | Require a password to open the note in Slate (body stays plaintext on disk) |
| `:note unlock <password>` | Remove the app lock |
| `:note encrypt <password>` | Encrypt the note body at rest |
| `:note decrypt <password>` | Store it unencrypted again |
| `:note unprotect <password>` | Remove any lock or encryption and the password |

Passwords are redacted from command history. `[editor.security] encrypt_notes = true` encrypts new notes by default using the password in `password_env`. Not available for file-backed notes.

## Export and backup

| Command | Action |
| --- | --- |
| `:export pdf <path>` | Markdown-aware PDF (headings, lists, code, tables, images, checklists) |
| `:export md [path]` | Markdown to a file, or the clipboard without a path |
| `:export txt [path]` | Plain text to a file, or the clipboard |
| `:backup export <path.zip>` | Back up the whole notes database (runs in the background) |
| `:backup load <path.zip>` | Stage a restore; Slate quits and applies it on the next start |

Details: [Export Reference](./export.md).

## IMAP email sync

- `slate imap-sync` pulls new messages once; `[imap] auto_sync_on_startup = true` polls in the background while Slate runs.
- Messages are appended to daily inbox notes (`inbox-email-YYYY-MM-DD` by default), newest first, with UID checkpoints so nothing is imported twice.
- First sync only takes recent mail (`initial_sync_past_days`, default 1). New mail triggers a desktop notification.
- Any IMAP-over-TLS provider works; the password comes from `password_env`. Build without IMAP: `cargo build -p slate --no-default-features`.

## Themes, wrap and other settings

- `[theme] color_scheme` (14 schemes, default `gruvbox-light`) and `accent`; config changes reload live.
- `[editor] wrap`, `markdown_autoformat`, `checklist_auto_reorder`, `autosave`, `format_on_save`, `vim_mode`, `date_format`, `date_time_format`, `variable_autocomplete_min_chars`.

All keys: [Configuration](./configuration.md).

## Diagnostics

| Command | Action |
| --- | --- |
| `:perf status` | Show perf tracing state |
| `:perf toggle` / `on` / `off` | Toggle tracing |
| `:perf dump [top]` | Write the slowest buckets to the perf log |
| `:perf where` | Show the log path |
| `:perf cap <n>` | Samples kept per bucket |

Planned features: [`roadmap/features.md`](../roadmap/features.md).
