# WASM Plugin System Design

Status: draft architecture proposal
Scope owner: backend/runtime + command surface, with shared-core integration points

## 1) Goal

Enable third-party extensibility with one plugin model that works in both GUI and TUI, without moving core editing semantics out of shared core.

Primary outcomes:

- same plugin behavior in GUI and TUI
- no per-keystroke plugin execution in hot editing paths
- bounded, capability-scoped side effects
- deterministic, reviewable command/transform behavior

## 2) Non-goals

- plugins do not replace vim stepping, motion semantics, undo model, or markdown/table/list core rules
- plugins do not run arbitrary host code
- plugins do not get unrestricted filesystem/network access
- plugins do not introduce frontend-specific behavior forks

## 3) Layer ownership

Shared core (`crates/editor-core`):

- remains canonical for editing semantics
- owns edit-operation model and command normalization/parsing contracts
- receives plugin-produced operations and applies them like any other command result

Plugin runtime (`crates/app-core` new `plugins` module):

- plugin discovery, manifest validation, lifecycle
- WASM sandbox execution, capability checks, resource limits
- plugin command registry and hook dispatch

Frontends (GUI/TUI):

- render plugin command suggestions and statuses
- invoke shared host command/plugin dispatch path
- never execute plugin semantics directly in frontend-specific code

## 4) Runtime architecture

### 4.1 Package layout

Plugin bundle directory:

```text
<plugin-id>/
  plugin.toml
  plugin.wasm
  README.md (optional)
  LICENSE (optional)
```

`plugin.toml` fields:

- `id`: globally unique, kebab-case (`acme.wordcount`)
- `name`
- `version` (semver)
- `api_version` (plugin host API compatibility)
- `entry` (default `plugin.wasm`)
- `commands[]` (id, aliases, description, modes)
- `hooks[]` (opt-in: `on_note_open`, `before_write`, `after_write`)
- `capabilities` (explicit permission list)
- `limits` (optional tighter limits than host defaults)

### 4.2 Discovery paths

- user plugins: `$XDG_CONFIG_HOME/slate/plugins/` (or platform equivalent)
- workspace plugins (optional, off by default): `<repo>/.slate/plugins/`

Load order:

1. user plugins
2. workspace plugins (if enabled)

Conflict policy:

- duplicate `id` -> highest precedence source wins, others disabled with warning
- command alias collisions -> builtin command wins, plugin alias disabled

### 4.3 Execution engine

Use a native WASM runtime in Rust host (recommended: `wasmtime` with component model when feasible).

Host hard limits:

- max module size (for example 2 MB compressed / 8 MB raw)
- max memory pages per plugin instance
- max CPU/fuel per invocation
- max wall-clock timeout per hook/command

Each invocation returns structured result:

- success payload
- recoverable plugin error
- trap/timeout/permission denied (host converts to safe error message)

## 5) Plugin API contract (v1)

Prefer a WIT world for ABI stability. Keep the initial surface intentionally small.

### 5.1 Core data types

- `EditorSnapshot`
  - `text: string`
  - `selection: { anchor: u32, head: u32 }`
  - `mode: enum { vim, editor }`
  - `note_meta: { id, title, source_kind }`
- `EditOperation` (same semantic shape as shared core operation)
- `CommandResult`
  - `message: string`
  - `operations: list<EditOperation>`
- `HookResult`
  - `continue: bool`
  - `message?: string`

### 5.2 Guest exports

- `plugin_init() -> PluginInfo`
- `list_commands() -> list<CommandSpec>`
- `run_command(command_id, snapshot, args) -> CommandResult`
- `on_note_open?(snapshot) -> HookResult`
- `before_write?(snapshot) -> HookResult`
- `after_write?(snapshot) -> HookResult`

### 5.3 Host imports

- `log(level, message)` (rate-limited)
- `now_ms()` (optional; deterministic mode can stub/freeze)
- `read_setting(key)` (read-only plugin config)
- `request_capability(capability)` (returns denied unless pre-granted)

