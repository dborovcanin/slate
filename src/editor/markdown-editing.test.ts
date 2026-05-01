import test, { before } from "node:test";
import assert from "node:assert/strict";
import { EditorState } from "@codemirror/state";
import {
  __tableCursorInternals,
  formatTableLines,
  runTableCellNavigationCommand,
  runTableHeaderDeleteColumnCommand,
  rewriteLineWithChecklistToggleSuffix,
} from "./markdown-editing.ts";
import {
  ensureWasmReady,
  getTableCursorCellInfo,
  runDocChangeRules,
  runTableBoundaryEditRules,
  runTableMultilineBreakRule,
} from "./wasm.ts";
import type { EditorView } from "@codemirror/view";

before(async () => {
  await ensureWasmReady();
});

test("formatTableLines normalizes cell padding and preserves delimiter alignment markers", () => {
  const input = ["| col | value |", "| :--- | ---: |", "| x | 10 |"];
  const out = formatTableLines(input);

  assert.deepEqual(out, ["| col | value |", "| :--- | ----: |", "| x   | 10    |"]);
});

test("formatTableLines fills missing cells in shorter rows", () => {
  const input = ["| a | b | c |", "| --- | --- | --- |", "| 1 | 2 |"];
  const out = formatTableLines(input);

  assert.deepEqual(out, ["| a   | b   | c   |", "| --- | --- | --- |", "| 1   | 2   |     |"]);
});

test("formatTableLines inserts a delimiter row when missing", () => {
  const input = ["| test | count |", "| bro | 5 |"];
  const out = formatTableLines(input);

  assert.deepEqual(out, ["| test | count |", "| ---- | ----- |", "| bro  | 5     |"]);
});

test("formatTableLines returns empty input unchanged", () => {
  assert.deepEqual(formatTableLines([]), []);
});

test("formatTableLines mirrors rust formatter when delimiter was previously widest", () => {
  const input = ["| a | hi         |", "| --- | ---------- |", "| 1 | 22         |"];
  const out = formatTableLines(input);
  assert.deepEqual(out, ["| a   | hi  |", "| --- | --- |", "| 1   | 22  |"]);
});

test("formatTableLines keeps escaped pipes inside cells", () => {
  const input = ["| left \\| right | v |", "| --- | --- |", "| short | 1 |"];
  const out = formatTableLines(input);
  assert.deepEqual(out, ["| left \\| right | v   |", "| ------------- | --- |", "| short         | 1   |"]);
});

test("rewriteLineWithChecklistToggleSuffix toggles checklist checked state", () => {
  assert.equal(
    rewriteLineWithChecklistToggleSuffix("- [ ] ship docs /x"),
    "- [x] ship docs",
  );
  assert.equal(
    rewriteLineWithChecklistToggleSuffix("- [x] ship docs /x"),
    "- [ ] ship docs",
  );
});

test("rewriteLineWithChecklistToggleSuffix converts list items to checked checklist", () => {
  assert.equal(rewriteLineWithChecklistToggleSuffix("- ship docs /x"), "- [x] ship docs");
  assert.equal(rewriteLineWithChecklistToggleSuffix("1. ship docs /x"), "1. [x] ship docs");
});

test("rewriteLineWithChecklistToggleSuffix ignores non-suffix /x usage", () => {
  assert.equal(rewriteLineWithChecklistToggleSuffix("- path/x"), null);
  assert.equal(rewriteLineWithChecklistToggleSuffix("- item /x now"), null);
});

test("runDocChangeRules keeps table autoformat selection collapsed (no backward range)", () => {
  const text = "| header | number |\n| val | 3388.89 |";
  const head = text.indexOf("3388.89") + "3388.89".length;
  const operation = runDocChangeRules(
    {
      text,
      selection: { anchor: head, head },
    },
    { markdownAutoformat: true },
  );
  assert.ok(operation);
  assert.equal(operation?.selection?.head, undefined);
});

test("table cursor internals compute empty-cell anchor after left padding", () => {
  const line = "| aaa |     | bb  |";
  const cell = __tableCursorInternals.tableCellAtColumn(line, 8);
  assert.ok(cell);
  assert.equal(__tableCursorInternals.tableCellNavigationAnchorInLine(cell), 8);
});

test("table cursor internals clamp right padding to content anchor", () => {
  const doc = "| aaa | bb  |";
  const state = EditorState.create({ doc });
  const rightPaddingPos = 11;
  assert.equal(__tableCursorInternals.clampTableCursorToContent(state, rightPaddingPos), 10);
  assert.equal(__tableCursorInternals.clampTableCursorToContent(state, 9), null);
});

test("table cursor internals clamp left padding to mandatory one-space anchor", () => {
  const doc = "| aaa |     |";
  const state = EditorState.create({ doc });
  assert.equal(__tableCursorInternals.clampTableCursorToContent(state, 7), 8);
});

