# Slate

A minimal, fast, keyboard-first note-taking app for the terminal. Inspired by [Antinote](https://antinote.io/).

Open fast, type, close. Notes are autosaved locally. No accounts, no cloud, no bloat.

## Documentation

Detailed project documentation is organized in [`docs/`](docs/README.md):

- [Feature Guide](docs/features.md) (all features with their keys and commands)
- [How Slate Works](docs/how-slate-works.md)
- [Keymaps](docs/keymaps.md)
- [Command Reference](docs/command-reference.md)
- [Configuration](docs/configuration.md)
- [Architecture](docs/architecture.md)
- [Variables Specification](docs/variables.md)
- [Export Reference](docs/export.md)

Roadmap and planning docs are in [`roadmap/`](roadmap/).

## Features

Grouped by area, with the keys and commands for each. Commands run in the command bar (`:` in Normal mode, `Ctrl+E` while editing). Details for every item: [Feature Guide](docs/features.md).

### Notes and saving

| Feature | Keys / commands |
| --- | --- |
| New note, fuzzy switcher, full-text content search | `Ctrl+N`, `Ctrl+P`, `Tab` inside the switcher |
| Delete a note | `Delete` in the switcher |
| Autosave (background, idle + on switch/quit), manual save | automatic, `Ctrl+S`, `:w`, `:w!` |
| Quit | `Ctrl+Q`, `:q`, `:wq` |
| Open a markdown file as a note (saved back to the file) | `slate path/to/file.md` |
| Collections and a session working collection | `Ctrl+G`, `:collection create/choose/join/leave/update/delete/purge` |
| Collection browser: collections, notes and a preview side by side; copy, move, rename, create, delete and search note text | `Ctrl+B`, `-` (Normal), `:browse`; `Ctrl+F` / `Ctrl+/` inside |

### Capture and daily notes

| Feature | Keys / commands |
| --- | --- |
| Today's daily note from a template | `slate today`, `:today` |
| Quick capture of a timestamped `- HH:MM` entry | `slate capture buy milk` |
| Capture command output through a pipe | `cmd \| slate capture` |
| Append piped output to a note | `cmd \| slate append [--id <note-id>]` |
| Paste every new clipboard text at the cursor | `:clip-watch on`, `:clip-watch off` |

### Editing

| Feature | Keys / commands |
| --- | --- |
| Styled-in-place markdown (headings, lists, quotes, code, links, emphasis) | always on (`style` module) |
| List continuation, checklists, checklist auto-reorder | `Enter`, `- [ ]`, trailing ` /x` |
| Inline formatting on a selection | `:bold`, `:italic`, `:strike`, `:icode`, `:format clear` |
| Convert lines to heading / checklist / bullets / numbers | `:title`, `:clist`, `:ulist`, `:olist` |
| Format the whole note | `:format` (or `format_on_save`) |
| In-note search | `/`, `Ctrl+F`, `n` / `N` |
| Folding | `za`, `:fold`, `:unfold`, `:fold-toggle` |
| Insert a date, set a line reminder (desktop notification) | `:date`, `:remind`, `:remind toggle` |
| Soft wrap for prose; tables and code scroll | `[editor] wrap` |

### Vim mode and macros

| Feature | Keys |
| --- | --- |
| Normal / insert / visual / visual-line modes | `Esc`, `i a I A o O`, `v`, `V` (`vim_mode = true` starts in Normal) |
| Motions with counts | `h j k l w b 0 $ gg G gj gk` |
| Operators and text objects | `d y c` + `w b e 0 $ t{c}`, `iw aw i\| a\| i( i[ i{ i" i*` ... |
| Edit, paste, undo | `x`, `dd`, `yy`, `cc`, `C`, `p`, `u`, `Ctrl+R` |
| Macros: record, stop, replay, counted replay | `q{r}`, `q`, `@{r}`, `3@{r}` |

### Links and images

| Feature | Keys / commands |
| --- | --- |
| Wiki links `[[SHORTID]]`, `[[SHORTID#Heading\|Title]]` with note and heading autocomplete | type `[[`, then `#` for headings |
| Follow a link | `Ctrl+]`, `gd` |
| Preview the linked note | `K` |
| Images `![alt](./assets/x.png)`, imported by pasting an image path | paste |
| Image preview (sixel / kitty / iTerm2 / half-blocks) | `gx` (Normal), `Ctrl+O` (vim mode off); `o` opens the system viewer |
| Web search with results you can open or insert as links | `?`, `:web <query>`; `Enter` open, `Shift+Enter` insert |

### Calculations, variables and tables

| Feature | Keys / commands |
| --- | --- |
| Ghost results for math lines (units, percentages, [fend](https://github.com/printfn/fend)) | automatic; `Tab` applies the result |
| Note-local variables with autocomplete | `name := expression` |
| Variables from other notes | `[[SHORTID]].name` |
| Sum / average a paragraph, list, table row/column, or the note | `:sum`, `:avg`, `:sum list`, `:avg row`, `:sum column`, `:sum doc` |
| Tables that align as you type, with cell navigation | `\|`, `Tab` / `Shift+Tab`, `Ctrl+Arrow`, `Shift+Enter` for multiline cells |
| Table formulas and cell references | `:=sum_col()`, `:=avg_row() * 2`, `:=(1,2) + (2,2)` |
| Per-note modules | `:module status`, `:module math\|table\|variables\|style\|cross_note on\|off\|toggle` |

```
200 * 1.19              → 238
50 kg to lbs            → 110.231 lb
sqrt(144) + 3^2         → 21
rate := 1.19
200 * rate              → 238
```

### Security, export and sync

| Feature | Keys / commands |
| --- | --- |
| App-level lock (plaintext at rest) / encryption at rest | `:note lock <pw>`, `:note encrypt <pw>`, `:note unprotect <pw>` |
| Export to PDF, markdown or text (file or clipboard) | `:export pdf <path>`, `:export md [path]`, `:export txt [path]` |
| Full database backup and restore | `:backup export <path.zip>`, `:backup load <path.zip>` |
| IMAP mail into daily inbox notes (one-off or background) | `slate imap-sync`, `[imap] auto_sync_on_startup = true` |
| Themes (14 color schemes, live config reload) | `[theme] color_scheme`, `accent` |

**Storage**
- SQLite with WAL mode in `~/.local/share/slate/notes.db`
- Config file is auto-generated on first run (`~/.config/slate/config.toml`)

## Requirements

- Linux (primary target) or macOS
- Rust toolchain (stable)
- Optional: `notify-send` for reminder/IMAP notifications
- Optional: Node.js, only for the perf check scripts in `scripts/`

## Build and install

```sh
make build      # cargo build --release -p slate
make install    # installs ~/.local/bin/slate, desktop entry, icon
```

The binary is `target/release/slate`.

Development:

```sh
cargo run -p slate                  # run the terminal app
cargo run -p slate -- --list        # list notes
cargo build -p slate --no-default-features   # build without IMAP
```

### Arch AUR build scripts

See [`packaging/aur/README.md`](packaging/aur/README.md). `slate-git` builds from source; `slate-bin` installs the prebuilt Linux binary published by CI.

## Testing

```sh
make test       # cargo test --workspace
make perf       # startup + table perf checks (requires node; honors [perf].enabled)
```

## Configuration

See [docs/configuration.md](docs/configuration.md) for all keys. Minimal example:

```toml
[theme]
color_scheme = "gruvbox-light"

[editor]
autosave = true
vim_mode = false

[editor.modules]
math = true
table = true
variables = true
style = true
```

## Architecture

```
crates/
  editor-core/   # editing semantics: vim stepping, commands, markdown/table/list rules, folding, calc planning
  app-core/      # SQLite storage, note sources, calc engine, cross-note index, config
  tui/           # terminal app (package `slate`)
    src/
      lib.rs         # CLI parsing and mode selection (terminal, append, imap-sync)
      terminal/      # editor runtime, rendering, input, overlays
      commands/      # export (md/txt/pdf) and backup
      imap.rs        # IMAP sync (feature `imap`)
      bin/           # perf probes (note-startup, table-perf, perf-config)
```

Editing semantics live in `editor-core`; the terminal app is an input/rendering layer. See [docs/architecture.md](docs/architecture.md).

## License

TBD
