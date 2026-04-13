# Note — Architecture Simplification & Production Hardening Plan

## 1. Context

`note` is a Tauri v2 scratchpad for Linux that ships **two runtimes in one binary**: a Tauri/CodeMirror GUI and a hand-written Rust TUI. It's ~17K LOC today. The codebase is clean and well-tested at the unit level, but three structural problems hold it back from being the minimal, beautiful, efficient tool it wants to be:

1. **The editor engine exists twice.** `src/editor/core/` (TypeScript, 1,348 LoC) and `src-tauri/src/editor_core/` (Rust, 2,176 LoC) implement the same text-rules, context parsing, command dispatch, sum logic, and markdown-table alignment. 95% of the logic overlaps. It has already drifted in 3 functions (`indentOrderedMarker`, `outdentOrderedMarker`, `tableContinuationRule`) and one schema (`CommandExecutionResult` has 2 extra Rust-only fields). The TS half is consumed only by CodeMirror; the Rust half is consumed only by the TUI. Neither half is reachable from the other runtime — they are parallel, not shared.
2. **The TUI re-implements everything the GUI already has.** `terminal/mod.rs` (2,984 lines) + `render.rs` (1,083 lines) contain their own vim state machine, switcher, date picker, command palette, and undo stack — all conceptually identical to the GUI's. Nothing is shared.
3. **Production gates are missing.** No logging, no CI, no CSP, no panic hook, 21 `.unwrap()` sites in production Rust code, hand-synced IPC types that have already drifted, no release workflow, minimal accessibility, no migration framework.

Goal: collapse the duplication, drive both runtimes from a single source-of-truth core, and close the production gaps — without regressing any feature and without turning either runtime into a second-class citizen.

## 2. Target architecture

Single Cargo workspace. One pure-logic `editor-core` crate compiled to both native and wasm32. Both runtimes become thin shells.

```
┌──────────────────────────────────────────────────────────────────┐
│  editor-core (crate, no I/O, no std I/O)                         │
│  text_rules  context  commands  sum  markdown_table              │
│  format      operations  types  vim::state  overlay::state       │
│  ↑ native ↑                                       ↑ wasm32 ↑     │
└──────────────────────────────────────────────────────────────────┘
      │                                                    │
      │ native lib                                         │ wasm-bindgen
      ▼                                                    ▼
┌──────────────────┐   ┌──────────────────┐   ┌──────────────────┐
│   app-core       │   │    note-tui      │   │    note-gui      │
│  storage  calc   │   │ render (ANSI)    │   │ Tauri commands   │
│  config          │   │ input (termios)  │   │ IPC server       │
│  migrations      │   │ adapters→core    │   │ webview shell    │
└──────────────────┘   └──────────────────┘   └──────────────────┘
                                                      │
                                                      ▼
                                              ┌──────────────────┐
                                              │  src/ (vite)     │
                                              │  editor mount    │
                                              │  overlays/       │
                                              │  theme           │
                                              │  ↳ imports wasm  │
                                              └──────────────────┘
```

**Architectural principles:**
- `editor-core` is pure logic. No filesystem, no network, no `std::time` beyond what the calc-free parts need, no globals. Input: `EditorContextSnapshot` + event. Output: `EditOperation` or a state transition. This shape is **already** what the current code produces — the refactor mostly moves files, not logic.
- `app-core` owns all I/O (SQLite, fend, config TOML, XDG paths). Native-only. Both runtime crates depend on it.
- Runtime crates (`note-gui`, `note-tui`) are adapters: they translate keystrokes into snapshots + events, feed them to `editor-core`, and apply the returned operations to their respective views.
- Frontend TS keeps only the plumbing CodeMirror can't give up: decoration/widget construction, calc result rendering, DOM event precedence — everything else calls into WASM.

**Why WASM wins here**: the editor-core functions are called at rule-execution points (Enter, Tab, arrow, tab-navigation, format command, sum command), **not** on every keystroke. Each call is tiny — a `{text, selection, changedRange}` snapshot in and an `EditOperation` out. WASM boundary cost is negligible at this rate. Rendering-rate TS code (CodeMirror decorations, typing) stays in TS. We get Rust's single-source-of-truth without paying the IPC roundtrip on every character.