No direct fs/network imports in v1. Side effects go through explicit host ops gated by manifest capabilities.

## 6) Capability model

Default policy: deny all optional capabilities.

Initial capabilities:

- `clipboard.read`
- `clipboard.write`
- `note.read_metadata`
- `note.read_body` (active note only)
- `note.write_edits` (only through returned operations)
- `export.write_file` (host-validated path policy)
- `http.fetch` (off by default; allowlist domains if granted)

Permission UX:

- first use prompt with exact capability + plugin id
- one-shot allow, always allow, deny
- stored in user config with revocation UI/command

## 7) Command and hook integration

### 7.1 Command dispatch

Command resolution order:

1. builtin commands (existing `editor-core` catalog)
2. plugin commands (`<plugin-id>:<command>` canonical form)
3. unique alias fallback (only if no builtin collision)

Plugin commands are classified as host dispatch (`HostPlugin`) and executed by runtime layer. Resulting `EditOperation`s are applied by existing adapter pipelines.

### 7.2 Hook policy

Allowed hooks are non-hot-path lifecycle events only:

- note open
- before write
- after write

Not allowed in v1:

- per keystroke hooks
- cursor-move hooks
- render-frame hooks

This preserves responsiveness and avoids semantic drift between frontends.

## 8) Performance strategy

- preload manifests/index at startup, lazy-load module bytes on first use
- compile cache keyed by `(plugin_hash, runtime_version, api_version)`
- keep one warm instance per active plugin, reset state between calls when needed
- batch plugin command suggestions (single host call for list)
- strict timeout/fuel defaults to prevent UI/TUI stalls

Targets:

- zero impact on typing path p95
- command invocation overhead acceptable for explicit user actions

## 9) Parity and correctness

Parity requirement: plugin command on same snapshot must produce the same `CommandResult` in GUI and TUI.

Test plan:

- plugin host conformance tests (manifest, capability denial, timeout)
- cross-frontend parity fixtures for plugin command execution
- malicious plugin fixtures (infinite loop, oversized output, invalid operation ranges)
- regression tests for command collision and fallback behavior

## 10) Error handling

Failure isolation rules:

- plugin panic/trap only fails that invocation
- plugin can be auto-disabled after N consecutive hard failures
- host reports concise message and records detailed diagnostics in logs
- no plugin failure may corrupt note state or crash editor session

## 11) Minimal implementation plan

Phase 1: Runtime skeleton

- add `app-core::plugins` with manifest loader + validator
- add plugin registry + command index
- add `HostPlugin` command dispatch path in host command layer
- add no-op capability gate and timeout wrappers

Phase 2: ABI + command execution

- define v1 WIT contract and guest SDK helpers
- run `run_command` and map result to existing `EditOperation`
- GUI/TUI both use same host dispatch entrypoint

Phase 3: Hooks + permissions

- implement `before_write` / `after_write` hooks
- add capability prompt + persisted grants
- add plugin management commands (`:plugin list`, `:plugin enable`, `:plugin disable`)

Phase 4: hardening

- compile cache, structured diagnostics, auto-disable policy
- conformance and parity suites in CI
- documentation and sample plugins

## 12) Architectural decisions and tradeoffs

Decision: execute plugins in Rust host runtime, not frontend JS.

- Why: keeps one canonical execution environment for GUI and TUI; avoids duplicating sandbox logic.

Decision: plugins return edit operations, not direct text mutation APIs.

- Why: preserves existing undo/redo and operation pipeline semantics.

Decision: disallow per-keystroke hooks in v1.

- Why: protects hot-path performance and keeps behavior deterministic.

Decision: builtin commands retain precedence over plugin aliases.

- Why: avoids regressions and command-surface instability.

## 13) Open questions

- Should workspace plugins be enabled by default or opt-in per workspace?
- Do we require plugin signing in v1, or defer to v2 after capability model is stable?
- Should network capability be entirely postponed to v2?
- Should plugin state persistence be supported in v1 (`plugin kv`) or deferred?
