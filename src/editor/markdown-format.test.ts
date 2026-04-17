import test, { before } from "node:test";
import assert from "node:assert/strict";
import { formatMarkdownText } from "./markdown-format.ts";
import { ensureWasmReady } from "./wasm.ts";

before(async () => {
  await ensureWasmReady();
});

test("formatMarkdownText normalizes markdown basics", () => {
  const input = "##Title  \n* item\n1.   task\n";
  const out = formatMarkdownText(input);
  assert.equal(out, "## Title\n- item\n1. task\n");
});

test("formatMarkdownText normalizes markdown table padding", () => {
  const input = "| a | b |\n| --- | --- |\n| 1 | 2 |\n";
  const out = formatMarkdownText(input);
  assert.equal(out, "| a   | b   |\n| --- | --- |\n| 1   | 2   |\n");
});

test("formatMarkdownText inserts missing markdown table delimiter row", () => {
  const input = "| test | count |\n| bro | 5 |\n";
  const out = formatMarkdownText(input);
  assert.equal(out, "| test | count |\n| ---- | ----- |\n| bro  | 5     |\n");
});
