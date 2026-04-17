import test, { before } from "node:test";
import assert from "node:assert/strict";
import { EditorState } from "@codemirror/state";
import {
  __tableCursorInternals,
  formatTableLines,
  rewriteLineWithChecklistToggleSuffix,
} from "./markdown-editing.ts";
import { ensureWasmReady, runDocChangeRules, runTableBoundaryEditRules } from "./wasm.ts";

before(async () => {
  await ensureWasmReady();
});

test("formatTableLines normalizes cell padding and preserves delimiter alignment markers", () => {
  const input = ["| col | value |", "| :--- | ---: |", "| x | 10 |"];
  const out = formatTableLines(input);

  assert.deepEqual(out, ["| col  | value |", "| :--- | ----: |", "| x    | 10    |"]);
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
