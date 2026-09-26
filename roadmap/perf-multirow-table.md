# Multirow Table Performance Playbook

## Scope

This file is an operational/measurement reference.
Execution backlog ownership for large-note performance lives in `roadmap/plan.md` (Performance Backlog).


This document defines reusable profiling scenarios and metric names for large-table and multirow-cell work.

## Goals

- Keep cursoring and editing responsive on very large notes/tables.
- Avoid full-table/full-document work on localized edits where possible.
- Track regressions with stable bucket names.

## Tracing Buckets

Use the existing perf tracing commands and keep these bucket names stable:

- `tui.doc_change_rules`
- `tui.enter_rules`
- `tui.tab_rules`
- `tui.calc.recompute`
- `tui.calc.viewport_eval`

Prefer stable `reason` labels:

- `applied`, `noop`
- `incremental`, `stale_full`

## Required Scenarios

Run all scenarios when changing table logic:

1. `100k` note, table block of `200` rows, no continuation rows
2. `100k` note, table block of `200` rows with `30%` continuation rows
3. `400k` note, table block of `500` rows with mixed formula + `!ERROR#...` values
4. `400k` note, cursor movement only (left/right/up/down, tab/shift-tab)
5. `400k` note, localized edits in one table cell (typing/backspace/delete)

## Commands

1. `:perf on`
2. Execute scenario interactions for ~20-30s
3. `:perf dump`
4. `:perf off`

Core microbench:

```sh
cargo run -p slate --bin table-perf --release
```

This runs the Rust microbench binary (`table-perf`) and prints p50/p95/p99/max for:

- `format_table_lines_with_cache`
- `table_cell_cursor_info_in_document_cached`
- `run_doc_change_rules_with_table_cache`

## Reporting

Record for each bucket:

- `count`
- `avg`
- `p50`
- `p95`
- `p99`
- `max`

At minimum, compare current run vs last known-good run for:

- `tui.doc_change_rules` (`applied`)
- `tui.tab_rules` (`applied`)

## Regression Gates (recommended)

- No `>15%` p95 increase for `tui.doc_change_rules/applied` in scenarios 1-3.
- No `>10%` p95 increase for cursor-only scenarios.
- No new long-tail spikes (`p99` and `max`) without root-cause notes.
