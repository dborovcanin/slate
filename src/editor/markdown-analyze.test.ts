import assert from "node:assert/strict";
import test, { before } from "node:test";
import { ensureWasmReady, markdownAnalyzeLines } from "./wasm.ts";

before(async () => {
  await ensureWasmReady();
});

const FRESH = { inCodeBlock: false, codeFenceLang: null };

test("markdownAnalyzeLines decodes line info from the flat payload", () => {
  const result = markdownAnalyzeLines(
    ["# Title", "- [x] done", "plain text", "---"],
    FRESH,
  );
  assert.equal(result.lines.length, 4);

  assert.equal(result.lines[0]!.info.headingLevel, 1);
  assert.equal(result.lines[0]!.info.headingMarkerEnd, 2);

  const checklist = result.lines[1]!.info;
  assert.equal(checklist.checklistMarkerStart, 2);
  assert.equal(checklist.checklistMarkerEnd, 5);
  assert.equal(checklist.checklistContentStart, 6);
  assert.equal(checklist.checklistChecked, true);

  // Non-heading line reports null (not 0) for absent optional fields.
  assert.equal(result.lines[2]!.info.headingLevel, null);
  assert.equal(result.lines[3]!.info.isHorizontalRule, true);
});

test("markdownAnalyzeLines decodes inline tokens with correct types and ranges", () => {
  const result = markdownAnalyzeLines(["**bold** and `code`"], FRESH);
  const tokens = result.lines[0]!.inlineTokens;
  assert.ok(tokens.length > 0);
  // Every token has a valid string type and a non-decreasing range.
  for (const token of tokens) {
    assert.equal(typeof token.type, "string");
    assert.ok(token.from <= token.to);
  }
  assert.ok(tokens.some((t) => t.type === "strong"));
  assert.ok(tokens.some((t) => t.type === "code" || t.type === "code-marker"));
});

test("markdownAnalyzeLines tracks fence state and decodes the lang string table", () => {
  const result = markdownAnalyzeLines(
    ["```rust", "let x = 1;", "```", "after"],
    FRESH,
  );
  assert.equal(result.lines[0]!.info.isCodeFence, true);
  assert.equal(result.lines[1]!.inCodeBlock, true);
  assert.equal(result.lines[1]!.codeFenceLang, "rust");
  // Code line inside the fence carries code tokens (e.g. the `let` keyword).
  assert.ok(result.lines[1]!.codeTokens.some((t) => t.type === "keyword"));
  assert.equal(result.lines[3]!.inCodeBlock, false);
  assert.equal(result.finalInCodeBlock, false);
});

test("markdownAnalyzeLines carries the starting fence state into the chunk", () => {
  const result = markdownAnalyzeLines(["still code", "```"], {
    inCodeBlock: true,
    codeFenceLang: "python",
  });
  assert.equal(result.lines[0]!.inCodeBlock, true);
  assert.equal(result.lines[0]!.codeFenceLang, "python");
  // Closing fence ends the block.
  assert.equal(result.finalInCodeBlock, false);
});

test("markdownAnalyzeLines handles an empty chunk", () => {
  const result = markdownAnalyzeLines([], FRESH);
  assert.deepEqual(result.lines, []);
  assert.equal(result.finalInCodeBlock, false);
  assert.equal(result.finalCodeFenceLang, null);
});
