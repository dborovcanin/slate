# Variables Specification

This document defines variable and table-formula behavior in Slate.

## Syntax

- assignment form: `name := expression`
- no-space form also works: `name:=expression`
- table formula form: cell-local `:=expression`
- table cell reference in table formulas: `(row,col)` (1-based)

Variable names:

- case-insensitive
- may include ASCII letters, digits, underscore, and spaces
- internal spaces are normalized (multiple spaces collapse)

## Scope and precedence

- scope is note-local
- definitions are global inside the note
- if assigned multiple times, the last assignment wins

## Module gating

Calc runtime is gated by per-note modules:

- `math`: master calc gate
- `variables`: variable definition/reference/autocomplete gate
- `table`: table formula parsing/eval gate

Matrix summary:

- `math=off`: no calc evaluation
- `math=on`, `variables=off`, `table=off`: plain/list arithmetic only
- `math=on`, `variables=on`, `table=off`: variable features enabled, table formulas ignored
- `math=on`, `variables=off`, `table=on`: table formulas work, variable refs remain unresolved
- `math=on`, `variables=on`, `table=on`: full behavior

## Evaluation behavior

- assignment lines do not show calc ghost output
- non-assignment lines are evaluated in context:
  - plain line: full line
  - list/checklist line: list body expression
  - table line: each `:=...` formula cell left to right

Table rule:

- `:=expr` inside a cell is always a formula marker
- it is never treated as a variable definition

## Table references and errors

Table references are table-local and use data rows only.

Inline table errors:

- `!ERROR#out_of_bounds`
- `!ERROR#non_numeric`
- `!ERROR#self_reference`
- `!ERROR#cycle`

## Conversion assignments

Conversion assignments are normalized to numeric values when stored.

Example:

```text
len := 3 m to km
len + 2
```

`len` is stored as numeric `0.003` and reused numerically.

## Error handling

- unresolved variable refs produce no ghost output
- cyclic dependencies produce no ghost output
- diagnostics exist for tests/debugging paths

## Autocomplete

- source: backend/shared evaluation variable index
- default trigger threshold: 3 characters (`variable_autocomplete_min_chars`)

TUI popup interactions:

- `Up/Down`: move selection
- `Tab` or `Enter`: accept selection
- `Esc`: close popup

`Tab` fallback when popup is closed:

- accept active variable completion if available
- otherwise apply calc ghost result
