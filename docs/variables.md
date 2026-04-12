# Variables Specification (v1)

This document defines the shared variable behavior used by the Tauri UI and Rust core.

## Syntax

- Assignment form: `name := expression`
- `:=` may be written with or without spaces (`name:=expression` also works)
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

## Evaluation

- Variable assignment lines do not show calc ghost output.
- Non-assignment lines are evaluated in line context:
  - plain line: whole line
  - list/checklist line: list body expression
  - table line: a single calc-like cell (if exactly one eligible cell exists)
- Variables are resolved before expression evaluation.
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

## Autocomplete

- Source: variable index from backend evaluation response.
- Trigger: prefix match on normalized variable name.
- Default threshold: 3 typed characters (configurable).
- `Tab` behavior:
  - if variable autocomplete popup is open, `Tab` accepts selected variable
  - otherwise, `Tab` applies calc ghost result