## 3. Target repository layout

```
/                                 (vite root, package.json, tsconfig, vite.config.ts)
  Cargo.toml                      (workspace manifest — replaces src-tauri/Cargo.toml)
  migrations/
    0001_init.sql                 (moved from src-tauri/migrations)
  src/                            (TS frontend, now much smaller)
    main.ts  app.ts  api.ts  state.ts
    editor/
      editor.ts                   (CodeMirror mount, orchestration only)
      wasm.ts                     (NEW: wasm init + typed re-exports)
      calc-decoration.ts          (slimmed, parser/commit logic moved to core)
      markdown/
        tokens.ts                 (NEW: split from markdown-decoration.ts)
        decorations.ts            (NEW)
        rich-text.ts              (NEW)
      variable-autocomplete.ts
      vim.ts                      (thin adapter, ~250 lines)
      markdown-editing.ts
    overlays/
      overlay.ts                  (NEW: unified modal factory)
      switcher.ts                 (~80 lines, adapter)
      command-picker.ts           (~80 lines, adapter)
      date-picker.ts              (~120 lines, adapter)
    theme/
    styles/
    ui/
      toast.ts                    (NEW: error toast)
      escape.ts                   (NEW: centralized HTML escape)
  crates/
    editor-core/                  (NEW — pure logic, dual-target)
      Cargo.toml                  (crate-type = ["lib", "cdylib"])
      src/
        lib.rs
        context.rs
        text_rules.rs
        commands.rs
        sum.rs
        markdown_table.rs
        format.rs                 (extracted from commands.rs)
        operations.rs
        types.rs
        vim/
          mod.rs                  (state machine: mode, count, operator, register)
          motion.rs                (hjkl, w, b, gg, G, 0, $, %, f/F, t/T)
          action.rs                (d, c, y, p, x, o, O, a, A, I)
        overlay/
          mod.rs                  (generic modal: filter, cursor, submit)
          switcher.rs
          command_picker.rs
          date_picker.rs
        wasm.rs                   (#[cfg(target_arch = "wasm32")] wasm-bindgen glue)
    app-core/                     (NEW — native-only I/O)
      Cargo.toml
      src/
        lib.rs
        storage/
          mod.rs  sqlite.rs  models.rs  migrations.rs
        calc/
          mod.rs  engine.rs
        config/
          mod.rs
    note-gui/                     (Tauri shell, replaces src-tauri/)
      Cargo.toml
      tauri.conf.json             (with CSP, see §6)
      capabilities/default.json
      build.rs                    (tauri-specta type generation)
      src/
        main.rs  lib.rs
        commands/
          notes.rs  calc.rs  export.rs  config.rs  log.rs
        ipc/server.rs
    note-tui/                     (NEW crate — extracted terminal app)
      Cargo.toml
      src/
        main.rs  lib.rs
        app.rs                    (orchestration, autosave)
        render.rs                 (hand-written ANSI, kept)
        input.rs                  (termios raw mode, escape parsing)
        adapters.rs               (translate TUI events → editor-core state)
    note-msg/                     (CLI IPC client)
      Cargo.toml
      src/main.rs
  pkg/                            (wasm-bindgen output, .gitignore)
  docs/
    ARCHITECTURE.md               (NEW)
    CONTRIBUTING.md               (NEW)
    CHANGELOG.md                  (NEW, Keep-a-Changelog)
    variables.md                  (existing)
  .github/workflows/              (NEW)
    ci.yml
    release.yml
```

Rationale: a single workspace is the only way to share `editor-core` and `app-core` cleanly across three binaries without publishing them to crates.io. `src-tauri/` as a directory disappears — Tauri just becomes `crates/note-gui/`.

## 4. Module-level simplifications (concrete)

Each item is a win on its own. LoC estimates are net deltas after all changes.

