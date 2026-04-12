import test from "node:test";
import assert from "node:assert/strict";
import { parseNumbers, parseScope } from "./ex-commands.ts";

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
