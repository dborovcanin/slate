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

## Runtime Tracing

Use the in-editor profiler for interaction latency and payload metrics.

- Enable with command: `:perf on`
- Inspect summary: `:perf status`
- Dump top buckets: `:perf dump`
- Disable: `:perf off`

Table scenario conventions live in `roadmap/perf-multirow-table.md`.
