import test from "node:test";
import assert from "node:assert/strict";
import { formatMarkdownText } from "./markdown-format.ts";

test("formatMarkdownText normalizes markdown basics", () => {
  const input = "##Title  \n* item\n1.   task\n";
  const out = formatMarkdownText(input);
  assert.equal(out, "## Title\n- item\n1. task\n");
});

test("formatMarkdownText aligns markdown tables", () => {
  const input = "| a | b |\n| --- | --- |\n| 1 | 2 |\n";
  const out = formatMarkdownText(input);
  assert.equal(out, "| a   | b   |\n| --- | --- |\n| 1   | 2   |\n");
});
