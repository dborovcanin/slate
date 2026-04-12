import test from "node:test";
import assert from "node:assert/strict";
import { parseNumbers, parseScope, resolveScopeRange } from "./ex-commands.ts";

test("parseScope defaults to paragraph", () => {
  assert.equal(parseScope(undefined), "paragraph");
  assert.equal(parseScope(""), "paragraph");
  assert.equal(parseScope(" paragraph "), "paragraph");
});

test("parseScope maps aliases", () => {
  assert.equal(parseScope("doc"), "doc");
  assert.equal(parseScope("all"), "doc");
  assert.equal(parseScope("file"), "doc");
  assert.equal(parseScope("list"), "list");
  assert.equal(parseScope("table"), "table");
});

test("parseNumbers extracts integers and decimals", () => {
  const values = parseNumbers("a 10 b -2.5 c 1,200 d +0.75");
  assert.deepEqual(values, [10, -2.5, 1200, 0.75]);
});

test("resolveScopeRange finds paragraph around cursor", () => {
  const lines = ["a", "b", "", "c", "d"];
  assert.deepEqual(resolveScopeRange(lines, 2, "paragraph"), { startLine: 1, endLine: 2 });
  assert.deepEqual(resolveScopeRange(lines, 4, "paragraph"), { startLine: 4, endLine: 5 });
});

test("resolveScopeRange requires cursor on list/table for those scopes", () => {
  const lines = ["- a", "- b", "", "x", "| a | 1 |", "| b | 2 |"];
  assert.deepEqual(resolveScopeRange(lines, 2, "list"), { startLine: 1, endLine: 2 });
  assert.equal(resolveScopeRange(lines, 4, "list"), null);
  assert.deepEqual(resolveScopeRange(lines, 6, "table"), { startLine: 5, endLine: 6 });
  assert.equal(resolveScopeRange(lines, 4, "table"), null);
});

test("resolveScopeRange for doc spans entire file", () => {
  const lines = ["x", "y", "z"];
  assert.deepEqual(resolveScopeRange(lines, 2, "doc"), { startLine: 1, endLine: 3 });
});