### 4.1 Collapse TS editor/core/ → WASM shim
- **Current**: `src/editor/core/{text-rules, context, commands, sum, markdown-table, operations, types}.ts` = 1,348 lines.
- **After**: `src/editor/wasm.ts` (~120 lines: wasm init, typed re-exports, snapshot adapter) + small `src/editor/core/codemirror-adapter.ts` (keep, CodeMirror-specific glue).
- **Net savings**: ~1,150 lines of TS deleted; zero logic change; zero test regression (existing `text-rules.test.ts` becomes `editor-core` Rust tests we already have + wasm-bindgen-test for target parity).

### 4.2 Split `markdown-decoration.ts` (927 → ~650 lines)
- Move tokenizer and line-classifier logic into `editor-core::markdown_tokens` (shared with text-rules regexes that already duplicate the same patterns).
- Split TS file into `src/editor/markdown/{tokens,decorations,rich-text}.ts`.
- Net savings: ~280 lines of TS, plus the tokenizer becomes available to the TUI.

### 4.3 Slim `calc-decoration.ts` (855 → ~500 lines)
- Parser, commit-mark diff, incremental plan → `editor-core::calc_render`.
- CodeMirror state-field, widgets, effects, scheduling stay in TS (CM-specific).
- Net savings: ~350 lines of TS; keeps decoration fidelity; commit-mark logic reused in TUI when it gets calc ghosts.

### 4.4 Unify overlays (742 → ~280 lines)
- Current: `src/switcher/switcher.ts` (192), `src/editor/command-picker.ts` (236), `src/editor/date-picker.ts` (314) each reimplement: open/close, input field, filtered list, keyboard nav, Esc handling, focus handling. 742 lines combined.
- Build `src/overlays/overlay.ts` — one factory: `createOverlay({title, input, items, renderItem, onSelect, keybindings, onClose}) → { open, close, update }`.
- Each specific overlay becomes a wiring file (~80-100 lines).
- Net savings: ~400 lines of TS, plus ARIA compliance applied once in the factory.
- **Mirror in Rust**: `editor-core::overlay` state machine (filter + cursor + items). Both the GUI overlays and the TUI overlays now drive from the same filter logic and keymap definitions.

### 4.5 Unify vim state machines
- GUI `src/editor/vim.ts` (596 lines) and TUI's vim logic inside `terminal/mod.rs` (~400 lines scattered) are independently built. They drift; they don't share tests.
- Extract `editor-core::vim`: `VimState`, `on_key(key, snapshot) -> VimStep { ops, mode, status }`. Pure state + operation output, no I/O.
- GUI `vim.ts` becomes a CM adapter (~220 lines) translating key events into `VimState::on_key` calls and dispatching returned operations as CodeMirror transactions.
- TUI vim wiring becomes a ~150-line adapter.
- Shared state machine in Rust: ~550 lines.
- Net savings: ~270 lines, plus drift elimination, plus motion/operator coverage can be expanded once and both runtimes benefit.

### 4.6 Move the embedded markdown formatter
- `editor_core/commands.rs` has ~100 lines of markdown formatter (headings, lists, tables) that exists only in Rust. GUI's `:format` command goes through a TS round-trip.
- Move to `editor-core::format::markdown_format(text) -> String`. Both runtimes call it; GUI's `:format` flows through WASM.
- Net savings: 0 LoC but fixes a silent drift risk and unifies behavior.

### 4.7 Reify sum as a first-class module
- Current: `sum.ts` exists as a proper TS module (148 lines); Rust inlines the same logic inside `execute_command`'s `Sum` branch.
- Extract `editor-core::sum::{resolve_scope_range, execute_sum}` so both runtimes call the same function and the TUI can use sum independently.
- Net savings: ~50 lines post-dedup, plus correctness parity.

### 4.8 Delete dead files
- `src-tauri/src/bin/scratch.rs` (13 lines — confirmed dev scratch pad calling `render::render_line` with a canned string).
- `src-tauri/src/terminal_stub.rs` (17 lines) and the `#[cfg(not(unix))]` branch in `lib.rs:7-11` — TUI goes Unix-only; Windows gets GUI only (the current Tauri build already targets Windows via MSVC/NSIS and never exercised the stub in practice).
- IPC Windows boilerplate inside `ipc/server.rs` (gate the whole module on `#[cfg(unix)]`).
- Net savings: ~40 lines + one cfg tree simplified.

