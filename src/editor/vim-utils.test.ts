import test from "node:test";
import assert from "node:assert/strict";
import { computeBlockSpans } from "./vim-utils.ts";

test("computeBlockSpans builds inclusive block over multiple lines", () => {
  const spans = computeBlockSpans(
    [10, 10, 10],
    0,
    2,
    2,
    4,
  );

  assert.deepEqual(spans, [
    { lineIndex: 0, fromCol: 2, toCol: 5 },
    { lineIndex: 1, fromCol: 2, toCol: 5 },
    { lineIndex: 2, fromCol: 2, toCol: 5 },
  ]);
});

test("computeBlockSpans clamps to shorter lines", () => {
  const spans = computeBlockSpans(
    [2, 0, 8],
    0,
    1,
    2,
    4,
  );

  assert.deepEqual(spans, [
    { lineIndex: 0, fromCol: 1, toCol: 2 },
    { lineIndex: 1, fromCol: 0, toCol: 0 },
    { lineIndex: 2, fromCol: 1, toCol: 5 },
  ]);
});

test("computeBlockSpans handles reverse selection direction", () => {
  const forward = computeBlockSpans([8, 8, 8], 0, 1, 2, 3);
  const reverse = computeBlockSpans([8, 8, 8], 2, 3, 0, 1);
  assert.deepEqual(reverse, forward);
});
