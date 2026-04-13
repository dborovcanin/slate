import test from "node:test";
import assert from "node:assert/strict";
import {
  findCalcSegment,
  findListCalcSegment,
  findSingleCalcTableCell,
  lineForCalcEvaluation,
} from "./calc-line-utils.ts";
import {
  containsVariableAssignment,
  mergePartialCalcResults,
} from "./calc-decoration.ts";

test("findSingleCalcTableCell extracts a single expression cell", () => {
  const line = "| item | 4+2 |";
  const cell = findSingleCalcTableCell(line);
  assert.ok(cell);
  assert.equal(line.slice(cell!.fromCol, cell!.toCol), "4+2");
  assert.equal(cell!.expr, "4+2");
});

test("findSingleCalcTableCell ignores ambiguous rows with multiple expressions", () => {
  const line = "| 1+2 | 3+4 |";
  assert.equal(findSingleCalcTableCell(line), null);
});

test("findListCalcSegment extracts calc body for unordered and ordered items", () => {
  const unordered = findListCalcSegment("- 4+2");
  assert.ok(unordered);
  assert.equal(unordered!.expr, "4+2");

  const ordered = findListCalcSegment("1. 10/2");
  assert.ok(ordered);
  assert.equal(ordered!.expr, "10/2");

  const orderedNested = findListCalcSegment("1.1 10/2");
  assert.ok(orderedNested);
  assert.equal(orderedNested!.expr, "10/2");

  const arrow = findListCalcSegment("  -> 9-3");
  assert.ok(arrow);
  assert.equal(arrow!.expr, "9-3");
});

test("findListCalcSegment extracts calc body for checklists", () => {
  const unchecked = findListCalcSegment("- [ ] 5*5");
  assert.ok(unchecked);
  assert.equal(unchecked!.expr, "5*5");

  const checked = findListCalcSegment("- [x] 7+1");
  assert.ok(checked);
  assert.equal(checked!.expr, "7+1");

  const nested = findListCalcSegment("  - [ ] 4+2");
  assert.ok(nested);
  assert.equal(nested!.expr, "4+2");
});

test("lineForCalcEvaluation uses cell expression for table rows", () => {
  assert.equal(lineForCalcEvaluation("| total | 50 kg to lbs |"), "50 kg to lbs");
  assert.equal(lineForCalcEvaluation("- [ ] 4+2"), "4+2");
  assert.equal(lineForCalcEvaluation("  - [ ] 4+2"), "4+2");
  assert.equal(lineForCalcEvaluation("  -> 4+2"), "4+2");
  assert.equal(lineForCalcEvaluation("1. 10/2"), "10/2");
  assert.equal(lineForCalcEvaluation("1.1 10/2"), "10/2");
  assert.equal(lineForCalcEvaluation("- [x] 6"), "");
  assert.equal(lineForCalcEvaluation("- 6"), "");
  assert.equal(lineForCalcEvaluation("| label | 6 |"), "");
  assert.equal(lineForCalcEvaluation("2+2"), "2+2");
});

test("findCalcSegment returns null when no scoped calc target exists", () => {
  assert.equal(findCalcSegment("- buy milk"), null);
  assert.equal(findCalcSegment("| a | b |"), null);
});

test("containsVariableAssignment detects := while ignoring lookalike operators", () => {
  assert.equal(containsVariableAssignment(["tax := 0.2"]), true);
  assert.equal(containsVariableAssignment(["tax:=0.2"]), true);
  assert.equal(containsVariableAssignment(["- base := 10"]), true);
  assert.equal(containsVariableAssignment(["if a != b"]), false);
  assert.equal(containsVariableAssignment(["x == y"]), false);
  assert.equal(containsVariableAssignment(["2 + 2"]), false);
  assert.equal(containsVariableAssignment(["a >= b", "c <= d"]), false);
});

test("mergePartialCalcResults overlays backend results on top of base", () => {
  const base = new Map<number, string>([
    [0, "4"],
    [1, "stale"],
    [3, "9"],
  ]);
  const lineResults: (string | null)[] = [null, "20", "30", null];
  const merged = mergePartialCalcResults(base, lineResults, 1, 3);
  assert.deepEqual(
    [...merged.entries()].sort((a, b) => a[0] - b[0]),
    [
      [0, "4"],
      [1, "20"],
      [2, "30"],
      [3, "9"],
    ],
  );
});

test("mergePartialCalcResults deletes entries when backend returns null in range", () => {
  const base = new Map<number, string>([
    [0, "4"],
    [1, "4"],
    [2, "9"],
  ]);
  const lineResults: (string | null)[] = [null, null, null];
  const merged = mergePartialCalcResults(base, lineResults, 1, 2);
  assert.equal(merged.has(1), false);
  assert.equal(merged.get(0), "4");
  assert.equal(merged.get(2), "9");
});

test("mergePartialCalcResults leaves indices outside range untouched", () => {
  const base = new Map<number, string>([
    [0, "keep"],
    [5, "keep"],
  ]);
  const lineResults: (string | null)[] = [null, null, "new", null, null, null];
  const merged = mergePartialCalcResults(base, lineResults, 2, 3);
  assert.equal(merged.get(0), "keep");
  assert.equal(merged.get(2), "new");
  assert.equal(merged.get(5), "keep");
});