### 4.9 Replace hand-synced IPC types with `tauri-specta`
- Annotate every Tauri command with `#[specta::specta]`; emit `src/bindings.ts` from a `build.rs`.
- Delete hand-written `Note`, `ThemeConfig`, `NoteEvaluationResult`, `CommandExecutionResult`, `VariableIndexEntry` interfaces from `src/api.ts`.
- `CommandExecutionResult` schema drift (`clipboard_text`, `quit_requested` Rust-only) is fixed mechanically.
- Net savings: ~60 lines + zero drift risk forever.

### Total module-level reduction
Rough net: **~2,300 lines of duplicated/parallel code eliminated**, traded for ~800 lines of new shared Rust + ~200 lines of wiring. Net LoC saving ~1,300, but the *ratio of source-of-truth logic* jumps from ~30% shared to ~85% shared. That's the real win.

## 5. TUI deep refactor (Phase 5 scope)

`terminal/mod.rs` (2,984) + `render.rs` (1,083) = 4,067 lines today.

**Strategy**: extract into `crates/note-tui/`, move all state management into `editor-core`, keep hand-written ANSI rendering. Do **not** adopt ratatui — it would break the minimal-binary goal.

**Decomposition**:
- `crates/note-tui/src/app.rs` — app struct: current note id, undo stack, autosave timer, dirty flag. Event loop: reads input from `input.rs`, converts to `editor-core` events, dispatches, applies returned operations to its `Vec<String>` buffer, paints via `render.rs`. Target ~550 lines.
- `crates/note-tui/src/render.rs` — kept almost as-is. One clean pass to extract duplicated ANSI-escape helpers, remove dead markdown rendering duplicated with editor-core tokens. Target ~900 lines (down from 1,083).
- `crates/note-tui/src/input.rs` — termios raw mode setup, escape-sequence parser (`ESC[A`, etc.), mouse ignored. Target ~200 lines.
- `crates/note-tui/src/adapters.rs` — `EditorCoreAdapter` trait impls that translate TUI state ↔ `editor-core::EditorContextSnapshot`. Target ~150 lines.
- State that moves into `editor-core`: vim state (~450 lines), overlay state machines for switcher/command-picker/date-picker (~400 lines), mode transitions (~100 lines). Combined new in editor-core: ~950 lines.

**Post-refactor**:
- `crates/note-tui/` ~1,800 lines.
- `editor-core` gains ~950 shared lines.
- **Net: 4,067 → 2,750** lines in TUI territory, half of which is now shared with the GUI.

Binary size impact: neutral (no new deps). Startup: unchanged.

## 6. Production hardening

Numbered items, each with severity/effort/target files. Check off in Phase 1 and Phase 6.

### Critical (must land before v0.2)

1. **Panic hook + tracing (3h)** — Add `tracing` + `tracing-subscriber` to workspace. Install `std::panic::set_hook` in both `note-gui/src/main.rs` and `note-tui/src/main.rs` that writes backtrace + build version to `$XDG_STATE_HOME/note/panic.log`. `env_logger` style filtering via `NOTE_LOG=debug`.
2. **Replace `Mutex::lock().unwrap()` (2h)** — 5 sites in `app-core/src/storage/sqlite.rs`. Convert to `?` with a `StorageError` enum via `thiserror`. On poisoned mutex, log and return structured error; don't crash the process.
3. **TS invoke wrapper + toast (4h)** — `src/api.ts` gets `invokeSafe<T>(cmd, args): Result<T, NoteError>`. Every call site wrapped. Error → toast via new `src/ui/toast.ts` (~60 lines, accessible live region). Global unhandled rejection handler in `main.ts`.
4. **CSP (30m)** — `crates/note-gui/tauri.conf.json` gains `"security": { "csp": "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; connect-src ipc: https://ipc.localhost; img-src 'self' data:;" }`. Verify fonts still load; narrow `unsafe-inline` if possible after CM inspection.
5. **HTML escape audit (30m)** — `highlightPositions` in `switcher.ts:168` already per-char-escapes but uses `innerHTML`. Replace with `replaceChildren()` building real `<b>` nodes. Same treatment for `date-picker.ts:180` (`grid.innerHTML = ""` → `grid.replaceChildren()`). Centralize `escapeHtml` in `src/ui/escape.ts`.
6. **GitHub Actions CI (3h)** — `.github/workflows/ci.yml`: `cargo fmt --check`, `cargo clippy --workspace -- -D warnings`, `cargo test --workspace`, `npm ci && npm run build && npm run test:ts`. Linux-only matrix for now. Gate on PR + push to `main`.

