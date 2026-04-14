import test from "node:test";
import assert from "node:assert/strict";
import { EditorState } from "@codemirror/state";
import {
  findCalcSegment,
  findListCalcSegment,
  findSingleCalcTableCell,
  lineForCalcEvaluation,
} from "./calc-line-utils.ts";
import {
  builtinFormulaExplanation,
  containsBuiltinFormula,
  computeCalcRefresh,
  containsVariableAssignment,
  formatFormulaDisplayValue,
  lineUsesAssignmentGhostPrefix,
  mergePartialCalcResults,
  remapCalcResultsForDocChange,
  remapVariableIndexForDocChange,
  type CommitMarkerLoc,
} from "./calc-decoration.ts";

function buildLineStarts(lines: readonly string[]): number[] {
  const starts: number[] = [];
  let offset = 0;
  for (const line of lines) {
    starts.push(offset);
    offset += line.length + 1;
  }
  return starts;
}

function markerFor(
  lines: readonly string[],
  lineIdx: number,
  lastLiteral: string,
): CommitMarkerLoc {
  const starts = buildLineStarts(lines);
  const text = lines[lineIdx];
  const eqIdx = text.lastIndexOf(" = ");
  if (eqIdx < 0) throw new Error("marker helper requires an ` = ` trailer");
  return {
    docPos: starts[lineIdx] + eqIdx,
    lineIdx,
    offsetInLine: eqIdx,
    lastLiteral,
  };
}

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

test("findSingleCalcTableCell extracts builtin formula cells", () => {
  const line = "| name | =avg_col() | 1.91 |";
  const cell = findSingleCalcTableCell(line);
  assert.ok(cell);
  assert.equal(line.slice(cell!.fromCol, cell!.toCol), "=avg_col()");
  assert.equal(cell!.expr, "=avg_col()");
});

