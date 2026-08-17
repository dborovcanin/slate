import test from "node:test";
import assert from "node:assert/strict";
import { computeBlockSpans, vimCharClass } from "./vim-utils.ts";

// Classes mirror `char_class` in crates/editor-core/src/vim_actions.rs:
// 0 = whitespace, 1 = word (alphanumeric or underscore), 2 = everything else.
test("vimCharClass classifies ASCII the same way the core classifier does", () => {
  for (const ch of ["a", "Z", "0", "9", "_"]) {
    assert.equal(vimCharClass(ch), 1, `${ch} should be a word character`);
  }
  for (const ch of [" ", "\t", "\n", "\r"]) {
    assert.equal(vimCharClass(ch), 0, `${JSON.stringify(ch)} should be whitespace`);
  }
  for (const ch of ["-", ".", "|", "*", "("]) {
    assert.equal(vimCharClass(ch), 2, `${ch} should be punctuation`);
  }
  assert.equal(vimCharClass(""), 0);
});

// Rust's char::is_alphanumeric is Unicode-aware. An ASCII-only classifier here
// makes w/b/e stop in different columns in the UI than in the TUI.
test("vimCharClass treats non-ASCII letters and digits as word characters", () => {
  for (const ch of ["é", "ü", "ñ", "Ж", "日", "한", "α", "²"]) {
    assert.equal(vimCharClass(ch), 1, `${ch} should be a word character`);
  }
});

test("vimCharClass treats non-ASCII whitespace and symbols as non-word", () => {
  assert.equal(vimCharClass(" "), 0, "no-break space is whitespace");
  assert.equal(vimCharClass("　"), 0, "ideographic space is whitespace");
  for (const ch of ["·", "—", "€", "→"]) {
    assert.equal(vimCharClass(ch), 2, `${ch} should be punctuation`);
  }
});

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
