# Keymaps

This document summarizes GUI, terminal, and vim key mappings.

## GUI global shortcuts

| Shortcut | Action |
| --- | --- |
| `Ctrl+N` | New note |
| `Ctrl+P` | Open note switcher |
| `Ctrl+Up` / `Ctrl+Down` | Previous / next note |
| `Ctrl+Shift+;` | Open command picker |
| `Ctrl+E` | Export active note to clipboard |
| `Ctrl+Shift+E` | Export active note to file |
| `Ctrl+Shift+D` | Open date picker |
| `Ctrl+B` | Toggle bold markdown |
| `Ctrl+I` | Toggle italic markdown |
| `Ctrl+Shift+X` | Toggle strikethrough markdown |
| `Ctrl+K` | Insert or wrap markdown link |
| `Ctrl/Cmd+Alt+Z` | Toggle fold at cursor |
| `Ctrl++` / `Ctrl+-` | Increase / decrease font size |
| `Ctrl+Alt++` / `Ctrl+Alt+-` | Next / previous font family |
| `Ctrl+W` | Hide window |
| `Ctrl+Q` | Quit window |
| `Ctrl+Z` / `Ctrl+Y` | Undo / redo |
| `Tab` | Accept variable completion or apply calc result |
| `Escape` | Close active overlay (switcher, picker, etc.) |

## Switcher keys (GUI + TUI)

| Key | Action |
| --- | --- |
| `ArrowUp` / `ArrowDown` | Move selection |
| `Enter` | Open selected note |
| `Delete` or `Ctrl+Backspace` | Delete selected note (with confirmation) |
| `Escape` | Close switcher |

## Terminal mode shortcuts

| Shortcut | Action |
| --- | --- |
| `Ctrl+N` | New note |
| `Ctrl+P` | Open note switcher |
| `Ctrl+S` | Manual save |
| `Ctrl+Q` | Quit |
| `Ctrl+W` | Quit |
| `Ctrl+]` | Navigate wiki link at cursor |
| `Tab` | Accept variable completion or apply calc result |

## Table editing keys (GUI + TUI editor input)

| Shortcut | Action |
| --- | --- |
| `ArrowLeft` / `ArrowRight` | Move within table cell content; cross cells at boundaries |
| `ArrowUp` / `ArrowDown` | Move vertically in the same table column |
| `Ctrl+ArrowLeft` / `Ctrl+ArrowRight` | Jump to previous / next table cell |
| `Shift+Enter` | Split cell into multiline continuation row (`|>`) |
| `Backspace` / `Delete` | Table-aware boundary edit inside current row/cell |
| `Ctrl+Backspace` / `Ctrl+Delete` | Structural table boundary/column edit where applicable |

## Vim mode basics

Vim mode is optional and starts in normal mode when enabled.

- `Esc` to return to normal mode
- `i` to enter insert mode
- `v` for visual mode
- `V` for visual-line mode

Supported and tested core motions/actions include:

- movement: `h`, `j`, `k`, `l`, `w`, `b`, `0`, `$`, `gg`, `G`
- edit actions: `x`, `dd`, `u`, `Ctrl+r`, `o`, `O`, `a`, `A`, `I`
- yank/delete with counts: examples `yy`, `3yy`, `d2w`, `yaw`
- macros (normal + insert flows): `q<register>` start, `q` stop, `@<register>` replay, `3@a` counted replay
- fold toggle: `za`
- wiki-link navigation: `gd`

## Vim command bar entry

- Vim mode: press `:` in normal/visual modes.
- Non-vim editor mode: use `Ctrl+Shift+;`.

For command syntax and aliases, see [Command Reference](./command-reference.md).
