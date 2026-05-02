# UI Perf Tracing Metrics

This document defines reusable tracing conventions for UI performance metrics.

## Collector

UI tracing uses `src/perf/editor-profiler.ts`.

- Enable: `:perf on`
- Disable: `:perf off`
- Status: `:perf status`
- Dump top buckets: `:perf dump`
- Log location hint: `:perf where`

`perf dump` writes a compact report through `append_startup_log` in `ui_perf` mode.

## Metric Naming

Use `component.operation.phase` naming:

- `editor.autosave.snapshot`
- `editor.autosave.snapshotWait`
- `editor.autosave.saveNote`
- `editor.autosave.flush`

Rules:

1. Prefix with owner (`editor`, `markdown`, `calc`, `wasm`).
2. Keep operation stable across implementations (`autosave`, `decorations`).
3. Use phase suffixes for breakdowns (`snapshot`, `saveNote`, `flush`).
4. Do not encode dynamic values in metric names; place them in `metrics`.

## Reasons

Use `reason` for categorical branch labels only:

- `inline`, `background`, `cache`
- `autosave_success`, `autosave_error`
- `manual_success`, `manual_error`
- `sync`, `sync_wasm_not_ready`

Rules:

1. Keep reason vocabulary small and explicit.
2. Do not put note IDs or free-form text in `reason`.

## Numeric Payload (`metrics`)

Put dynamic numbers in `metrics`, e.g.:

- `docLength`
- `lineCount`
- `bodyLength`
- `chunkLines`
- `forceWrite` (`0 | 1`)
- `staleDuringSave` (`0 | 1`)

Rules:

1. Metrics must be numeric and finite.
2. Prefer bytes/counts/booleans as `0|1`.
3. Keep key names stable for trend comparisons.

## Autosave Flow Metrics

Current autosave instrumentation captures:

1. `editor.autosave.snapshot`
   - Time to materialize save payload.
   - Reasons: `inline`, `background`.
2. `editor.autosave.snapshotWait`
   - Time spent waiting for snapshot availability.
   - Reasons: `cache`, `inline`, `background`.
3. `editor.autosave.saveNote`
   - Backend save RPC duration.
   - Reasons: `autosave`, `manual`.
4. `editor.autosave.flush`
   - End-to-end flush duration.
   - Reasons: `autosave_success`, `autosave_error`, `manual_success`, `manual_error`.

## Baseline Workflow

1. Enable profiler: `:perf on`
2. Reproduce scenario (e.g. 100k-line note typing + autosave idle).
3. Dump report: `:perf dump`
4. Store output in perf notes/PR description.
5. Compare p95/p99 and metric maxima before vs after changes.

## 100k-Line Autosave Targets

Scenario:

1. Open a plain-text note with ~100,000 lines.
2. Make 30 localized edits (single-line inserts/deletes).
3. Let autosave flush after each edit burst.
4. Run `:perf dump` and capture autosave buckets.

Before capture:

- Use the first run on a branch as the `before` baseline.
- Record p95 for:
  - `editor.autosave.snapshotWait`
  - `editor.autosave.saveNote`
  - `editor.autosave.flush`

After targets:

1. `editor.autosave.snapshotWait` p95: `<= 20ms` and `>= 40%` lower than before.
2. `editor.autosave.flush` p95: `<= 80ms` and `>= 25%` lower than before.
3. `editor.autosave.saveNote` p95: `<= 60ms` and no worse than `+10%` vs before.
4. `editor.autosave.snapshot` reason mix for 100k docs: `inline` should be `0` samples; use `background`/`cache`.

## TUI Runtime Tracing

TUI now has reusable runtime tracing buckets via command bar:

- `:perf on` / `:perf off` / `:perf status`
- `:perf dump [top]`
- `:perf clear`
- `:perf cap <n>`
- `:perf where`

Dump output is appended to `note-startup-tui_perf.log` in temp dir.

Current core buckets:

- `tui.render.frame` (`draw`)
- `tui.key.dispatch` (`input`)
- `tui.idle.dispatch` (`autosave_tick`)
- `tui.vim.step` (`no_intent|unhandled|movement|mutating`)
- `tui.doc_change_rules` (`noop|applied`)
- `tui.enter_rules` (`noop|applied`)
- `tui.tab_rules` (`noop|applied`)
- `tui.calc.recompute` (`math_disabled|stale_full|incremental`)
- `tui.calc.viewport_eval` (`eval`)
- `tui.save` (`noop|normal|forced`)