test("findSingleCalcTableCell prioritizes a single builtin formula cell", () => {
  const line = "| =avg_col() | 2+2 |";
  const cell = findSingleCalcTableCell(line);
  assert.ok(cell);
  assert.equal(line.slice(cell!.fromCol, cell!.toCol), "=avg_col()");
  assert.equal(cell!.expr, "=avg_col()");
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

test("containsBuiltinFormula detects builtin formulas from calc segments", () => {
  assert.equal(
    containsBuiltinFormula([
      "| first | second |",
      "| --- | --- |",
      "| name | =avg_col() |",
    ]),
    true,
  );
  assert.equal(containsBuiltinFormula(["- [ ] =sum_row()"]), true);
});

test("containsBuiltinFormula ignores non-formula lines", () => {
  assert.equal(containsBuiltinFormula(["| name | 2+2 |"]), false);
  assert.equal(containsBuiltinFormula(["- [ ] buy milk"]), false);
  assert.equal(containsBuiltinFormula(["plain text"]), false);
});

test("builtinFormulaExplanation normalizes formula labels", () => {
  assert.equal(builtinFormulaExplanation("=avg_col()"), "avg_col()");
  assert.equal(builtinFormulaExplanation(" sum_column ( ) "), "sum_col()");
  assert.equal(builtinFormulaExplanation("2+2"), null);
});

test("formatFormulaDisplayValue rounds numeric results and strips approximation prefixes", () => {
  assert.equal(formatFormulaDisplayValue("6.666666"), "6.67");
  assert.equal(formatFormulaDisplayValue("≈ 6.666666"), "6.67");
  assert.equal(formatFormulaDisplayValue("approximately 12.000"), "12");
  assert.equal(formatFormulaDisplayValue("5.555 m"), "5.56 m");
});

test("lineUsesAssignmentGhostPrefix detects assignment across plain/list/table lines", () => {
  assert.equal(lineUsesAssignmentGhostPrefix("total := 12"), true);
  assert.equal(lineUsesAssignmentGhostPrefix("- value := total + 12"), true);
  assert.equal(lineUsesAssignmentGhostPrefix("| value := total + 12 |"), true);
  assert.equal(lineUsesAssignmentGhostPrefix("total + 12"), false);
  assert.equal(lineUsesAssignmentGhostPrefix("| label | total + 12 |"), false);
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

test("computeCalcRefresh returns empty plan when there are no markers", () => {
  const lines = ["1 + 1 = 2"];
  const plan = computeCalcRefresh(
    [],
    lines,
    buildLineStarts(lines),
    new Map([[0, "2"]]),
    { from: 0, to: 0 },
  );
  assert.deepEqual(plan.changes, []);
  assert.deepEqual(plan.prune, []);
  assert.deepEqual(plan.syncedLines, []);
});

test("computeCalcRefresh rewrites a stale trailer when backend result changed", () => {
  const lines = ["1 + 1 = 2"];
  const marker = markerFor(lines, 0, "2");
  const plan = computeCalcRefresh(
    [marker],
    lines,
    buildLineStarts(lines),
    new Map([[0, "3"]]),
    { from: 0, to: 0 },
  );
  assert.equal(plan.changes.length, 1);
  const change = plan.changes[0];
  assert.equal(change.lineIdx, 0);
  assert.equal(change.insert, " = 3");
  assert.equal(change.newLiteral, "3");
  assert.equal(change.from, marker.docPos);
  assert.equal(change.to, lines[0].length);
  assert.deepEqual(plan.prune, []);
  assert.deepEqual(plan.syncedLines, [0]);
});

test("computeCalcRefresh is a no-op when current literal already matches the new result", () => {
  const lines = ["1 + 1 = 2"];
  const plan = computeCalcRefresh(
    [markerFor(lines, 0, "2")],
    lines,
    buildLineStarts(lines),
    new Map([[0, "2"]]),
    { from: 0, to: 0 },
  );
  assert.deepEqual(plan.changes, []);
  assert.deepEqual(plan.prune, []);
  assert.deepEqual(plan.syncedLines, [0]);
});

test("computeCalcRefresh prunes markers when the trailer prefix is gone", () => {
  // User deleted the trailer entirely; the marker's offset no longer has ` = `.
  const lines = ["1 + 1"];
  const marker: CommitMarkerLoc = {
    docPos: 5,
    lineIdx: 0,
    offsetInLine: 5,
    lastLiteral: "2",
  };
  const plan = computeCalcRefresh(
    [marker],
    lines,
    buildLineStarts(lines),
    new Map([[0, "2"]]),
    { from: 0, to: 0 },
  );
  assert.deepEqual(plan.changes, []);
  assert.deepEqual(plan.prune, [marker.docPos]);
  assert.deepEqual(plan.syncedLines, []);
});

test("computeCalcRefresh prunes markers when the user hand-edited the literal", () => {
  const lines = ["1 + 1 = FOO"];
  const marker = markerFor(lines, 0, "2");
  const plan = computeCalcRefresh(
    [marker],
    lines,
    buildLineStarts(lines),
    new Map([[0, "2"]]),
    { from: 0, to: 0 },
  );
  assert.deepEqual(plan.changes, []);
  assert.deepEqual(plan.prune, [marker.docPos]);
  assert.deepEqual(plan.syncedLines, []);
});

test("computeCalcRefresh skips refresh when the selection overlaps the trailer", () => {
  const lines = ["1 + 1 = 2"];
  const marker = markerFor(lines, 0, "2");
  const cursor = marker.docPos + 4; // inside the literal
  const plan = computeCalcRefresh(
    [marker],
    lines,
    buildLineStarts(lines),
    new Map([[0, "3"]]),
    { from: cursor, to: cursor },
  );
  assert.deepEqual(plan.changes, []);
  assert.deepEqual(plan.prune, []);
  assert.deepEqual(plan.syncedLines, []);
});

test("computeCalcRefresh keeps the marker when the backend has no result for the line", () => {
  const lines = ["1 + 1 = 2"];
  const marker = markerFor(lines, 0, "2");
  const plan = computeCalcRefresh(
    [marker],
    lines,
    buildLineStarts(lines),
    new Map(),
    { from: 0, to: 0 },
  );
  assert.deepEqual(plan.changes, []);
  assert.deepEqual(plan.prune, []);
  assert.deepEqual(plan.syncedLines, []);
});

test("computeCalcRefresh emits ordered changes across multiple lines", () => {
  const lines = ["1 + 1 = 2", "- buy milk", "2 + 2 = 4"];
  const starts = buildLineStarts(lines);
  const markers = [markerFor(lines, 0, "2"), markerFor(lines, 2, "4")];
  const plan = computeCalcRefresh(
    markers,
    lines,
    starts,
    new Map([
      [0, "5"],
      [2, "10"],
    ]),
    { from: 0, to: 0 },
  );
  assert.equal(plan.changes.length, 2);
  assert.equal(plan.changes[0].lineIdx, 0);
  assert.equal(plan.changes[0].newLiteral, "5");
  assert.equal(plan.changes[1].lineIdx, 2);
  assert.equal(plan.changes[1].newLiteral, "10");
  // Must be sorted ascending by `from` so CodeMirror accepts the batch.
  assert.ok(plan.changes[0].from < plan.changes[1].from);
  assert.deepEqual(plan.syncedLines, [0, 2]);
});

test("computeCalcRefresh mixes refresh and prune outcomes independently", () => {
  const lines = ["1 + 1 = 2", "2 + 2 = WAT"];
  const markers = [markerFor(lines, 0, "2"), markerFor(lines, 1, "4")];
  const plan = computeCalcRefresh(
    markers,
    lines,
    buildLineStarts(lines),
    new Map([
      [0, "3"],
      [1, "4"],
    ]),
    { from: 0, to: 0 },
  );
  assert.equal(plan.changes.length, 1);
  assert.equal(plan.changes[0].lineIdx, 0);
  assert.equal(plan.changes[0].newLiteral, "3");
  assert.deepEqual(plan.prune, [markers[1].docPos]);
  assert.deepEqual(plan.syncedLines, [0]);
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

test("remapCalcResultsForDocChange shifts ghosts down when inserting lines above", () => {
  const start = EditorState.create({ doc: "top\nexpr\ntail" });
  const tr = start.update({
    changes: { from: 0, to: 0, insert: "new line\n" },
  });
  const remapped = remapCalcResultsForDocChange(
    new Map([[1, "42"]]),
    tr.startState.doc,
    tr.changes,
    tr.newDoc,
  );
  assert.equal(remapped.get(2), "42");
  assert.equal(remapped.has(1), false);
});

test("remapCalcResultsForDocChange shifts ghosts up when deleting lines above", () => {
  const start = EditorState.create({ doc: "drop\nexpr\ntail" });
  const dropLine = start.doc.line(1);
  const tr = start.update({
    changes: { from: dropLine.from, to: dropLine.to + 1, insert: "" },
  });
  const remapped = remapCalcResultsForDocChange(
    new Map([[1, "42"]]),
    tr.startState.doc,
    tr.changes,
    tr.newDoc,
  );
  assert.equal(remapped.get(0), "42");
  assert.equal(remapped.has(1), false);
});

test("remapCalcResultsForDocChange keeps ghost when only list marker prefix changes", () => {
  const start = EditorState.create({ doc: "3.1 2 + val1\nnext" });
  const tr = start.update({
    changes: { from: 0, to: 4, insert: "7.9 " },
  });
  const remapped = remapCalcResultsForDocChange(
    new Map([[0, "200"]]),
    tr.startState.doc,
    tr.changes,
    tr.newDoc,
  );
  assert.equal(remapped.get(0), "200");
});

test("remapCalcResultsForDocChange drops ghost when expression text changes", () => {
  const start = EditorState.create({ doc: "3.1 2 + val1\nnext" });
  const exprPos = start.doc.toString().indexOf("2 + val1");
  const tr = start.update({
    changes: { from: exprPos, to: exprPos + 1, insert: "9" },
  });
  const remapped = remapCalcResultsForDocChange(
    new Map([[0, "200"]]),
    tr.startState.doc,
    tr.changes,
    tr.newDoc,
  );
  assert.equal(remapped.has(0), false);
});

test("remapCalcResultsForDocChange keeps ghost through full list rewrite when item is added above", () => {
  const start = EditorState.create({
    doc: "1. a\n2. b\n3.1 2 + val1\n3.2 tail",
  });
  const tr = start.update({
    changes: {
      from: 0,
      to: start.doc.length,
      insert: "1. a\n2. new\n3. b\n4.1 2 + val1\n4.2 tail",
    },
  });
  const remapped = remapCalcResultsForDocChange(
    new Map([[2, "200"]]),
    tr.startState.doc,
    tr.changes,
    tr.newDoc,
  );
  assert.equal(remapped.get(3), "200");
});

test("remapCalcResultsForDocChange keeps ghost through full list rewrite when item is added below", () => {
  const start = EditorState.create({
    doc: "1. a\n2. b\n3.1 2 + val1\n3.2 tail",
  });
  const tr = start.update({
    changes: {
      from: 0,
      to: start.doc.length,
      insert: "1. a\n2. b\n3.1 2 + val1\n3.2 new\n3.3 tail",
    },
  });
  const remapped = remapCalcResultsForDocChange(
    new Map([[2, "200"]]),
    tr.startState.doc,
    tr.changes,
    tr.newDoc,
  );
  assert.equal(remapped.get(2), "200");
});

test("remapVariableIndexForDocChange shifts variable line numbers after deleting unrelated line above", () => {
  const start = EditorState.create({
    doc: "remove me\nvar1 := 10\n2 + var1",
  });
  const firstLine = start.doc.line(1);
  const tr = start.update({
    changes: { from: firstLine.from, to: firstLine.to + 1, insert: "" },
  });
  const remapped = remapVariableIndexForDocChange(
    [{ name: "var1", normalized: "var1", line: 2 }],
    tr.startState.doc,
    tr.changes,
    tr.newDoc,
  );
  assert.deepEqual(remapped, [{ name: "var1", normalized: "var1", line: 1 }]);
});

test("remapCalcResultsForDocChange keeps duplicate-expression ghosts on distinct lines after list rewrite", () => {
  const start = EditorState.create({
    doc: "1.1 2 + val1\n1.2 2 + val1\nend",
  });
  const tr = start.update({
    changes: {
      from: 0,
      to: start.doc.length,
      insert: "1. head\n2.1 2 + val1\n2.2 2 + val1\nend",
    },
  });
  const remapped = remapCalcResultsForDocChange(
    new Map([
      [0, "200"],
      [1, "200"],
    ]),
    tr.startState.doc,
    tr.changes,
    tr.newDoc,
  );
  assert.equal(remapped.get(1), "200");
  assert.equal(remapped.get(2), "200");
  assert.equal(remapped.size, 2);
});
