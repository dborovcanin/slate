# Multirow Table Performance Playbook

## Scope

This file is an operational/measurement reference.
Execution backlog ownership for large-note performance lives in `roadmap/plan.md` (Architecture and Performance Review Backlog).


This document defines reusable profiling scenarios and metric names for large-table and multirow-cell work.

## Goals

- Keep cursoring and editing responsive on very large notes/tables.
- Avoid full-table/full-document work on localized edits where possible.
- Track regressions with stable bucket names across UI and TUI.

## Tracing Buckets

Use the existing perf tracing commands and keep these bucket names stable:

- `tui.doc_change_rules`
- `tui.enter_rules`
- `tui.tab_rules`
- `tui.calc.recompute`
- `tui.calc.viewport_eval`
- `markdown.decorations.safeBuild`
- `calc.decorations.safeBuild`

Prefer stable `reason` labels:

- `applied`, `noop`
- `incremental`, `stale_full`
- `selectionSet`, `viewportChanged`, `docChanged`

## Required Scenarios

Run all scenarios in both UI and TUI when changing table logic:

1. `100k` note, table block of `200` rows, no continuation rows
2. `100k` note, table block of `200` rows with `30%` continuation rows
3. `400k` note, table block of `500` rows with mixed formula + `!ERROR#...` values
4. `400k` note, cursor movement only (left/right/up/down, tab/shift-tab)
5. `400k` note, localized edits in one table cell (typing/backspace/delete)

## Commands

UI:

1. `:perf on`
2. Execute scenario interactions for ~20-30s
3. `:perf dump`
4. `:perf off`

TUI:

1. `:perf on`
2. Execute scenario interactions for ~20-30s
3. `:perf dump`
4. `:perf off`

Core microbench:

```sh
npm run perf:table
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
- `markdown.decorations.safeBuild` (`viewportChanged`)
- `calc.decorations.safeBuild` (`docChanged`)

## Regression Gates (recommended)

- No `>15%` p95 increase for `tui.doc_change_rules/applied` in scenarios 1-3.
- No `>10%` p95 increase for cursor-only scenarios.
- No new long-tail spikes (`p99` and `max`) without root-cause notes.
