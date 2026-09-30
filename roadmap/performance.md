# Performance Baselines

## Scope

This file is an operational/measurement reference.
Execution backlog ownership for large-note performance lives in `roadmap/plan.md` (Performance Backlog).


This repository stores simple startup performance baselines in:

- `perf/baselines/startup.json`

## Record a new baseline

```sh
cargo run --release -p slate --bin perf-check -- --record-startup [--runs N]
```

This runs the native probe binary (`note-startup tui`) and writes median timings for each startup mark.

## Check for regressions

```sh
cargo run --release -p slate --bin perf-check
```

Runs every check enabled in `perf/config.json` (startup, table, large note). The startup check fails when a measured mark regresses more than `threshold_pct` (default 15%).

## Large-note latency

```sh
cargo test --release -p slate --lib large_note_perf -- --ignored --nocapture
```

Generates a note (prose, images, tables with formulas, variables with duplicate assignments and chapter-long running totals, wiki links and cross-note references to two linked notes), then times navigation, typing, structural edits, undo/redo, search, open and idle ticks. Each sample is one action's key handling plus the repaint after it. The test fails when a p95 exceeds its limit in `perf/baselines/large_note.json`; `perf-check` runs it in CI.

CI checks 30k and 100k lines. `SLATE_LARGE_NOTE_SIZES=200000,400000` measures other line counts; sizes without limits are reported only. `open` is the first paint; `open_calc` (reported only) is when calc values appear, since notes above 20k lines prepare calc off the input thread. `search` is one sample for the whole `/query` + Enter key sequence, not per key.

Measured 2026-09-29 (p50 ms, 30k / 100k lines): scroll one line 1.9 / 5.3, jump 1.7 / 5.1, type 0.4 / 0.4, delete line 6.2 / 17, paste line 6.3 / 15, undo 5.4 / 13, open 8.6 / 29 (calc values at 28 / 99), idle tick p95 18 / 21. Autosave and the first dependency index build run off the input thread; the per-edit dependency index sync (~20 ms at 100k) runs on an idle tick.

## Runtime Tracing

Use the in-editor profiler for interaction latency and payload metrics.

- Enable with command: `:perf on`
- Inspect summary: `:perf status`
- Dump top buckets: `:perf dump`
- Disable: `:perf off`

Table scenario conventions live in `roadmap/perf-multirow-table.md`.
