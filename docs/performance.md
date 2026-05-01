# Performance Baselines

This repository stores simple startup performance baselines in:

- `perf/baselines/startup.json`

## Record a new baseline

```sh
npm run perf:startup:record
```

This runs the native probe binary (`note-startup`) for both `gui` and `tui` modes and writes median timings for each startup mark.

## Check for regressions

```sh
npm run perf:startup:check
```

The check fails when a measured startup mark regresses more than the configured threshold (`threshold_pct`, default 15%).

## UI startup marks

GUI WebView startup marks are emitted in browser console when enabled.

Enable with either:

- query param `?startupMetrics=1`
- local storage key `note.startup.metrics=1`

When enabled, the app logs one line starting with:

- `NOTE_UI_STARTUP_METRICS`

## UI Runtime Tracing

Use the in-editor profiler for interaction latency and payload metrics.

- Enable with command: `:perf on`
- Inspect summary: `:perf status`
- Dump top buckets: `:perf dump`
- Disable: `:perf off`

Detailed naming/reason/metric conventions live in:

- `docs/perf-tracing.md`
