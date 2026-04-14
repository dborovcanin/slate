import test, { before } from "node:test";
import assert from "node:assert/strict";
import { Text } from "@codemirror/state";
import {
  buildMarkdownDecorationsForSpans,
  classifyMarkdownLine,
  findInlineMarkdownTokens,
  findVariableNameRanges,
  tokenizeCodeLine,
} from "./markdown-decoration.ts";
import { ensureWasmReady } from "./wasm.ts";

before(async () => {
  await ensureWasmReady();
});

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

function collectDecorations(decos: ReturnType<typeof buildMarkdownDecorationsForSpans>) {
  const out: Array<{ from: number; to: number; cls: string }> = [];
  const cursor = decos.iter();
  while (cursor.value) {
    const spec = cursor.value.spec as { class?: string; attributes?: { class?: string } };
    const cls = spec.class ?? spec.attributes?.class ?? "";
    out.push({ from: cursor.from, to: cursor.to, cls });
    cursor.next();
  }
  return out;
}

test("buildMarkdownDecorationsForSpans only decorates lines inside visible spans", () => {
  const doc = Text.of([
    "# Outside heading",
    "- outside list",
    "# Inside heading",
    "- inside list",
    "# Also outside",
  ]);

  const decos = buildMarkdownDecorationsForSpans(
    doc,
    [{ fromLine: 3, toLine: 4 }],
    [],
  );
  const flat = collectDecorations(decos);

  const insideHeading = doc.line(3);
  const insideList = doc.line(4);
  const outsideFirst = doc.line(1);
  const outsideLast = doc.line(5);

  assert.ok(
    flat.some((d) => d.from >= insideHeading.from && d.to <= insideHeading.to && d.cls.includes("md-heading")),
    "inside heading should be decorated",
  );
  assert.ok(
    flat.some((d) => d.from >= insideList.from && d.to <= insideList.to && d.cls.includes("md-list")),
    "inside list should be decorated",
  );
  assert.ok(
    !flat.some((d) => d.from >= outsideFirst.from && d.to <= outsideFirst.to),
    "outside-before heading should not be decorated",
  );
  assert.ok(
    !flat.some((d) => d.from >= outsideLast.from && d.to <= outsideLast.to),
    "outside-after heading should not be decorated",
  );
});

test("buildMarkdownDecorationsForSpans recovers fence state when viewport starts inside a code block", () => {
  const doc = Text.of([
    "prose before",
    "```rust",
    "let x = 1;",
    "fn main() {}",
    "```",
    "prose after",
  ]);

  const decos = buildMarkdownDecorationsForSpans(
    doc,
    [{ fromLine: 3, toLine: 4 }],
    [],
  );
  const flat = collectDecorations(decos);

  const line3 = doc.line(3);
  const line4 = doc.line(4);

  assert.ok(
    flat.some((d) => d.from === line3.from && d.cls.includes("md-code-block-line")),
    "line inside fenced block should get code-block line class",
  );
  assert.ok(
    flat.some((d) => d.from >= line3.from && d.to <= line3.to && d.cls.includes("md-code-token-keyword")),
    "keywords inside the fenced block should be highlighted with the correct language",
  );
  assert.ok(
    flat.some((d) => d.from === line4.from && d.cls.includes("md-code-block-line")),
    "second body line should also be treated as code",
  );
});

test("buildMarkdownDecorationsForSpans stays stable for calc variable expressions with underscores", () => {
  const doc = Text.of([
    "# Plan",
    "",
    "money_prior := 182980 = 182980",
    "money_now := 186633 = 186633",
    "",
    "total := money_now - money_prior = 3653",
    "",
    "## Features",
    "- [ ] Context menu",
  ]);

  let decos: ReturnType<typeof buildMarkdownDecorationsForSpans> | null = null;
  assert.doesNotThrow(() => {
    decos = buildMarkdownDecorationsForSpans(
      doc,
      [{ fromLine: 1, toLine: doc.lines }],
      [{ normalized: "money_prior" }, { normalized: "money_now" }, { normalized: "total" }],
    );
  });

  const flat = collectDecorations(decos!);
  const heading = doc.line(1);
  const checklist = doc.line(9);
  assert.ok(
    flat.some(
      (d) => d.from >= heading.from && d.to <= heading.to && d.cls.includes("md-heading"),
    ),
    "heading formatting should remain active",
  );
  assert.ok(
    flat.some((d) => d.from === checklist.from && d.cls.includes("md-checklist-item")),
    "checklist formatting should remain active",
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
