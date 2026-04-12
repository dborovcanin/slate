import test from "node:test";
import assert from "node:assert/strict";
import {
  findCalcSegment,
  findListCalcSegment,
  findSingleCalcTableCell,
  lineForCalcEvaluation,
} from "./calc-line-utils.ts";

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
