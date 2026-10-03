# MCP Server

`slate mcp` runs a [Model Context Protocol](https://modelcontextprotocol.io) server, so an AI assistant or bot (Claude Code, Codex, opencode, or any MCP client) can find, read, create and edit your notes. It is **off by default**.

The client starts `slate mcp` as a child process and talks to it over stdin/stdout. Nothing listens on the network. The server opens the same database as the editor, so you can keep Slate open while the bot works: an open note picks up the bot's changes on its own (see [Working alongside the editor](#working-alongside-the-editor)).

## Contents

- [Turn it on](#turn-it-on)
- [Connect a client](#connect-a-client)
  - [Claude Code](#claude-code)
  - [Codex](#codex)
  - [opencode](#opencode)
  - [Other clients](#other-clients)
- [Tools](#tools)
- [How writes work](#how-writes-work)
- [Working alongside the editor](#working-alongside-the-editor)
- [What the server cannot do](#what-the-server-cannot-do)
- [Instructions for the bot](#instructions-for-the-bot)
- [Troubleshooting](#troubleshooting)

## Turn it on

1. Set this in `~/.config/slate/config.toml`:

   ```toml
   [mcp]
   enabled = true
   ```

   While `enabled` is `false` (the default), `slate mcp` exits with an error naming the config file, so clients cannot reach your notes until you turn it on.

2. Make sure the client can find `slate`. `make install` puts it in `~/.local/bin/slate`. If that directory isn't on the `PATH` the client sees (desktop launchers and services often have a short `PATH`), use the absolute path in the configs below, for example `/home/you/.local/bin/slate`.

Any client you connect can read and change every note that isn't encrypted. Only connect clients you trust with your notes.

## Connect a client

Every client needs the same thing: run the command `slate` with the argument `mcp`, over stdio.

### Claude Code

```sh
# Available in every project (user scope):
claude mcp add --scope user --transport stdio slate -- slate mcp

# Or only in the current project (local scope, the default):
claude mcp add --transport stdio slate -- slate mcp
```

To share it with a project instead, add it to `.mcp.json` at the project root (or run the command with `--scope project`):

```json
{
  "mcpServers": {
    "slate": {
      "command": "slate",
      "args": ["mcp"]
    }
  }
}
```

Check it with `claude mcp list`, or `/mcp` inside Claude Code.

### Codex

```sh
codex mcp add slate -- slate mcp
```

Or add this to `~/.codex/config.toml`:

```toml
[mcp_servers.slate]
command = "slate"
args = ["mcp"]
```

Check it with `codex mcp list`, or `/mcp` inside Codex.

### opencode

Add this to `opencode.json` (or `opencode.jsonc`) in the project, or to the global config at `~/.config/opencode/opencode.json`:

```json
{
  "$schema": "https://opencode.ai/config.json",
  "mcp": {
    "slate": {
      "type": "local",
      "command": ["slate", "mcp"],
      "enabled": true
    }
  }
}
```

### Other clients

Configure a stdio (or "local") server with the command `slate` and the argument `mcp`. The server supports MCP protocol revisions `2024-11-05` through `2025-11-25` and offers tools only, no resources or prompts. To use a different data or config directory, set `XDG_DATA_HOME` / `XDG_CONFIG_HOME` in the server's environment.

## Tools

| Tool | Arguments | What it does |
| --- | --- | --- |
| `list_notes` | `collection?`, `limit?` (default 50, max 500) | Notes, most recently updated first: `id`, `title`, `revision`, `encrypted` |
| `list_collections` | none | Collections with their note counts |
| `search_notes` | `query`, `collection?`, `limit?` (default 20) | Full-text search; matching lines with a snippet (matches wrapped in `[[ ]]`) and line number. Encrypted notes are not searched |
| `read_note` | `id` | The note's full markdown `text`, `title`, `revision` and `collections` |
| `create_note` | `text`, `collection?` | Creates a note and returns its `id` and `revision`. The first line becomes the title |
| `append_to_note` | `id`, `text` | Adds `text` to the end of the note on a new line. No revision needed |
| `replace_in_note` | `id`, `old_text`, `new_text`, `revision?` | Replaces one exact piece of text. `old_text` must occur exactly once |
| `update_note` | `id`, `text`, `revision` | Replaces the whole text of the note |

Collections are named, not identified by id: `collection: "Work"`. Results come back as JSON, both as text and as structured content.

## How writes work

- **Revisions.** `read_note`, `list_notes` and every write return the note's `revision`. `update_note` needs it, and `replace_in_note` checks it when given. If the note changed since that revision (in the editor, or through another tool call), the write is refused with `the note changed since that revision ... read it again and redo the change`, and nothing is overwritten.
- **Appends** are applied in one step against whatever text is stored, so they need no revision.
- **New notes** get the same per-note modules as notes created in the editor (`[editor.modules]`). If `[editor.security] encrypt_notes = true`, the server needs the password variable (`password_env`, default `SLATE_NOTES_PASSWORD`) in its environment, or `create_note` fails.
- **History.** Bot edits are recorded in [note history](features.md#note-history) like any other save, so `:history` can restore a version from before them.
- **Titles** come from the first line, as in the editor. A note renamed in the browser (`r`) keeps its name.

## Working alongside the editor

While Slate is idle, it checks about once a second (unless `[editor] reload_outside_changes = false`) whether the open note was changed somewhere else. The check covers `slate mcp`, `slate append`, `slate capture`, IMAP sync, another Slate window, and edits to an open markdown file.

- **No unsaved changes:** Slate loads the new text in place. Your cursor and reminders stay on the lines they were on, and the status line shows `note changed outside Slate; reloaded (u undoes)`. The reload is a single undo step.
- **Unsaved changes:** Slate keeps your text and shows `note changed outside Slate (:e! loads it, :w! keeps your version)`. With autosave on, the next autosave stops with a conflict instead of overwriting the bot's change. Use `:e!` to take the stored note (dropping your edits) or `:w!` to keep yours (dropping the bot's).

The check waits while an overlay (command bar, switcher, browser, search, visual selection) is open. Lists such as the note switcher pick up new notes the next time they open.

## What the server cannot do

- **Delete notes.** It has no tool for it.
- **Open encrypted notes.** Encrypted notes, including notes in encrypted collections, are listed (`"encrypted": true`) but cannot be read, searched or written. They can only be unlocked in Slate.
- **Reach files.** Markdown files opened with `slate file.md` aren't available, so a client cannot read or write arbitrary files through the server.
- **Set reminders, rename notes, change modules or manage collections.**

## Instructions for the bot

The server sends short usage instructions to the client when it connects. To make a bot use your notes the way you want, you can also add something like this to its own instructions (`CLAUDE.md`, `AGENTS.md`, or the system prompt):

```markdown
## Notes (Slate MCP server)

- My notes are in Slate, available through the `slate` MCP tools.
- Before writing, find the right note with `search_notes` or `list_notes`, and read it with `read_note`.
- Add to the end of a note with `append_to_note`. Change existing text with `replace_in_note`, using text copied exactly from `read_note`. Use `update_note` only to rewrite a whole note, and pass the `revision` from `read_note`.
- If a write reports that the note changed, read it again and redo the change. Never force it.
- New notes start with a `# Title` line. Put work notes in the `Work` collection.
- Keep the note's existing markdown style: checklists (`- [ ]`), tables, and `name := value` variables are live in Slate.
```

## Troubleshooting

- **`slate mcp: the MCP server is off`**: set `enabled = true` under `[mcp]` in the config file the message names.
- **The client says the server failed to start or exited:** run `slate mcp` yourself. It should wait silently for input. If it prints a config or database error, fix that first. Also check that `slate` is on the client's `PATH`, or use the absolute path.
- **Try it by hand:** errors go to stderr, and stdout carries only protocol messages:

  ```sh
  printf '%s\n' \
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18"}}' \
    '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"list_notes","arguments":{"limit":3}}}' \
    | slate mcp
  ```
