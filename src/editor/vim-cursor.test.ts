import test from "node:test";
import assert from "node:assert/strict";
import { vimAppendInsertPos, vimNormalLineEndPos } from "./vim-utils.ts";

test("vimNormalLineEndPos lands on last character for non-empty line", () => {
  assert.equal(vimNormalLineEndPos(10, 15), 14);
});

test("vimNormalLineEndPos stays at line start for empty line", () => {
  assert.equal(vimNormalLineEndPos(7, 7), 7);
});

test("vimAppendInsertPos advances within the same line", () => {
  assert.equal(vimAppendInsertPos(11, 15), 12);
});

test("vimAppendInsertPos does not cross to next line at line end", () => {
  assert.equal(vimAppendInsertPos(15, 15), 15);
});