test("runTableBoundaryEditRules keeps backspace/delete inside table cell boundaries", () => {
  const text = "| aaa | bb |";
  const keepBackspace = runTableBoundaryEditRules(
    {
      text,
      selection: { anchor: 8, head: 8 },
    },
    { backward: true },
  );
  assert.ok(keepBackspace);
  assert.deepEqual(keepBackspace?.changes ?? [], []);

  const keepDelete = runTableBoundaryEditRules(
    {
      text,
      selection: { anchor: 10, head: 10 },
    },
    { backward: false },
  );
  assert.ok(keepDelete);
  assert.deepEqual(keepDelete?.changes ?? [], []);
});

test("runTableBoundaryEditRules supports explicit structural merges", () => {
  const text = "| aaa | bb |";
  const mergePrev = runTableBoundaryEditRules(
    {
      text,
      selection: { anchor: 8, head: 8 },
    },
    { backward: true, structuralMerge: true },
  );
  assert.ok(mergePrev);
  assert.deepEqual(mergePrev?.changes[0], {
    from: 0,
    to: text.length,
    insert: "| aaa bb |",
  });
});

test("runTableMultilineBreakRule splits table cell into next row", () => {
  const text = "| left | value |";
  const head = text.indexOf("value") + 2;
  const op = runTableMultilineBreakRule({
    text,
    selection: { anchor: head, head },
  });
  assert.ok(op);
  assert.deepEqual(op?.changes, [
    {
      from: 0,
      to: text.length,
      insert: "| left | va  |\n| ---- | --- |\n|      | lue |",
    },
  ]);
  const result = op?.changes?.[0]?.insert ?? "";
  const lines = result.split("\n");
  const thirdLineStart = lines[0].length + 1 + lines[1].length + 1;
  const expectedAnchor = thirdLineStart + lines[2].indexOf("| lue") + 2;
  assert.equal(op?.selection?.anchor, expectedAnchor);
});

test("getTableCursorCellInfo maps continuation line to previous logical row", () => {
  const info = getTableCursorCellInfo(
    [
      "| name | value |",
      "| --- | --- |",
      "| alpha | one |",
      "|> beta | two |",
      "| gamma | three |",
    ],
    3,
    4,
  );
  assert.ok(info);
  assert.equal(info?.logicalRowIndex, 0);
  assert.equal(info?.logicalRowCount, 2);
  assert.equal(info?.isContinuationRow, true);
});

function createTestEditorView(doc: string, cursor: number): {
  view: EditorView;
  getState: () => EditorState;
} {
  let state = EditorState.create({
    doc,
    selection: { anchor: cursor },
  });
  const view = {
    get state() {
      return state;
    },
    dispatch(spec: unknown) {
      state = state.update(spec as never).state;
    },
  } as unknown as EditorView;
  return { view, getState: () => state };
}

test("table scoped snapshot remaps edit operation back to document offsets", () => {
  const doc = [
    "alpha",
    "beta",
    "| a |   | c |",
    "| --- | --- | --- |",
    "| 1 | 2 | 3 |",
    "omega",
  ].join("\n");
  const cursor = doc.indexOf("|   |") + 2;
  const { view, getState } = createTestEditorView(doc, cursor);

  assert.equal(runTableHeaderDeleteColumnCommand(view), true);
  const result = getState().doc.toString();
  assert.equal(
    result,
    ["alpha", "beta", "| a   | c   |", "| --- | --- |", "| 1   | 3   |", "omega"].join("\n"),
  );
  const headerStart = result.indexOf("| a   | c   |");
  assert.notEqual(headerStart, -1);
  assert.equal(getState().selection.main.head, headerStart + 3);
});

test("table column delete lands on next column when removing first column", () => {
  const doc = [
    "alpha",
    "|   | b | c |",
    "| --- | --- | --- |",
    "| 1 | 2 | 3 |",
    "omega",
  ].join("\n");
  const cursor = doc.indexOf("|   |") + 2;
  const { view, getState } = createTestEditorView(doc, cursor);

  assert.equal(runTableHeaderDeleteColumnCommand(view), true);
  const result = getState().doc.toString();
  assert.equal(
    result,
    ["alpha", "| b   | c   |", "| --- | --- |", "| 2   | 3   |", "omega"].join("\n"),
  );
  const headerStart = result.indexOf("| b   | c   |");
  assert.notEqual(headerStart, -1);
  assert.equal(getState().selection.main.head, headerStart + 3);
});

test("table scoped snapshot remaps navigation selection back to document offsets", () => {
  const doc = ["alpha", "beta", "| a | b | c |", "| --- | --- | --- |", "| 1 | 2 | 3 |"].join("\n");
  const tableStart = doc.indexOf("| a | b | c |");
  const cursor = tableStart + 3;
  const { view, getState } = createTestEditorView(doc, cursor);

  assert.equal(
    runTableCellNavigationCommand(view, {
      markdownAutoformat: true,
      outdent: false,
    }),
    true,
  );
  assert.equal(getState().selection.main.head, tableStart + 7);
});
