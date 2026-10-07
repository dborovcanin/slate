# Scripts and shortcuts

Register local executables in `~/.config/slate/config.toml`. Scripts run only
when invoked; there are no hooks or plugin runtime. Use trusted executables:
they run with your user permissions, including filesystem and network access.

```toml
[scripts.uppercase]
argv = ["python3", "/absolute/path/to/slate/scripts/examples/uppercase.py"]
input = "selection"
output = "replace-selection"
timeout_seconds = 30

[scripts.meeting]
argv = ["python3", "/absolute/path/to/slate/scripts/examples/meeting.py"]
input = "none"
output = "insert"

[keybindings.visual]
"<C-r>" = "run uppercase"

[keybindings.normal]
"<Space>m" = "run meeting 'Project X'"

[keybindings.editor]
"<C-r>" = "run meeting 'Project X'"
```

Configuration is loaded when Slate starts. Restart after changing it.
Executables use normal PATH lookup. Other paths use the process working
directory; `~`, environment variables, globs and shell substitutions are not
expanded. Script names contain ASCII letters, digits, `_` or `-`.

## Editor commands

- `:run uppercase`: transform the visual selection.
- `:run meeting "Project X"`: insert a template at the invocation cursor.
- `:run-cancel`: cancel the running script.

In non-Vim mode, open the command bar with the usual command-bar shortcut.
Only one script runs at a time. Input is `none` (empty text), `selection`
(requires a selection), or `note` (current buffer, including unsaved edits).
Output is `insert` (at the original cursor), `replace-selection` (requires a
selection), or `message` (status only). Insert leaves selected text intact;
choose replacement explicitly when needed.

Edits form one undo step. If text changes while a script runs, the result is
discarded. Switching/reloading notes cancels the run, including switching back
to the same note. Results wait while a dialog or command bar is open. Locked
and read-only notes cannot be passed to scripts. An unlocked protected note
can be passed explicitly, just like other readable text.

Arguments support single/double quotes, empty quoted arguments and backslash
escapes (except within single quotes). No shell expansion occurs. Arguments
are appended individually to the configured `argv`, and also included in JSON.
Command names are case insensitive; script names and arguments retain their case.

## CLI

```sh
slate run meeting 'Project X'
printf 'hello' | slate run uppercase
slate run --id <note-id> uppercase
slate run --file ./note.md uppercase
```

An input-bearing script reads piped stdin unless an explicit target precedes
the script name. Both `selection` and `note` input use the supplied CLI text;
the CLI has no visual selection. `input = "none"` needs no input and rejects
note/file targeting. Flags after the script name belong to the script.

The CLI writes response text to stdout without adding a newline and any
response message to stderr. It **does not save or modify notes**, regardless
of the configured editor output mode. This makes pipelines predictable;
`slate run meeting 'Project X' | slate capture` captures a generated template.
Protected notes cannot be read by CLI scripts.

## JSON protocol v1

Slate writes one UTF-8 JSON object to stdin and closes it. A script may
exit without reading it:

```json
{"version":1,"args":["Project X"],"text":"selected or note text","note_id":"n1"}
```

`note_id` is null when CLI input is piped or absent. A script writes one JSON
object to stdout and exits successfully:

```json
{"text":"replacement or generated text","message":"optional status"}
```

`text` is required, including for a status-only script. Put diagnostic logging
on stderr; stdout must contain only the JSON response. Unknown response fields
are rejected. Nonzero exit status, malformed JSON, timeout, cancellation, and
input/output limits leave the editor buffer unchanged.

Requests are limited to 16 MiB of encoded JSON; stdout and stderr each to
4 MiB. The default timeout is 30 seconds, configurable from 1 to 3600. Script
execution and pipe I/O run outside the terminal input/render loop. Preparing
input and applying edits still cost time proportional to the relevant text;
large whole-note scripts are best invoked deliberately. On Unix, cancellation
and timeout terminate the script process group, including ordinary descendants.
On other platforms only the direct child is terminated. Detached processes
are the script author's responsibility.

## Keybindings

Modes are `normal`, `editor` (insert/non-Vim editing), and `visual` (including
visual-line mode). Values are Slate commands, including `run` and `run-cancel`.
Bindings operate on editor input, not dialogs or the command bar. Normal-mode
bindings apply between Vim commands; pending operators/counts receive their
motion keys unchanged. Explicit
bindings override built-in keys; use that deliberately (the examples replace
Ctrl+R in the configured modes).

Sequences contain literal characters and tokens: `<Space>`, `<C-x>`, `<Esc>`,
`<Enter>`, `<Tab>`, `<Backspace>`, `<Delete>`, `<Up>`, `<Down>`, `<Left>`, `<Right>`,
`<Home>`, `<End>`, `<lt>` (literal `<`). For example `<Space>m` means Space, then
m. There is no implicit leader setting.

Sequences have 1–8 keys; at most 128 bindings are supported. Overlapping
prefixes in the same mode are rejected, including alternate spellings of the
same keys. Unmatched sequences are replayed through normal input handling.
Incomplete prefixes replay after 900 ms; Escape cancels a pending prefix.
Avoid ordinary typing prefixes in `editor` mode unless that delay is wanted.