### High (first release)

7. **tauri-specta migration (4h)** — per §4.9. Do it per-command, not big-bang, to avoid runtime surprises.
8. **Single version source (1h)** — Drop `version` from `package.json`; generate `src/version.ts` from `Cargo.toml` at build time via a tiny vite plugin or a pre-build `node` script.
9. **Migration runner (3h)** — `app-core/src/storage/migrations.rs`: reads `migrations/*.sql`, writes to `schema_version` pragma, forward-only. Current `0001_init.sql` is grandfathered.
10. **CHANGELOG.md + release workflow (3h)** — Keep-a-Changelog format. `.github/workflows/release.yml` tag-triggered (`v*`), runs `cargo build --release`, bundles `.deb` + `.AppImage` + Windows `.exe`, uploads to GH Releases. Drop the unsigned warning — document code-signing as a separate future item.
11. **ARCHITECTURE.md + CONTRIBUTING.md (2h)** — Capture the workspace layout, the dual-runtime shape, "how to add an editor rule", "how to add a command". Short documents, not epics.

### Medium (v0.3)

12. **Accessibility pass (4h)** — `role="dialog"` + `aria-modal="true"` on the overlay factory; focus trap (save `document.activeElement` on open, restore on close); `role="listbox"` + `aria-selected` on overlay items; `aria-live="polite"` on toast. Manual test with Orca.
13. **IPC integration tests (6h)** — `tests/integration/` using `tauri::test::mock_builder` for each Tauri command. TUI integration test that pipes a scripted key sequence into `app::run` and asserts the output buffer.
14. **Logging abstraction (2h)** — `src/logger.ts` exposing `logger.{info,warn,error}`; errors invoke a new `log_error` Tauri command that appends to the same `panic.log` file so GUI + TUI + panics all land in one place.
15. **tauri-plugin-updater (2h)** — Opt-in via config flag; updater server = GitHub Releases. Disabled by default for homelab users.

### Nice-to-have

16. **Windows code signing** — Document the procedure; defer the certificate.
17. **Coverage tooling** — `cargo-tarpaulin` + `c8`, report on CI.
18. **Performance profiling** — `puffin` for Rust hot paths if any post-WASM regression shows up.

**Total hardening effort**: ~40 hours concentrated in Phase 1 and Phase 6.

## 7. Implementation phasing

Seven phases, each independently shippable. Nothing in phase N breaks phase N-1.

### Phase 1 — Foundation (1 week)
Workspace manifest migration (`Cargo.toml` at root, `crates/note-gui/` replaces `src-tauri/`), no behavior change. Items 1, 2, 3, 4, 5, 6 from hardening. Delete `bin/scratch.rs`, `terminal_stub.rs`. No architecture churn yet — this phase ships logging, error handling, CI, CSP.

**Exit criteria**: `cargo test --workspace` green, `npm test` green, CI workflow green on main, panic log verified, invoke errors produce a toast in manual test.

### Phase 2 — Extract `app-core` + `editor-core` (native only) (1 week)
Create `crates/editor-core/` and `crates/app-core/`. Move `src-tauri/src/editor_core/` → `crates/editor-core/` (native-only still). Move `src-tauri/src/storage/`, `src-tauri/src/calc/`, `src-tauri/src/config/` → `crates/app-core/`. Wire both into `note-gui` and add placeholder dep from `note-tui` (still housed under `src-tauri/src/terminal/` at this stage). Fix the 3 drifted functions (`indentOrderedMarker`, `outdentOrderedMarker`, `tableContinuationRule`) and the `CommandExecutionResult` schema drift while we're there. Extract the 100-line markdown formatter to `editor-core::format`.

