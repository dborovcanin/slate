# Slate

A minimal, fast, keyboard-first note-taking app for the terminal. Inspired by [Antinote](https://antinote.io/).

Open fast, type, close. Notes are autosaved locally. No accounts, no cloud, no bloat.

## Documentation

Detailed project documentation is organized in [`docs/`](docs/README.md):

- [How Slate Works](docs/how-slate-works.md)
- [Feature Guide](docs/features.md)
- [Keymaps](docs/keymaps.md)
- [Command Reference](docs/command-reference.md)
- [Configuration](docs/configuration.md)
- [Architecture](docs/architecture.md)
- [Variables Specification](docs/variables.md)
- [Export Reference](docs/export.md)

Roadmap and planning docs are in [`roadmap/`](roadmap/).

## Features

**Editor**
- Full-screen terminal editor with note switcher and status bar
- Autosave (idle flush + save on exit/switch) and restore of the last-open note
- Multi-theme color schemes with live config reload (default: `gruvbox-light`)
- Command bar (`:`) for calc/date/reminder/list/module/fold/export/note-security flows (full list + aliases: `docs/command-reference.md`)
- Per-note modules (`math`, `table`, `variables`, `style`), persisted with each note
- Markdown-style rich editing: live styling for headings, quotes, lists, inline code, links, bold/italic/strike while keeping raw markdown editable
- Checklist markdown (`- [ ]`, `- [x]`), optional checklist auto-reorder, ` /x` toggle suffix
- List continuation on Enter, markdown table autoformat/alignment, table-aware cursor movement
- Wiki links with autocomplete, collapsed rendering, `Ctrl+]` / `gd` navigation, and `K` preview
- Open markdown files directly: `slate path/to/file.md` (`.md`, `.markdown`, `.mdown`, `.mkd`), saved back to the file
- Pipe append: `cmd | slate append` (or `cmd | slate append --id <note-id>`)
- Daily notes and quick capture: `slate today` / `:today`, `slate capture buy milk`, `cmd | slate capture`

**Vim mode (optional)**
- Enable with `[editor] vim_mode = true`
- Normal/insert/visual/visual-line/visual-block modes, counts, text objects
- Macros (`q<register>`, `@<register>`, counted replay), `Q` register summary
- Folding (`za`, `:fold`, `:unfold`, `:fold-toggle`)
- Ex commands: `:w`, `:wq`, `:q`, `:sum`, `:avg`, `:date`, `:format`, `:clist`, `:ulist`, `:olist`, `:module ...`, and more

**Inline calculations**
- Type a math expression and see the result as a ghost annotation to the right of the line
- Press Tab to apply the result inline
- Uses note-context evaluation with line-aware extraction (normal lines, list bodies, single calc table cell)
- In markdown tables, lists, and checklists, calc applies to the expression part and `Tab` replaces that expression in place
- Table formula cells use a leading `:=` marker: `:=sum_col()`, `:=avg_row() * 2`, or `:=tax_rate * subtotal`
- `name := expression` remains the variable assignment syntax; inside a table cell, `:=expression` without a name is a formula marker, not a variable definition
- Supports arithmetic, unit conversions (`50 kg to lbs`), percentages, and everything [fend](https://github.com/printfn/fend) can evaluate
- Non-math lines are ignored — no noise
- Date-like lines (`YYYY-MM-DD`, `DD.MM.YYYY`, `MM/DD/YYYY`) are ignored to avoid false numeric ghost suggestions

**Variables (Antinote-style, note-local)**
- Define variables with `:=` assignment syntax: `name := expression`
- Variable names are case-insensitive and may include spaces (letters/digits/underscore/space)
- Variables are note-local and reactive: dependent lines recompute as definitions change
- Conversion assignments are normalized to numeric values for reuse (`len := 3 m to km`, then `len + 2`)
- Assignment lines do not render calc ghosts
- Unresolved/cyclic variable expressions fail silently (no ghost noise)
- Variable autocomplete popup appears while typing (default after 3 chars); `Tab` accepts completion when popup is open, otherwise `Tab` applies calc ghost
- Full behavior contract: `docs/variables.md`

```
200 * 1.19              → 238
50 kg to lbs            → 110.231 lb
sqrt(144) + 3^2         → 21
```

**Wiki links**
- Syntax: `[[01HX4VHR]]`, `[[01HX4VHR#Heading]]`, `[[01HX4VHR|Display Title]]`, `[[01HX4VHR#Heading|Display Title]]`
- Short ids are 8-character alphanumeric prefixes of note ids
- Navigation with `Ctrl+]` or vim `gd`
- Broken links are rendered distinctly and resolve automatically when notes become available

**Multiple notes**
- Create, switch, and delete notes with keyboard shortcuts
- Fuzzy search switcher (`Ctrl+P`) and collection picker (`Ctrl+G`)
- Titles derived from the first non-empty line

**Note protection**
- `note lock` requires a password to open a note in the app, but the note body remains plaintext in SQLite
- `note encrypt` requires a password and stores the note body encrypted at rest
- Password arguments for note-security commands are redacted from command history entries

**Export and backup**
- `export pdf <path>`, `export md [path]`, `export txt [path]` (`md`/`txt` without a path export to clipboard)
- Markdown-aware PDF output (headings, lists, code blocks, tables, images, checklists)
- Full database backup/restore: `backup export <path.zip>`, `backup load <path.zip>`

**IMAP email sync (special notes)**
- Run sync once: `slate imap-sync`
- Enable background polling in app sessions: set `[imap] auto_sync_on_startup = true`
- Pulls from configured IMAP folder and appends messages to daily notes (`inbox-email-YYYY-MM-DD` by default)
- Works with any provider exposing IMAP over TLS
- Uses incremental UID checkpointing to avoid reprocessing old messages
- Fetches newest messages first by default
- First sync defaults to recent mail only (`[imap] initial_sync_past_days = 1`) to avoid flooding
- Shows a system notification when background sync appends new emails

**Keyboard shortcuts**

| Shortcut | Action |
| --- | --- |
| `Ctrl+N` | New note |
| `Ctrl+P` | Fuzzy note switcher |
| `Ctrl+G` | Collection picker |
| `Ctrl+S` | Manual save |
| `Ctrl+Q` / `Ctrl+W` | Quit |
| `Ctrl+]` | Navigate wiki link at cursor |
| `Tab` | Accept variable completion or apply calc result |

Full list: [docs/keymaps.md](docs/keymaps.md).

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
