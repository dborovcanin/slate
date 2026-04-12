# Handoff Notes

This document captures what was implemented after milestones M1-M4, plus what should be done next.

## What was polished in this pass

- Added confirmation prompt before note deletion (`Ctrl+Backspace`)
- Added safe async action wrapper for keyboard-triggered commands to avoid unhandled promise rejections
- Made shortcuts case-robust (`Ctrl+Shift+E`, etc.)
- Improved live UI updates:
  - Status/title updates while typing
  - Switcher list refreshes while open as notes change
  - Active edited note moves to top in local state
- Fixed calc annotation race condition (stale async results are dropped and re-evaluated)
- Added IPC socket parent directory creation before bind (more robust fallback path behavior)
- Fixed a critical SQLite deadlock in `save_note` caused by nested mutex locking

## Tests added

### TypeScript (`node --test --experimental-strip-types`)

- `src/state.test.ts`
  - title derivation behavior
  - state updates and note reordering
  - adjacent navigation boundaries
- `src/switcher/fuzzy.test.ts`
  - fuzzy match success/failure behavior
  - fuzzy sort ordering

### Rust (`cargo test`)

- `src-tauri/src/calc/engine.rs`
  - arithmetic evaluation
  - non-expression skipping
  - re-evaluation of lines already containing ` = result`
  - batch line evaluation
- `src-tauri/src/storage/sqlite.rs`
  - CRUD lifecycle
  - most-recent/list ordering by `updated_at`
  - delete idempotency behavior

## Commands

```sh
# Build frontend bundle + typecheck (used by Tauri build)
npm run build

# Run all tests
npm run test

# TS tests only
npm run test:ts

# Rust tests only
npm run test:rust
```

## Known gaps / suggested next work

1. Add a proper confirmation modal (current implementation uses `window.confirm`).
2. Add frontend tests around calc decoration apply behavior and switcher keyboard navigation.
3. Add integration tests for Tauri commands (`notes`, `calc`, `export`) via command layer.
4. Consider moving clipboard export from `navigator.clipboard` to a Tauri clipboard plugin for stricter Linux/WebView reliability.
5. Add CI (`npm run build`, `npm run test`) so regressions are blocked automatically.
