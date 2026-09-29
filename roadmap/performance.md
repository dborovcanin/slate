# Performance Baselines

## Scope

This file is an operational/measurement reference.
Execution backlog ownership for large-note performance lives in `roadmap/plan.md` (Performance Backlog).


This repository stores simple startup performance baselines in:

- `perf/baselines/startup.json`

## Record a new baseline

```sh
node scripts/startup-record.mjs
```

This runs the native probe binary (`note-startup tui`) and writes median timings for each startup mark.

## Check for regressions

```sh
node scripts/startup-check.mjs
```

The check fails when a measured startup mark regresses more than the configured threshold (`threshold_pct`, default 15%).

## Large-note latency

```sh
cargo test --release -p slate --lib large_note_perf -- --ignored --nocapture
```

Generates a note (prose, images, tables with formulas, variables with duplicate assignments and chapter-long running totals, wiki links and cross-note references to two linked notes), then times navigation, typing, structural edits, undo/redo, search, open and idle ticks. Each sample is one action's key handling plus the repaint after it. The test fails when a p95 exceeds its limit in `perf/baselines/large_note.json`; `node scripts/perf-check.mjs` runs it in CI.

CI checks 30k and 100k lines. `SLATE_LARGE_NOTE_SIZES=200000,400000` measures other line counts; sizes without limits are reported only. Autosave and the first dependency index build run off the input thread; the remaining large-note costs at 100k lines are opening the note (~100 ms), the first search (~50 ms) and the per-edit index sync on the idle tick (~20 ms).

## Runtime Tracing

Use the in-editor profiler for interaction latency and payload metrics.

- Enable with command: `:perf on`
- Inspect summary: `:perf status`
- Dump top buckets: `:perf dump`
- Disable: `:perf off`

Table scenario conventions live in `roadmap/perf-multirow-table.md`.
