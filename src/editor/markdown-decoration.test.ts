import test from "node:test";
import assert from "node:assert/strict";
import { classifyMarkdownLine, findInlineMarkdownTokens } from "./markdown-decoration.ts";

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