**Exit criteria**: Rust tests pass, GUI runs, TUI runs, nothing visibly different to users.

### Phase 3 — WASM target + TS deletion (1.5 weeks)
Add `wasm-bindgen = "0.2"` + `wasm-bindgen-test` dev-dep to `editor-core`. Add `#[cfg(target_arch = "wasm32")] mod wasm;` exposing: `run_doc_change_rules`, `run_enter_rules`, `run_tab_rules`, `run_table_cell_navigation_rules`, `list_command_suggestions`, `execute_command`, `markdown_format`, `sum_execute`, plus tokenizer exports for markdown-decoration. `Makefile` gets a `wasm` target: `cargo build -p editor-core --target wasm32-unknown-unknown --release && wasm-bindgen --target web --out-dir pkg target/wasm32-unknown-unknown/release/editor_core.wasm && wasm-opt -Oz pkg/editor_core_bg.wasm`. Vite imports the `pkg/` via `vite-plugin-wasm` + `vite-plugin-top-level-await`.

Create `src/editor/wasm.ts` init shim with `console_error_panic_hook` in dev. Delete `src/editor/core/text-rules.ts`, `context.ts`, `commands.ts`, `sum.ts`, `markdown-table.ts`, `operations.ts`, `types.ts`. Update all call sites (edit `editor.ts:15`, `calc-decoration.ts`, `command-picker.ts`, `markdown-editing.ts`, `vim.ts`, etc.) to import from `./wasm`. Port TS-side tests to exercise WASM rather than the deleted TS modules (or delete them where Rust coverage suffices).

Measure WASM size with `wasm-opt -Oz`. Target <400KB. If overshoot, switch `regex` → `regex-lite` inside `editor-core` (no Unicode classes needed for our patterns).

**Exit criteria**: `src/editor/core/` deleted (except `codemirror-adapter.ts` which is pure CM glue). GUI functional, all shortcuts work, all tests green. WASM bundle measured and documented in ARCHITECTURE.md.

### Phase 4 — Overlay unification (3 days)
Build `src/overlays/overlay.ts` and `editor-core::overlay`. Rewrite `switcher.ts`, `command-picker.ts`, `date-picker.ts` as ~80-120 line adapters. Accessibility work (item 12) lands in this phase because it's cheap once the factory exists. Add `src/ui/toast.ts` and `src/ui/escape.ts` here as reusable UI utilities.

**Exit criteria**: 3 overlays open/close/filter/select correctly in manual test. Orca reads "dialog" + "listbox" + item labels.

### Phase 5 — TUI deep refactor (2 weeks)
Create `crates/note-tui/`. Move `src-tauri/src/terminal/mod.rs` and `render.rs` in, split mod.rs into `app.rs`, `input.rs`, `adapters.rs`. Extract vim state machine + overlay state machines into `editor-core::vim` and `editor-core::overlay` (GUI picks them up simultaneously — rewrite `src/editor/vim.ts` as a thin adapter). Add a small test harness in `crates/note-tui/tests/` that feeds scripted key sequences and asserts screen output.

Gate the `note-tui` crate on `#[cfg(unix)]`. Drop `terminal_stub.rs` dependency permanently.

**Exit criteria**: TUI launches with `note --terminal`, all keybindings work, vim mode works, undo works, switcher works, date picker works. GUI vim mode still passes all existing tests (because it now shares the state machine).

### Phase 6 — Production gates (1 week)
Items 7, 8, 9, 10, 11, 14 from hardening. `tauri-specta` migration, version unification, migration runner, CHANGELOG, ARCHITECTURE.md, CONTRIBUTING.md, centralized logger.

**Exit criteria**: `src/api.ts` has zero hand-written Rust-mirror interfaces. `CHANGELOG.md` populated back to 0.1.0. Docs published in-repo.

