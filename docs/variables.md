# Variables Specification (v1)

This document defines the shared variable behavior used by the Tauri UI and Rust core.

## Syntax

- Assignment form: `name := expression`
- `:=` may be written with or without spaces (`name:=expression` also works)
- Table formula form: cell-local `:=expression` (the `:=` must be the first non-space token in the cell)
- Table cell reference form inside table formulas: `(row,col)` (1-based)
- Variable names:
  - case-insensitive
  - may contain ASCII letters, digits, underscore, and spaces
  - internal spaces are normalized (multiple spaces collapse to one)

Examples:

```text
subtotal := 100
tax rate := 0.2
total := subtotal + subtotal * tax rate
```

## Scope and Precedence

- Scope is note-local (single note/document).
- Definitions are global inside the note (not limited to paragraph/list/table block).
- If a variable is assigned multiple times, the last assignment in the note wins.
- Runtime enablement is per-note. Calc behavior is gated by note modules:
  - `modules.math` is the master switch for calc evaluation.
  - `modules.variables` controls variable definition/reference resolution and autocomplete.
  - `modules.table` controls table formula parsing/evaluation.
- `[editor.modules]` config values are defaults for newly created notes.

Module behavior matrix:

- `math=off` (any `variables/table` values): no calc evaluation.
- `math=on`, `variables=off`, `table=off`: plain/list arithmetic only.
- `math=on`, `variables=on`, `table=off`: plain/list arithmetic + variable assignments/references; table formulas ignored.
- `math=on`, `variables=off`, `table=on`: table formulas and builtin table functions work; variable refs stay unresolved.
- `math=on`, `variables=on`, `table=on`: full calc behavior.

## Evaluation

- Variable assignment lines do not show calc ghost output.
- Non-assignment lines are evaluated in line context:
  - plain line: whole line
  - list/checklist line: list body expression
  - table line: every `:=...` formula cell in the row (left-to-right)
- Variables are resolved before expression evaluation.
- Table formula cells are never treated as variable definitions (`:=expr` in a cell is always a formula, never `name := expr`).
- Table cell references are table-local and use data rows only (exclude header + delimiter rows).
- Table-reference errors are surfaced inline as:
  - `!ERROR#out_of_bounds`
  - `!ERROR#non_numeric`
  - `!ERROR#self_reference`
  - `!ERROR#cycle`
- Unit/currency conversion assignments are normalized to numeric-only values when stored.

Example:

```text
len := 3 m to km
len + 2
```

`len` is stored as a number (`0.003`), then reused numerically.

## Error Handling

- Unresolved variable references produce no ghost output.
- Cyclic dependencies produce no ghost output.
- Diagnostics are available from Rust command output for tests/debugging but are not shown in the editor UI.
- Table reference failures also emit structured diagnostics with `kind=table-ref-*` alongside inline `!ERROR#...` cell values.

## Autocomplete

- Source: variable index from backend evaluation response.
- Trigger: prefix match on normalized variable name.
- Default threshold: 3 typed characters (configurable).
- TUI popup interactions:
  - `Up/Down` moves the popup selection
  - `Tab` or `Enter` accepts the selected variable
  - `Esc` closes the popup
- `Tab` fallback when popup is closed:
  - variable autocomplete accept path (if available)
  - otherwise calc ghost apply path
