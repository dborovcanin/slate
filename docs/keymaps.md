# Keymaps

This document summarizes terminal and vim key mappings.

## Switcher keys

| Key | Action |
| --- | --- |
| `ArrowUp` / `ArrowDown` | Move selection |
| `Enter` | Open selected note |
| `Delete` or `Ctrl+Backspace` | Delete selected note (with confirmation) |
| `Escape` | Close switcher |

## Collection picker keys

| Key | Action |
| --- | --- |
| `ArrowUp` / `ArrowDown` | Move selection |
| `Enter` | Set active working collection |
| `Ctrl+E` | Edit selected collection (name/description/default tags) |
| `Escape` | Close picker |

## Global shortcuts

| Shortcut | Action |
| --- | --- |
| `Ctrl+N` | New note |
| `Ctrl+P` | Open note switcher |
| `Ctrl+G` | Open collection picker |
| `Ctrl+S` | Manual save |
| `Ctrl+Q` | Quit |
| `Ctrl+W` | Quit |
| `Ctrl+]` | Navigate wiki link at cursor |
| hover wiki link | Preview linked note (popup) |
| `Tab` | Accept variable completion or apply calc result |

## Table editing keys

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
- screen-row movement on soft-wrapped lines: `gj`, `gk` (with counts); in insert mode the arrow keys move by screen row on wrapped lines
- edit actions: `x`, `dd`, `cw`, `cc`, `C`, `u`, `Ctrl+r`, `o`, `O`, `a`, `A`, `I`
- yank/delete with counts: examples `yy`, `3yy`, `d2w`, `yaw`
- macros (normal + insert flows): `q<register>` start, `q` stop, `@<register>` replay, `3@a` counted replay
- macro pending cancel: `Esc` cancels pending register input after `q` or `@`
- macro status summary: `Q` (normal mode) prints recorded macro registers + step counts
- undo/redo: `u` / `Ctrl+r`
- fold toggle: `za`
- wiki-link navigation: `gd`
- wiki-link preview (peek linked note): `K` (toggle; dismissed on cursor move)

## Vim command bar entry

- Vim mode: press `:` in normal/visual modes.

For command syntax and aliases, see [Command Reference](./command-reference.md).
