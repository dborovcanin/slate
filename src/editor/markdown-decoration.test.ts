import test from "node:test";
import assert from "node:assert/strict";
import { classifyMarkdownLine, findInlineMarkdownTokens } from "./markdown-decoration.ts";

test("classifyMarkdownLine detects heading, list, quote and fences", () => {
  const heading = classifyMarkdownLine("### Title");
  assert.equal(heading.headingLevel, 3);
  assert.equal(heading.headingMarkerEnd, 4);

  const quote = classifyMarkdownLine("> quoted");
  assert.equal(quote.quoteMarkerEnd, 2);

  const list = classifyMarkdownLine("- item");
  assert.equal(list.listMarkerEnd, 2);

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
