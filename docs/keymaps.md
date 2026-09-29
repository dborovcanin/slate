# Keymaps

Key tables by mode. The [Feature Guide](./features.md) lists the same keys grouped by feature.

**Insert** is the text-editing mode; **Normal** is vim normal mode (`Esc` from Insert; `[editor] vim_mode = true` starts there).

## Everywhere

| Key | Action |
| --- | --- |
| `Ctrl+Q` | Quit |
| `Ctrl+P` | Note switcher |
| `Ctrl+G` | Collection picker |
| `Ctrl+E` | Command bar (Insert, Normal, Visual) |
| `Ctrl+]` | Follow wiki link at cursor (Insert, Normal) |
| `Ctrl+S` | Save (in Insert only while autosave is on) |

## Insert mode

| Key | Action |
| --- | --- |
| `Ctrl+N` | New note |
| `Ctrl+F` | Search in note |
| `Ctrl+O` | Preview image at cursor (vim mode off) |
| `Tab` | Accept completion, else apply calc result, else next table cell, else indent list item, else two spaces |
| `Shift+Tab` | Previous table cell / outdent list item |
| `Enter` | New line, continue list, or accept popup selection |
| `Shift+Enter` | Split table cell into a `\|>` continuation row |
| `Ctrl+W`, `Ctrl+Backspace` | Delete word before cursor (table-aware) |
| `Ctrl+Delete` | Delete forward (table-aware) |
| `Ctrl+ArrowLeft` / `Ctrl+ArrowRight` | Previous / next word (previous / next cell in tables) |
| `ArrowUp` / `ArrowDown` | Screen-row motion on wrapped lines; same column in tables; popup selection |
| `Home` / `End`, `PageUp` / `PageDown` | Line start / end, page up / down |
| `[[` | Insert wiki link and open note autocomplete |
| `Esc` | Close popup, else Normal mode |

## Normal mode

