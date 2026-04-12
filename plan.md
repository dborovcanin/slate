# Plan: Linux Scratchpad v1 — Tauri Architecture

## Context

Building a minimal, fast, keyboard-first Linux scratchpad app inspired by [Antinote](https://antinote.io/). The original plan proposed Rust + egui. During review, we identified egui's text editing quality as an existential risk for an app whose core value is "type fast." Decision: **switch to Tauri v2** to get browser-grade text editing, accepting the ~200-400ms startup trade-off.

Key decisions from review:
- **Stack:** Tauri v2 (Rust backend + web frontend)
- **Calc UX:** Ghost annotations to the right of each line, Tab to apply result inline
- **Export:** Ctrl+E copies to clipboard, Ctrl+Shift+E opens file save dialog

---

## Architecture

```
                    local IPC (Unix domain socket)
┌─────────────┐  show|hide|toggle|ping   ┌──────────────────────────────────┐
│  note-msg   │ -----------------------> │           note (Tauri v2)        │
│   CLI       │                          │                                  │
└─────────────┘                          │  ┌──────── WebView ────────────┐ │
                                         │  │  Vanilla TS + CodeMirror 6  │ │
                                         │  │  • Editor with line decors  │ │
                                         │  │  • Calc ghost annotations   │ │
                                         │  │  • Fuzzy switcher overlay   │ │
                                         │  │  • Keyboard shortcut layer  │ │
                                         │  └─────────────────────────────┘ │
                                         │  ┌──────── Rust core ─────────┐ │
                                         │  │  Tauri commands (IPC)      │ │
                                         │  │  rusqlite + WAL storage    │ │
                                         │  │  fend-core calc engine     │ │
                                         │  │  Unix socket IPC server    │ │
                                         │  │  Platform (paths, app_id)  │ │
                                         │  └─────────────────────────────┘ │
                                         └──────────────────────────────────┘
```

### Why Tauri v2

- Browser-grade text editing via CodeMirror 6: undo/redo, IME, accessibility, Wayland clipboard — all solved
- CodeMirror's line decoration API is purpose-built for the calc ghost annotation UX
- Tauri v2 is stable, has good Wayland support via WebKitGTK, and the `app_id` / window management APIs are mature
- Rust backend keeps storage, calc, and IPC in a fast single-language layer
- Cross-platform (Linux/Windows/macOS) with no extra effort

### Why vanilla TypeScript (no framework)

- Zero framework overhead — no runtime, no build abstraction, smallest possible bundle
- The app has ~4 UI pieces (editor, switcher, status bar, shell). Manual DOM wiring is manageable at this scale.
- CodeMirror handles the editor's reactivity internally — the framework's job would mainly be toggling the switcher overlay and syncing note list state
- Simpler dependency tree, no framework version churn
- Direct DOM control makes debugging and performance tuning straightforward

### Why CodeMirror 6 (not raw textarea)

- The calc ghost annotation requires positioning results next to specific lines — CodeMirror's `Decoration.widget` and `StateField` handle this natively
- With a textarea, we'd need a fragile overlay div mirroring text positions
- CodeMirror also gives: proper multi-level undo/redo, search/replace, configurable keybindings, IME handling, accessibility
- Only import what we need: ~80-100KB for a plain-text setup (no syntax highlighting, no language modes)

---

## Folder Structure

```
note/
  src-tauri/
    Cargo.toml
    tauri.conf.json
    capabilities/
      default.json
    src/
      lib.rs                    # Tauri setup, plugin registration
      commands/
        mod.rs
        notes.rs                # CRUD, list, search Tauri commands
        calc.rs                 # evaluate_line command
        export.rs               # export_clipboard, export_file commands
      storage/
        mod.rs
        sqlite.rs               # connection, migrations, WAL setup
        models.rs               # Note struct, queries
      calc/
        mod.rs
        engine.rs               # fend-core wrapper
      ipc/
        mod.rs
        server.rs               # Unix socket listener for note-msg
    src/bin/
      note-msg.rs               # CLI binary: ping|show|hide|toggle
    migrations/
      0001_init.sql
  src/
    main.ts                     # Entry point, mount app, global keybindings
    app.ts                      # Root shell: layout, shortcut dispatcher, view routing
    editor/
      editor.ts                 # CodeMirror setup and lifecycle
      extensions.ts             # CM extensions (keybindings, autosave hook)
      calc-decoration.ts        # StateField + Decoration for ghost annotations
    switcher/
      switcher.ts               # Modal fuzzy search overlay (show/hide/filter)
      fuzzy.ts                  # Simple fuzzy match scorer
    api.ts                      # Typed wrappers around Tauri invoke()
    state.ts                    # App state: activeNote, noteList, event emitter
    styles/
      app.css
      editor.css
      switcher.css
      theme.css                 # CSS variables for colors, spacing
  index.html
  package.json
  vite.config.ts
  tsconfig.json
```

---

## Database Schema

```sql
-- migrations/0001_init.sql
CREATE TABLE notes (
    id TEXT PRIMARY KEY,            -- ULID (sortable, no coordination needed)
    body TEXT NOT NULL DEFAULT '',
    created_at TEXT NOT NULL,        -- ISO 8601
    updated_at TEXT NOT NULL         -- ISO 8601
);

CREATE INDEX idx_notes_updated ON notes(updated_at DESC);
```

Title is derived from first non-empty line of `body` — no separate column.

---

## Keyboard Shortcuts (v1)

| Shortcut        | Action                          |
| --------------- | ------------------------------- |
| Ctrl+N          | New note                        |
| Ctrl+P          | Open fuzzy switcher             |
| Ctrl+E          | Export to clipboard (markdown)  |
| Ctrl+Shift+E    | Export to file (.md / .txt)     |
| Ctrl+W          | Hide window                     |
| Ctrl+Backspace  | Delete current note (confirm)   |
| Escape          | Close switcher / cancel         |
| Tab (on calc)   | Apply calc result inline        |
| Ctrl+↑/↓       | Previous/next note              |

---

## Calc UX: Ghost Annotation Model

```
User types:     200 * 1.19
Editor shows:   200 * 1.19                → 238       (dimmed, right-aligned)
User presses:   Tab (when cursor on that line)
Result:         200 * 1.19 = 238                      (annotation removed, text inserted)

Non-calc line:  todo: buy groceries
Editor shows:   todo: buy groceries                   (no annotation)
```

**Implementation:**
1. CodeMirror `ViewPlugin` watches document changes (debounced ~100ms)
2. For each line, send content to Rust backend via `invoke("evaluate_line", { text })`
3. Backend tries `fend_core::evaluate(text)` — returns `Some(result)` or `None`
4. Frontend updates a `StateField<Map<lineNumber, string>>` with results
5. `Decoration.widget` renders ghost text after each line with a result
6. Tab keymap checks if current line has a result, inserts ` = {result}` at end of line

**Batching:** Send all visible lines in one `invoke("evaluate_lines", { lines: string[] })` call, not one call per line.

---

## Milestones

### M1 — Scaffold + Single Note Editor
- Initialize Tauri v2 project with vanilla TS (no framework)
- Set up Rust backend: rusqlite storage, migrations, WAL mode
- CodeMirror 6 editor with plain-text config
- Single hardcoded note: type, autosave (debounce 500ms + flush on blur/close), restore on startup
- Window config: utility window, fixed initial size, `app_id = "note"` for Sway
- **Acceptance:** Open app, type text, close, reopen — content persists. Text editing feels native (undo, redo, selection, clipboard all work).

### M2 — Multi-Note + Switcher
- Note CRUD in Rust backend (create, list, delete, get)
- Note list state in frontend, derived titles from first line
- Fuzzy switcher overlay (Ctrl+P): text input filters notes, Enter selects, Escape closes
- Ctrl+N creates new note and focuses editor
- Ctrl+↑/↓ for sequential navigation
- **Acceptance:** Create 5+ notes, fuzzy search by content, switch between them. Keyboard-only workflow.

### M3 — Inline Calc
- `fend-core` integration in Rust backend
- `evaluate_lines` Tauri command with batch evaluation
- CodeMirror decoration plugin for ghost annotations
- Tab to apply result
- **Acceptance:** Type `200 * 1.19` — see `→ 238` ghost text. Tab inserts `= 238`. Type `50 kg to lbs` — see conversion. Non-calc lines show nothing.

### M4 — Export + IPC + Polish
- Export: Ctrl+E copies note as markdown to clipboard, Ctrl+Shift+E opens native file dialog
- IPC: Unix domain socket server in Rust, `note-msg` CLI binary
- Commands: `ping`, `show`, `hide`, `toggle`
- Sway integration: document `bindsym` config for toggle
- Visual polish: consistent spacing, focus states, transitions, theme variables
- **Acceptance:** Full keyboard workflow end-to-end. `note-msg toggle` works from Sway keybind.

---

## Dependencies

### Rust (src-tauri/Cargo.toml)
- `tauri` v2 — app framework
- `rusqlite` (bundled feature) — SQLite with WAL
- `fend-core` — calc engine
- `ulid` — sortable unique IDs
- `serde`, `serde_json` — serialization for Tauri commands
- `tokio` (via Tauri) — async runtime for IPC server
- `directories` — XDG paths

### Frontend (package.json)
- `@codemirror/state`, `@codemirror/view`, `@codemirror/commands` — editor core
- `@tauri-apps/api` — Tauri frontend bindings
- `vite` — build tooling
- `typescript` — dev dependency

---

## Top Risks

1. **Tauri startup time on Linux.** WebKitGTK cold start can be 300-500ms. Mitigation: keep the frontend bundle tiny (vanilla TS, no framework), lazy-load non-critical modules, measure actual startup on target machine early in M1.

2. **WebKitGTK availability and version.** Arch Linux keeps WebKitGTK current, but other distros may lag. Mitigation: v1 targets Arch/Sway only. Document minimum WebKitGTK version.

3. **Calc evaluation latency.** Sending every line change to Rust for evaluation could feel laggy. Mitigation: batch all visible lines in one invoke call, debounce at 100ms, show results only for stable lines.

4. **CodeMirror bundle size.** Full CM6 can be large. Mitigation: import only `@codemirror/state`, `@codemirror/view`, `@codemirror/commands` — skip language modes, search, lint, autocomplete. Target <100KB.

5. **IPC race: note-msg before app is ready.** Mitigation: `note-msg` connects to socket with short timeout, returns nonzero if no server. Sway config uses `exec note || note-msg toggle` pattern.

6. **Scope creep around Antinote features.** Mitigation: v1 is notes + search + calc + export. Variables, reactive calc, themes, auto-paste — all deferred.

---

## Verification (end-to-end)

1. `cargo tauri dev` — app window appears, cursor in editor
2. Type paragraphs of text, undo/redo, clipboard paste — all work natively
3. Close window, reopen — content is intact (autosave)
4. Ctrl+N → type in new note → Ctrl+P → fuzzy search → switch back
5. Type `200 * 1.19` → ghost shows `→ 238` → Tab → line becomes `200 * 1.19 = 238`
6. Type `50 kg to lbs` → ghost shows `→ 110.231 lb`
7. Ctrl+E → paste somewhere → note content is there
8. Ctrl+Shift+E → file dialog → save → verify file
9. From terminal: `note-msg ping` → success
10. `note-msg toggle` → window hides → `note-msg toggle` → window shows
11. Sway: `bindsym $mod+n exec note-msg toggle || note` → works from any workspace