### Phase 7 — Release workflow + updater (3 days)
Item 10's release workflow activates: tag a `v0.2.0-rc.1`, watch GH Actions build + upload artifacts, manual smoke-test the artifacts, cut `v0.2.0`. Land `tauri-plugin-updater` wiring behind an opt-in config flag.

**Exit criteria**: A tag push produces attached GH release binaries; updater flows for opted-in users.

**Total schedule**: ~6.5 weeks of focused work, landing continuously. Not a big-bang.

## 8. Risks and mitigations

- **WASM binary overhead.** Editor-core with regex could weigh 400-600KB gzipped. Mitigation: compile with `wasm-opt -Oz`, `-C panic=abort`, `-C opt-level=z`, strip debuginfo in release. Fallback: swap to `regex-lite` and/or hand-roll the 3 simple regexes used by text-rules.
- **Debugger friction in webview.** `console_error_panic_hook` covers panics; source maps via `wasm-bindgen --debug` cover dev. Ship a DWARF-enabled dev wasm; production has no source maps.
- **Incremental calc correctness.** Moving parts of `calc-decoration.ts` into core risks breaking the ghost-mark/commit-mark diff logic. Mitigation: port current TS tests into `wasm-bindgen-test` **before** deleting the TS code. If a test can't be ported, leave the code in TS.
- **Vim parity regressions.** Unifying the state machine is the single most error-prone step. Mitigation: record the current behaviour via golden-output tests (keystroke script → buffer state) **before** starting Phase 5. Freeze them. Both GUI and TUI must pass the same suite post-refactor.
- **Workspace migration churn.** Moving files invalidates every cargo cache and editor ctags. Mitigation: do it in one commit, squash, land in Phase 1 when there's the least risk of losing in-flight work.
- **Tauri-specta wire-format churn.** Migrating all IPC types at once could hide a type mismatch. Mitigation: command-by-command.
- **Dropping Windows TUI.** Tauri GUI on Windows still works (MSVC/NSIS build); only the terminal entry becomes Unix-only. Document in CHANGELOG.

## 9. Critical files to be modified

- `/home/dusan/job/note/src-tauri/src/editor_core/text_rules.rs` (1,215 L) — moves to `crates/editor-core/src/text_rules.rs`; becomes primary source of truth.
- `/home/dusan/job/note/src-tauri/src/editor_core/commands.rs` (513 L) — split: dispatch stays, embedded formatter extracted.
- `/home/dusan/job/note/src-tauri/src/editor_core/context.rs` (393 L) — moves; Rust tests consolidated.
- `/home/dusan/job/note/src-tauri/src/terminal/mod.rs` (2,984 L) — extracted to `crates/note-tui/src/{app,input,adapters}.rs`; vim and overlay state machines hoisted into `editor-core`.
- `/home/dusan/job/note/src-tauri/src/terminal/render.rs` (1,083 L) — moves to `crates/note-tui/src/render.rs`; cleanup pass only.
- `/home/dusan/job/note/src-tauri/src/lib.rs` (253 L) — becomes `crates/note-gui/src/lib.rs`; Windows stub branches removed.
- `/home/dusan/job/note/src-tauri/tauri.conf.json` — moves + CSP added.
- `/home/dusan/job/note/src/editor/editor.ts` (253 L) — imports switch to `./wasm`, `runTableCellNavigationRules` comes from WASM.
- `/home/dusan/job/note/src/editor/core/` (1,348 L) — **deleted** except `codemirror-adapter.ts`.
- `/home/dusan/job/note/src/editor/markdown-decoration.ts` (927 L) — split into 3 files, tokenizer moved to WASM.
- `/home/dusan/job/note/src/editor/calc-decoration.ts` (855 L) — parser + commit-mark logic moved to WASM.
- `/home/dusan/job/note/src/editor/vim.ts` (596 L) — reduced to ~220-line CM adapter.
- `/home/dusan/job/note/src/switcher/switcher.ts` (192 L) + `src/editor/command-picker.ts` (236 L) + `src/editor/date-picker.ts` (314 L) — rewritten as ~80-120 line adapters on shared overlay factory.
- `/home/dusan/job/note/src/api.ts` — interfaces deleted, bindings re-exported from `src/bindings.ts` (tauri-specta output).
- `/home/dusan/job/note/src-tauri/src/storage/sqlite.rs` — `.unwrap()` removed, `StorageError` enum added.
- `/home/dusan/job/note/src-tauri/src/bin/scratch.rs` — **deleted**.
- `/home/dusan/job/note/src-tauri/src/terminal_stub.rs` — **deleted**.
- New: `Cargo.toml` (workspace), `crates/editor-core/Cargo.toml`, `crates/app-core/Cargo.toml`, `crates/note-gui/Cargo.toml`, `crates/note-tui/Cargo.toml`, `crates/note-msg/Cargo.toml`, `.github/workflows/{ci,release}.yml`, `docs/{ARCHITECTURE,CONTRIBUTING,CHANGELOG}.md`, `src/editor/wasm.ts`, `src/overlays/overlay.ts`, `src/ui/{toast,escape}.ts`.