| Key | Action |
| --- | --- |
| `h` `j` `k` `l`, arrows | Move (counts supported) |
| `w` / `b` | Next / previous word |
| `0` / `$`, `Home` / `End` | Line start / end |
| `gg` / `G`, `{n}G` | First / last / `n`th line |
| `gj` / `gk` | Screen row down / up on wrapped lines |
| `i` `a` `I` `A` `o` `O` | Enter Insert |
| `x`, `p` | Delete char, paste after |
| `dd` `yy` `cc` `C` | Delete / yank / change line, change to line end |
| `d` `y` `c` + `w` `b` `0` `$` (`e` for `d`/`c`) | Operator + motion |
| `dt{c}` / `ct{c}` | Delete / change till char |
| `iw` `aw` `i\|` `a\|` | Word and table-cell text objects (`diw`, `yi\|`, ...) |
| `i(` `i[` `i{` `i"` `` i` `` `i*` `i~` `i_` (and `a...`) | Delimiter text objects for `d` and `c` |
| `u` / `Ctrl+R` | Undo / redo |
| `v` / `V` | Visual / visual-line |
| `/`, `Ctrl+F` | Search; `n` / `N` next / previous match |
| `:` | Command bar |
| `q{r}` ... `q`, `@{r}`, `{n}@{r}` | Record, stop, replay macro |
| `za` | Toggle fold |
| `gd` | Follow wiki link |
| `K` | Toggle linked-note preview |
| `gx` | Preview image at cursor |
| `?` | Web search |

## Visual and visual-line mode

| Key | Action |
| --- | --- |
| motions, counts, `gg` / `G`, `gj` / `gk` | Extend selection |
| `y` | Yank (also to system clipboard) |
| `d` / `x` | Delete |
| `:` / `Ctrl+E` | Command bar on the selection (format, list, calc commands) |
| `v` / `V` / `Esc` / `Ctrl+C` | Leave visual mode |

## Table editing (Insert)

| Key | Action |
| --- | --- |
| `ArrowLeft` / `ArrowRight` | Move within cell content; cross cells at boundaries |
| `ArrowUp` / `ArrowDown` | Same column, previous / next row |
| `Tab` / `Shift+Tab`, `Ctrl+ArrowLeft` / `Ctrl+ArrowRight` | Next / previous cell |
| `Shift+Enter` | Split cell into multiline continuation row (`\|>`) |
| `Backspace` / `Delete` | Cell-aware boundary edit |
| `Ctrl+Backspace` / `Ctrl+Delete` / `Ctrl+W` | Structural edit (merge cells, delete column from header) |

## Popups (variables, wiki links)

| Key | Action |
| --- | --- |
| `ArrowUp` / `ArrowDown` | Move selection |
| `Tab` / `Enter` | Accept |
| `Esc` | Close |

## In-note search (`/`)

| Key | Action |
| --- | --- |
| `Tab` / `ArrowDown` / `Ctrl+N` | Next match |
| `Shift+Tab` / `ArrowUp` / `Ctrl+P` | Previous match |
| `Enter` | Keep position |
| `Esc` | Cancel, restore cursor and scroll |

## Note switcher (`Ctrl+P`)

| Key | Action |
| --- | --- |
| type | Filter by title |
| `ArrowUp` / `ArrowDown` | Move selection |
| `Enter` | Open note (asks for password if protected) |
| `Tab` | Content search (full text); `Tab` there returns to title search |
| `Ctrl+L` | Toggle working collection / all notes |
| `Ctrl+N` | New note |
| `Ctrl+G` | Collection picker |
| `Delete` / `Ctrl+Backspace` | Delete selected note (`y` / `Enter` confirm, `n` / `Esc` cancel) |
| `Ctrl+W` | Delete last query word |
| `Esc` / `Ctrl+P` | Close |

## Collection picker (`Ctrl+G`)

| Key | Action |
| --- | --- |
| type, `ArrowUp` / `ArrowDown` | Filter, move selection |
| `Enter` | Set working collection |
| `Ctrl+E` | Edit collection (`Tab` / `Shift+Tab` between fields, `Enter` save, `Esc` cancel) |
| `Esc` / `Ctrl+G` / `Ctrl+P` | Close |

## Image preview

| Key | Action |
| --- | --- |
| `o` | Open in system viewer |
| `Esc` | Close |

## Web search (`?`, `:web`)

| Key | Action |
| --- | --- |
| type, `Enter` | Run search |
| `ArrowUp` / `ArrowDown` | Select result |
| `Enter` | Open result in browser |
| `Shift+Enter` | Insert result as markdown link |
| `Esc` | Close |

## Date picker (`:date`, `:remind`)

| Key | Action |
| --- | --- |
| arrows | Day / week |
| `Ctrl+ArrowLeft` / `Ctrl+ArrowRight` | Month |
| `h` / `l` (`Home` / `End`) | Hour |
| `j` / `k` (`PageUp` / `PageDown`) | Minute |
| `Tab` / `t` | Toggle time |
| `Enter` / `Esc` | Confirm / cancel |

## Command bar and text inputs

| Key | Action |
| --- | --- |
| `ArrowLeft` / `ArrowRight` | Move cursor (cycle completions while the Tab menu is open) |
| `Home` / `End`, `Ctrl+A` / `Ctrl+E` | Start / end of input |
| `Ctrl+ArrowLeft` / `Ctrl+ArrowRight` | Previous / next word |
| `Backspace` / `Delete` | Delete before / at cursor |
| `Ctrl+W`, `Ctrl+Backspace` / `Ctrl+Delete` | Delete word before / after cursor |
| `Ctrl+U` / `Ctrl+K` | Delete to start / end |
| `Tab` / `Shift+Tab` | Complete command words |
| `ArrowUp` / `ArrowDown` | Command history |
| `Enter` / `Esc` | Run / cancel |

Command syntax and aliases: [Command Reference](./command-reference.md).
