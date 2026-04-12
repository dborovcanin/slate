import test from "node:test";
import assert from "node:assert/strict";
import { formatTableLines } from "./markdown-editing.ts";

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

test("formatTableLines returns empty input unchanged", () => {
  assert.deepEqual(formatTableLines([]), []);
});