## 10. Verification plan

End-to-end validation after each phase. Not just unit tests — we want to know features still feel right.

**Automated gates (run every phase):**
- `cargo fmt --check && cargo clippy --workspace -- -D warnings && cargo test --workspace` — green.
- `npm run build && npm run test:ts` — green.
- From Phase 3 onward: `make wasm && du -sh pkg/editor_core_bg.wasm` — budgeted ≤400 KB post-opt.

**Manual GUI smoke suite (after each phase that touches GUI):**
1. `cargo tauri dev`, wait for HMR.
2. Create a note, type a markdown table, verify auto-alignment fires on `|` and `Tab` between cells.
3. Type `200 * 1.19` at end of line, expect ghost `238`, press Tab, verify inline commit.
4. Declare `rate := 0.19` and `price := 100`, type `price * (1 + rate)`, verify `119`.
5. `Ctrl+P`, fuzzy-filter, select — switcher works with keyboard.
6. `Ctrl+Shift+;`, run `:sum list` on a list — sum inserts and clipboard set.
7. Toggle vim mode in config, restart, verify `i/esc/hjkl/yy/dd/u` + `:date`.
8. Trigger a save failure (e.g., kill db file between ops), verify toast error (post-Phase 1).
9. Check view source: CSP header present in response (post-Phase 1).

**Manual TUI smoke suite (after Phase 5):**
1. `note --terminal` in a real terminal.
2. Type the markdown table case — verify auto-alignment (uses shared editor-core rules).
3. `:sum list` command — same result as GUI.
4. Vim mode: `i/esc/hjkl/yy/p/u/.` — golden-output test passes.
5. Switcher `Ctrl+P` — overlay renders, filter, select.
6. Force a panic (debug build feature flag) — verify panic log written to `$XDG_STATE_HOME/note/panic.log`.

**Release validation (Phase 7):**
1. Tag `v0.2.0-rc.1` on a branch, watch GH Actions build, download the Linux artifact, run it, complete the GUI smoke suite.
2. Open a new note, close the app, reopen — last note restored, autosave intact.
3. Verify CHANGELOG entries match the shipped behavior.

## 11. Out of scope (explicitly deferred)

Listed to prevent scope creep:
- Rewriting the `fend-core` calc engine or adding new math features.
- Plugins / extensions system.
- Cloud sync, accounts, or any network feature.
- Mobile (Android/iOS Tauri targets).
- Rich-text WYSIWYG beyond the current decoration layer.
- Full syntax highlighting for fenced code blocks beyond what's already classified in `markdown-decoration.ts`.
- Windows TUI (remains GUI-only).
- Per-note history / time-travel snapshots.
- Internationalization.

---

**Summary of net impact:**
- ~2,300 lines of duplicated/parallel code eliminated.
- 1 workspace, 5 crates, 1 TS frontend — clean separation.
- 85% of editor logic is single-source (up from ~30%).
- Both GUI and TUI drive from the same text-rules, vim, and overlay state machines.
- Production gates: panic hook, tracing, CI, CSP, error toast, typed IPC, migration runner, release workflow.
- Estimated timeline: ~6.5 weeks of focused work, landing in seven independently-shippable phases.
