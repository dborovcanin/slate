import test from "node:test";
import assert from "node:assert/strict";
import {
  classifyMarkdownLine,
  findInlineMarkdownTokens,
  findVariableNameRanges,
  tokenizeCodeLine,
} from "./markdown-decoration.ts";

test("classifyMarkdownLine detects heading, list, quote and fences", () => {
  const heading = classifyMarkdownLine("### Title");
  assert.equal(heading.headingLevel, 3);
  assert.equal(heading.headingMarkerEnd, 4);

  const indentedHeading = classifyMarkdownLine("  ## Title");
  assert.equal(indentedHeading.headingLevel, 2);
  assert.equal(indentedHeading.headingMarkerEnd, 5);

  const quote = classifyMarkdownLine("> quoted");
  assert.equal(quote.quoteMarkerEnd, 2);

  const tightQuote = classifyMarkdownLine(">quoted");
  assert.equal(tightQuote.quoteMarkerEnd, 1);

  const list = classifyMarkdownLine("- item");
  assert.equal(list.listMarkerEnd, 2);
  assert.equal(list.checklistMarkerStart, null);

  const nestedList = classifyMarkdownLine("  -> item");
  assert.equal(nestedList.listMarkerEnd, 5);

  const checklist = classifyMarkdownLine("- [x] done");
  assert.equal(checklist.listMarkerEnd, 2);
  assert.equal(checklist.checklistMarkerStart, 2);
  assert.equal(checklist.checklistMarkerEnd, 5);
  assert.equal(checklist.checklistChecked, true);
  assert.equal(checklist.checklistContentStart, 6);

  const orderedNested = classifyMarkdownLine("1.2 item");
  assert.equal(orderedNested.listMarkerEnd, 4);

  const fence = classifyMarkdownLine("```ts");
  assert.equal(fence.isCodeFence, true);
});

test("classifyMarkdownLine detects horizontal rule", () => {
  const rule = classifyMarkdownLine("---");
  assert.equal(rule.isHorizontalRule, true);
});

test("findInlineMarkdownTokens finds rich markdown spans", () => {
  const tokens = findInlineMarkdownTokens("**bold** *em* ~~gone~~ `code` [txt](url)");
  const types = new Set(tokens.map((t) => t.type));

  assert.ok(types.has("strong"));
  assert.ok(types.has("emphasis"));
  assert.ok(types.has("strikethrough"));
  assert.ok(types.has("code"));
  assert.ok(types.has("link-text"));
  assert.ok(types.has("link-url"));
});

test("tokenizeCodeLine marks keywords, numbers, strings, comments and symbols in fenced code", () => {
  const rust = tokenizeCodeLine('let total: Result = parse_value(42); let s = "ok" // note', "rust");
  const rustTypes = new Set(rust.map((token) => token.type));
  assert.ok(rustTypes.has("keyword"));
  assert.ok(rustTypes.has("string"));
  assert.ok(rustTypes.has("comment"));
  assert.ok(rustTypes.has("number"));
  assert.ok(rustTypes.has("function"));
  assert.ok(rustTypes.has("type"));

  const sh = tokenizeCodeLine("if [ $x -eq 1 ]; then # done", "sh");
  const shTypes = new Set(sh.map((token) => token.type));
  assert.ok(shTypes.has("keyword"));
  assert.ok(shTypes.has("comment"));
});

test("findVariableNameRanges finds variable references with boundaries", () => {
  const ranges = findVariableNameRanges("total := subtotal + tax rate", [
    { normalized: "total" },
    { normalized: "subtotal" },
    { normalized: "tax rate" },
  ]);

  assert.deepEqual(
    ranges.map((r) => [r.from, r.to]),
    [
      [0, 5],
      [9, 17],
      [20, 28],
    ],
  );
});

test("findVariableNameRanges is case-insensitive and prefers longest overlap", () => {
  const ranges = findVariableNameRanges("Tax Rate + tax + tax_rate", [
    { normalized: "tax" },
    { normalized: "tax rate" },
    { normalized: "tax_rate" },
  ]);

  assert.deepEqual(
    ranges.map((r) => [r.from, r.to]),
    [
      [0, 8],
      [11, 14],
      [17, 25],
    ],
  );
});
