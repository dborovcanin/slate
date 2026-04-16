import assert from "node:assert/strict";
import test from "node:test";
import { describeFoldRanges } from "./folding.ts";
import { ensureWasmReady } from "./wasm.ts";

test("describeFoldRanges finds heading folds", async () => {
  await ensureWasmReady();
  const text = [
    "# One",
    "alpha",
    "## Two",
    "beta",
    "# Three",
    "gamma",
  ].join("\n");

  assert.deepEqual(describeFoldRanges(text), [
    { startLine: 1, endLine: 4, kind: "heading" },
    { startLine: 3, endLine: 6, kind: "heading" },
    { startLine: 5, endLine: 6, kind: "heading" },
  ]);
});

test("describeFoldRanges stops heading folds at same level only", async () => {
  await ensureWasmReady();
  const text = [
    "## Parent",
    "### Child",
    "details",
    "## Sibling",
    "tail",
  ].join("\n");

  assert.deepEqual(describeFoldRanges(text), [
    { startLine: 1, endLine: 3, kind: "heading" },
    { startLine: 2, endLine: 5, kind: "heading" },
    { startLine: 4, endLine: 5, kind: "heading" },
  ]);
});

test("describeFoldRanges finds fenced code fold and ignores heading inside code block", async () => {
  await ensureWasmReady();
  const text = [
    "# Top",
    "```ts",
    "## not-a-heading",
    "const x = 1",
    "```",
    "tail",
  ].join("\n");

  const ranges = describeFoldRanges(text);
  assert.deepEqual(ranges, [
    { startLine: 1, endLine: 6, kind: "heading" },
    { startLine: 2, endLine: 5, kind: "fence" },
  ]);
});
