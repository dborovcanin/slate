import test, { before } from "node:test";
import assert from "node:assert/strict";
import { formatTableLines, rewriteLineWithChecklistToggleSuffix } from "./markdown-editing.ts";
import { ensureWasmReady } from "./wasm.ts";

before(async () => {
  await ensureWasmReady();
});

test("formatTableLines aligns columns and preserves delimiter alignment markers", () => {
  const input = ["| col | value |", "| :--- | ---: |", "| x | 10 |"];
  const out = formatTableLines(input);

  assert.deepEqual(out, ["| col | value |", "| :--- | ----: |", "| x   | 10    |"]);
});

test("formatTableLines fills missing cells in shorter rows", () => {
  const input = ["| a | b | c |", "| --- | --- | --- |", "| 1 | 2 |"];
  const out = formatTableLines(input);

  assert.deepEqual(out, [
    "| a   | b   | c   |",
    "| --- | --- | --- |",
    "| 1   | 2   |     |",
  ]);
});

test("formatTableLines inserts a delimiter row when missing", () => {
  const input = ["| test | count |", "| bro | 5 |"];
  const out = formatTableLines(input);

  assert.deepEqual(out, [
    "| test | count |",
    "| ---- | ----- |",
    "| bro  | 5     |",
  ]);
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
