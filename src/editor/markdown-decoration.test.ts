import test, { before } from "node:test";
import assert from "node:assert/strict";
import { Text } from "@codemirror/state";
import {
  FenceCheckpointCache,
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
  const out: Array<{
    from: number;
    to: number;
    cls: string;
    widget: string;
    widgetRef: unknown;
  }> = [];
  const cursor = decos.iter();
  while (cursor.value) {
    const spec = cursor.value.spec as {
      class?: string;
      attributes?: { class?: string };
      widget?: { constructor?: { name?: string } };
    };
    const cls = spec.class ?? spec.attributes?.class ?? "";
    const widget = spec.widget?.constructor?.name ?? "";
    out.push({ from: cursor.from, to: cursor.to, cls, widget, widgetRef: spec.widget });
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

test("buildMarkdownDecorationsForSpans uses cached fence state for viewport-only analysis", () => {
  const doc = Text.of([
    "before",
    "```rust",
    "let x = 1;",
    "fn main() {}",
    "```",
    "after",
  ]);
  const cache = new FenceCheckpointCache(2);

  const decos = buildMarkdownDecorationsForSpans(
    doc,
    [{ fromLine: 3, toLine: 4 }],
    [],
    undefined,
    {
      getFenceStateBeforeLine: (lineNumber) =>
        cache.getStateBeforeLine(doc, lineNumber),
    },
  );
  const flat = collectDecorations(decos);
  const line3 = doc.line(3);

  assert.ok(
    flat.some((d) => d.from === line3.from && d.cls.includes("md-code-block-line")),
    "cached fence state should keep viewport lines inside code block",
  );
  assert.ok(
    flat.some(
      (d) =>
        d.from >= line3.from &&
        d.to <= line3.to &&
        d.cls.includes("md-code-token-keyword"),
    ),
    "cached fence state should preserve fenced language highlighting",
  );
});

test("buildMarkdownDecorationsForSpans cached fence state does not leak past closing fence", () => {
  const doc = Text.of([
    "```rust",
    "let x = 1;",
    "```",
    "# heading after fence",
  ]);
  const cache = new FenceCheckpointCache(2);

  const decos = buildMarkdownDecorationsForSpans(
    doc,
    [{ fromLine: 4, toLine: 4 }],
    [],
    undefined,
    {
      getFenceStateBeforeLine: (lineNumber) =>
        cache.getStateBeforeLine(doc, lineNumber),
    },
  );
  const flat = collectDecorations(decos);
  const heading = doc.line(4);

  assert.ok(
    flat.some(
      (d) =>
        d.from >= heading.from &&
        d.to <= heading.to &&
        d.cls.includes("md-heading"),
    ),
    "line after closing fence should render as heading",
  );
  assert.equal(
    flat.some((d) => d.from === heading.from && d.cls.includes("md-code-block-line")),
    false,
    "line after closing fence must not be marked as code block",
  );
});

test("FenceCheckpointCache invalidation recomputes state after edits above viewport", () => {
  const before = Text.of([
    "intro",
    "```",
    "inside",
    "still inside",
    "outside later",
  ]);
  const after = Text.of([
    "intro",
    "plain",
    "inside",
    "still inside",
    "outside later",
  ]);
  const cache = new FenceCheckpointCache(2);

  const beforeState = cache.getStateBeforeLine(before, 5);
  assert.equal(beforeState.inCodeBlock, true, "baseline should be inside fence");

  cache.invalidateFromLine(2);
  const afterState = cache.getStateBeforeLine(after, 5);
  assert.equal(
    afterState.inCodeBlock,
    false,
    "state should be recomputed after invalidation from changed line",
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

test("buildMarkdownDecorationsForSpans handles unsorted visible spans", () => {
  const doc = Text.of([
    "# Header",
    "plain",
    "more",
    "- [ ] checklist",
  ]);

  const decos = buildMarkdownDecorationsForSpans(
    doc,
    [
      { fromLine: 4, toLine: 4 },
      { fromLine: 1, toLine: 1 },
    ],
    [],
  );
  const flat = collectDecorations(decos);

  const heading = doc.line(1);
  const checklist = doc.line(4);
  assert.ok(
    flat.some(
      (d) => d.from >= heading.from && d.to <= heading.to && d.cls.includes("md-heading"),
    ),
    "heading in the earlier span should still be decorated",
  );
  assert.ok(
    flat.some((d) => d.from === checklist.from && d.cls.includes("md-checklist-item")),
    "checklist in the later span should still be decorated",
  );
});

test("buildMarkdownDecorationsForSpans hides inline markdown markers until caret enters token", () => {
  const doc = Text.of(["**bold** tail"]);
  const line = doc.line(1);

  const hidden = buildMarkdownDecorationsForSpans(
    doc,
    [{ fromLine: 1, toLine: 1 }],
    [],
  );
  const hiddenFlat = collectDecorations(hidden);

  assert.ok(
    hiddenFlat.some(
      (d) => d.from === line.from && d.to === line.from + 2 && d.widget === "HiddenMarkdownTokenWidget",
    ),
    "opening strong marker should be hidden when caret is elsewhere",
  );
  assert.ok(
    hiddenFlat.some(
      (d) =>
        d.from === line.from + 6 &&
        d.to === line.from + 8 &&
        d.widget === "HiddenMarkdownTokenWidget",
    ),
    "closing strong marker should be hidden when caret is elsewhere",
  );

  const revealed = buildMarkdownDecorationsForSpans(
    doc,
    [{ fromLine: 1, toLine: 1 }],
    [],
    { from: line.from + 3, to: line.from + 3, empty: true },
  );
  const revealedFlat = collectDecorations(revealed);

  assert.equal(
    revealedFlat.some(
      (d) => d.from === line.from && d.to === line.from + 2 && d.widget === "HiddenMarkdownTokenWidget",
    ),
    false,
    "opening marker replacement should be removed when caret enters token content",
  );
  assert.equal(
    revealedFlat.some(
      (d) =>
        d.from === line.from + 6 &&
        d.to === line.from + 8 &&
        d.widget === "HiddenMarkdownTokenWidget",
    ),
    false,
    "closing marker replacement should be removed when caret enters token content",
  );

  const revealFromOpeningMarker = buildMarkdownDecorationsForSpans(
    doc,
    [{ fromLine: 1, toLine: 1 }],
    [],
    { from: line.from + 1, to: line.from + 1, empty: true },
  );
  const revealFromOpeningFlat = collectDecorations(revealFromOpeningMarker);
  assert.equal(
    revealFromOpeningFlat.some(
      (d) => d.from === line.from && d.to === line.from + 2 && d.widget === "HiddenMarkdownTokenWidget",
    ),
    false,
    "opening marker replacement should be removed when caret is inside opening marker",
  );
  assert.equal(
    revealFromOpeningFlat.some(
      (d) =>
        d.from === line.from + 6 &&
        d.to === line.from + 8 &&
        d.widget === "HiddenMarkdownTokenWidget",
    ),
    false,
    "closing marker replacement should also be removed when caret is inside opening marker",
  );

  const revealFromClosingMarker = buildMarkdownDecorationsForSpans(
    doc,
    [{ fromLine: 1, toLine: 1 }],
    [],
    { from: line.from + 6, to: line.from + 6, empty: true },
  );
  const revealFromClosingFlat = collectDecorations(revealFromClosingMarker);
  assert.equal(
    revealFromClosingFlat.some(
      (d) => d.from === line.from && d.to === line.from + 2 && d.widget === "HiddenMarkdownTokenWidget",
    ),
    false,
    "opening marker replacement should be removed when caret is inside closing marker",
  );
  assert.equal(
    revealFromClosingFlat.some(
      (d) =>
        d.from === line.from + 6 &&
        d.to === line.from + 8 &&
        d.widget === "HiddenMarkdownTokenWidget",
    ),
    false,
    "closing marker replacement should be removed when caret is inside closing marker",
  );

  const hideImmediatelyAfterClosingMarker = buildMarkdownDecorationsForSpans(
    doc,
    [{ fromLine: 1, toLine: 1 }],
    [],
    { from: line.from + 8, to: line.from + 8, empty: true },
  );
  const hideAfterClosingFlat = collectDecorations(hideImmediatelyAfterClosingMarker);
  assert.ok(
    hideAfterClosingFlat.some(
      (d) =>
        d.from === line.from &&
        d.to === line.from + 2 &&
        d.widget === "HiddenMarkdownTokenWidget",
    ),
    "opening marker should hide immediately once caret leaves the inline component",
  );
  assert.ok(
    hideAfterClosingFlat.some(
      (d) =>
        d.from === line.from + 6 &&
        d.to === line.from + 8 &&
        d.widget === "HiddenMarkdownTokenWidget",
    ),
    "closing marker should hide immediately once caret leaves the inline component",
  );
});

test("buildMarkdownDecorationsForSpans hides wiki-link source and shows only alt text when caret is outside", () => {
  const doc = Text.of(["[[01HX4VHR#Intro|My Alt]] tail"]);
  const line = doc.line(1);

  const decos = buildMarkdownDecorationsForSpans(
    doc,
    [{ fromLine: 1, toLine: 1 }],
    [],
    { from: line.to, to: line.to, empty: true },
  );
  const flat = collectDecorations(decos);

  const hasHidden = (from: number, to: number) =>
    flat.some(
      (d) => d.from === line.from + from && d.to === line.from + to && d.widget === "HiddenMarkdownTokenWidget",
    );

  assert.ok(hasHidden(0, 2), "opening [[ should be hidden");
  assert.ok(hasHidden(2, 10), "short-id should be hidden");
  assert.ok(hasHidden(10, 16), "anchor should be hidden");
  assert.ok(hasHidden(16, 17), "separator should be hidden");
  assert.ok(hasHidden(23, 25), "closing ]] should be hidden");
  assert.ok(
    flat.some(
      (d) =>
        d.from === line.from + 17 &&
        d.to === line.from + 23 &&
        d.cls.includes("md-wiki-link-title"),
    ),
    "alt text should remain visible and styled as link title",
  );
});

test("buildMarkdownDecorationsForSpans hides image markdown source and shows image display widget when caret is outside", () => {
  const doc = Text.of(["![Diagram](./assets/plan.png){width=320 height=180} tail"]);
  const line = doc.line(1);
  const decos = buildMarkdownDecorationsForSpans(
    doc,
    [{ fromLine: 1, toLine: 1 }],
    [],
    { from: line.to, to: line.to, empty: true },
  );
  const flat = collectDecorations(decos);

  const hasHidden = (from: number, to: number) =>
    flat.some(
      (d) => d.from === line.from + from && d.to === line.from + to && d.widget === "HiddenMarkdownTokenWidget",
    );

  assert.ok(hasHidden(0, 2), "opening ![ should be hidden");
  assert.ok(hasHidden(2, 9), "alt text source should be hidden");
  assert.ok(hasHidden(9, 11), "]( marker should be hidden");
  assert.ok(hasHidden(11, 28), "src should be hidden");
  assert.ok(hasHidden(28, 29), "closing ) should be hidden");
  assert.ok(hasHidden(29, 51), "resize attrs should be hidden");
  assert.ok(
    flat.some((d) => d.from === line.from && d.to === line.from && d.widget === "MarkdownImageDisplayWidget"),
    "image display widget should render outside edit mode",
  );
});

test("buildMarkdownDecorationsForSpans reveals raw image markdown when caret is inside image token", () => {
  const doc = Text.of(["![Diagram](./assets/plan.png){width=320} tail"]);
  const line = doc.line(1);
  const cursor = line.from + 34; // inside attrs
  const decos = buildMarkdownDecorationsForSpans(
    doc,
    [{ fromLine: 1, toLine: 1 }],
    [],
    { from: cursor, to: cursor, empty: true },
  );
  const flat = collectDecorations(decos);

  assert.ok(
    !flat.some((d) => d.widget === "MarkdownImageDisplayWidget"),
    "image display widget should disappear while editing raw image markdown",
  );
  assert.ok(
    !flat.some(
      (d) => d.widget === "HiddenMarkdownTokenWidget" && d.from >= line.from && d.to <= line.to,
    ),
    "no image source segments should be hidden while cursor is inside",
  );
});

test("buildMarkdownDecorationsForSpans shows image widget at right boundary of image token", () => {
  const doc = Text.of(["![Diagram](./assets/plan.png) tail"]);
  const line = doc.line(1);
  const rightBoundary = line.from + "![Diagram](./assets/plan.png)".length;
  const decos = buildMarkdownDecorationsForSpans(
    doc,
    [{ fromLine: 1, toLine: 1 }],
    [],
    { from: rightBoundary, to: rightBoundary, empty: true },
  );
  const flat = collectDecorations(decos);
  assert.ok(
    flat.some((d) => d.widget === "MarkdownImageDisplayWidget"),
    "image display widget should render when cursor is at right boundary",
  );
});

test("buildMarkdownDecorationsForSpans image widget carries compact [Image #n] token label", () => {
  const doc = Text.of(["![Diagram](./assets/plan.png) tail"]);
  const line = doc.line(1);
  const decos = buildMarkdownDecorationsForSpans(
    doc,
    [{ fromLine: 1, toLine: 1 }],
    [],
    { from: line.to, to: line.to, empty: true },
    {
      imagePreviewResolver: () => ({ srcUrl: null, broken: true }),
    },
  );
  const flat = collectDecorations(decos);
  const widget = flat.find((entry) => entry.widget === "MarkdownImageDisplayWidget");
  assert.ok(widget, "image display widget should render");
  assert.equal(
    (widget!.widgetRef as { tokenLabel?: string }).tokenLabel,
    "[Image #1: Diagram]",
  );
});

test("buildMarkdownDecorationsForSpans applies TUI-style decorations for markdown tables", () => {
  const doc = Text.of([
    "| Item | Preview |",
    "| --- | --- |",
    "| Plan | content |",
  ]);
  const line = doc.line(3);
  const decos = buildMarkdownDecorationsForSpans(
    doc,
    [{ fromLine: 1, toLine: 3 }],
    [],
    { from: line.to, to: line.to, empty: true },
  );
  const flat = collectDecorations(decos);

  assert.ok(
    flat.some((d) => d.cls.includes("md-table-header")),
    "header row should get header line decoration",
  );
  assert.ok(
    flat.some((d) => d.cls.includes("md-table-divider")),
    "divider row should get divider line decoration",
  );
  assert.ok(
    flat.some((d) => d.from === line.from && d.cls.includes("md-table-row")),
    "last body row should keep table row decoration",
  );
  assert.ok(
    flat.some((d) => d.cls.includes("md-table-pipe")),
    "pipe characters should get pipe decoration",
  );
});

test("buildMarkdownDecorationsForSpans keeps escaped pipes inside the same header cell", () => {
  const doc = Text.of([
    "| left \\| right | value |",
    "| --- | --- |",
    "| short | 1 |",
  ]);
  const header = doc.line(1);
  const decos = buildMarkdownDecorationsForSpans(
    doc,
    [{ fromLine: 1, toLine: 3 }],
    [],
    { from: header.to, to: header.to, empty: true },
  );
  const flat = collectDecorations(decos);

  // Escaped pipe at position (backslash + pipe) should NOT get a pipe decoration
  const escapedPipeIdx = header.text.indexOf("\\|");
  const escapedPipeDocPos = header.from + escapedPipeIdx + 1;
  assert.equal(
    flat.some((d) => d.cls.includes("md-table-pipe") && d.from === escapedPipeDocPos),
    false,
    "escaped pipe should not get pipe decoration",
  );
  // The leading real pipe should get pipe decoration
  assert.ok(
    flat.some((d) => d.cls.includes("md-table-pipe") && d.from === header.from),
    "leading pipe should get pipe decoration",
  );
});

test("buildMarkdownDecorationsForSpans reveals full wiki-link source inside [[...]] for editing", () => {
  const doc = Text.of(["[[01HX4VHR#Intro|My Alt]] tail"]);
  const line = doc.line(1);

  const decos = buildMarkdownDecorationsForSpans(
    doc,
    [{ fromLine: 1, toLine: 1 }],
    [],
    { from: line.from + 12, to: line.from + 12, empty: true },
  );
  const flat = collectDecorations(decos);

  const hasHidden = (from: number, to: number) =>
    flat.some(
      (d) => d.from === line.from + from && d.to === line.from + to && d.widget === "HiddenMarkdownTokenWidget",
    );

  assert.equal(hasHidden(2, 10), false, "short-id should be editable when caret is inside link");
  assert.equal(hasHidden(10, 16), false, "anchor should be editable when caret is inside link");
  assert.equal(hasHidden(16, 17), false, "separator should be editable when caret is inside link");
  assert.equal(hasHidden(23, 25), false, "closing marker should be revealed while editing link");
});

test("buildMarkdownDecorationsForSpans shows display widget for wiki-link without alt text when caret is outside", () => {
  const doc = Text.of(["[[01HX4VHR#Intro]] tail"]);
  const line = doc.line(1);

  const decos = buildMarkdownDecorationsForSpans(
    doc,
    [{ fromLine: 1, toLine: 1 }],
    [],
    { from: line.to, to: line.to, empty: true },
    {
      wikiLinkResolver: () => ({ exists: true, title: "Note Title" }),
    },
  );
  const flat = collectDecorations(decos);

  const hasHidden = (from: number, to: number) =>
    flat.some(
      (d) => d.from === line.from + from && d.to === line.from + to && d.widget === "HiddenMarkdownTokenWidget",
    );

  assert.ok(hasHidden(0, 2), "opening [[ should be hidden");
  assert.ok(hasHidden(2, 10), "short-id should be hidden");
  assert.ok(hasHidden(10, 16), "anchor should be hidden");
  assert.ok(hasHidden(16, 18), "closing ]] should be hidden");
  assert.ok(
    flat.some((d) => d.from === line.from && d.to === line.from && d.widget === "WikiLinkDisplayWidget"),
    "display widget should render resolved title",
  );
});

test("buildMarkdownDecorationsForSpans allows entering edit mode for wiki-link without alt text", () => {
  const doc = Text.of(["[[01HX4VHR#Intro]] tail"]);
  const line = doc.line(1);

  const decos = buildMarkdownDecorationsForSpans(
    doc,
    [{ fromLine: 1, toLine: 1 }],
    [],
    { from: line.from + 11, to: line.from + 11, empty: true },
    {
      wikiLinkResolver: () => ({ exists: true, title: "Note Title" }),
    },
  );
  const flat = collectDecorations(decos);

  assert.equal(
    flat.some((d) => d.from === line.from && d.to === line.from && d.widget === "WikiLinkDisplayWidget"),
    false,
    "display widget should disappear while editing raw link source",
  );
  assert.equal(
    flat.some(
      (d) =>
        d.from === line.from + 2 &&
        d.to === line.from + 10 &&
        d.widget === "HiddenMarkdownTokenWidget",
    ),
    false,
    "short-id should be visible/editable when caret is inside link span",
  );
});

test("buildMarkdownDecorationsForSpans reveals wiki-link source from right boundary cursor", () => {
  const doc = Text.of(["[[01HX4VHR#Intro]] tail"]);
  const line = doc.line(1);
  const linkEnd = line.from + "[[01HX4VHR#Intro]]".length;

  const decos = buildMarkdownDecorationsForSpans(
    doc,
    [{ fromLine: 1, toLine: 1 }],
    [],
    { from: linkEnd, to: linkEnd, empty: true },
    {
      wikiLinkResolver: () => ({ exists: true, title: "Note Title" }),
    },
  );
  const flat = collectDecorations(decos);

  assert.equal(
    flat.some((d) => d.from === line.from && d.to === line.from && d.widget === "WikiLinkDisplayWidget"),
    false,
    "display widget should hide when cursor is at the link right boundary",
  );
  assert.equal(
    flat.some(
      (d) =>
        d.from === line.from + 2 &&
        d.to === line.from + 10 &&
        d.widget === "HiddenMarkdownTokenWidget",
    ),
    false,
    "source should be editable from the right boundary position",
  );
});

test("buildMarkdownDecorationsForSpans hides heading and quote prefixes off-caret", () => {
  const doc = Text.of(["# heading", "> quote"]);
  const headingLine = doc.line(1);
  const quoteLine = doc.line(2);

  const hidden = buildMarkdownDecorationsForSpans(
    doc,
    [{ fromLine: 1, toLine: 2 }],
    [],
  );
  const hiddenFlat = collectDecorations(hidden);
  assert.ok(
    hiddenFlat.some(
      (d) =>
        d.from === headingLine.from &&
        d.to === headingLine.from + 2 &&
        d.widget === "HiddenMarkdownTokenWidget",
    ),
    "heading marker prefix should be hidden when line is not active",
  );
  assert.ok(
    hiddenFlat.some(
      (d) =>
        d.from === quoteLine.from &&
        d.to === quoteLine.from + 2 &&
        d.widget === "HiddenMarkdownTokenWidget",
    ),
    "quote marker prefix should be hidden when line is not active",
  );

  const revealQuote = buildMarkdownDecorationsForSpans(
    doc,
    [{ fromLine: 1, toLine: 2 }],
    [],
    { from: quoteLine.from + 3, to: quoteLine.from + 3, empty: true },
  );
  const revealQuoteFlat = collectDecorations(revealQuote);
  assert.equal(
    revealQuoteFlat.some(
      (d) =>
        d.from === quoteLine.from &&
        d.to === quoteLine.from + 2 &&
        d.widget === "HiddenMarkdownTokenWidget",
    ),
    false,
    "quote marker replacement should be removed for active quote line",
  );
});

test("buildMarkdownDecorationsForSpans keeps ordered list markers visible and stylizes unordered markers", () => {
  const doc = Text.of(["  - bullet", "  1.2 item"]);
  const bulletLine = doc.line(1);
  const orderedLine = doc.line(2);

  const decos = buildMarkdownDecorationsForSpans(
    doc,
    [{ fromLine: 1, toLine: 2 }],
    [],
  );
  const flat = collectDecorations(decos);
  assert.ok(
    flat.some(
      (d) =>
        d.from === orderedLine.from &&
        d.to === orderedLine.from + 6 &&
        d.cls.includes("md-token-list"),
    ),
    "ordered list prefix should remain visible via list token styling",
  );
  assert.ok(
    flat.some(
      (d) =>
        d.from === bulletLine.from + 2 &&
        d.to === bulletLine.from + 3 &&
        d.cls === "",
    ),
    "unordered marker symbol should be replaced with styled glyph",
  );

  const revealBulletMarker = buildMarkdownDecorationsForSpans(
    doc,
    [{ fromLine: 1, toLine: 2 }],
    [],
    { from: bulletLine.from + 2, to: bulletLine.from + 2, empty: true },
  );
  const revealBulletFlat = collectDecorations(revealBulletMarker);
  assert.equal(
    revealBulletFlat.some(
      (d) =>
        d.from === bulletLine.from + 2 &&
        d.to === bulletLine.from + 3 &&
        d.cls === "",
    ),
    false,
    "unordered source marker should be revealed while caret is on the marker",
  );
});

test("buildMarkdownDecorationsForSpans hides unordered checklist list marker prefix", () => {
  const doc = Text.of(["- [ ] checklist"]);
  const line = doc.line(1);

  const decos = buildMarkdownDecorationsForSpans(
    doc,
    [{ fromLine: 1, toLine: 1 }],
    [],
  );
  const flat = collectDecorations(decos);

  assert.ok(
    flat.some(
      (d) =>
        d.from === line.from &&
        d.to === line.from + 2,
    ),
    "unordered marker prefix range should be replaced for checklist line",
  );
});

test("buildMarkdownDecorationsForSpans reveals checklist syntax when cursor is in marker", () => {
  const doc = Text.of(["- [ ] checklist"]);
  const line = doc.line(1);

  const decos = buildMarkdownDecorationsForSpans(
    doc,
    [{ fromLine: 1, toLine: 1 }],
    [],
    { from: line.from + 1, to: line.from + 1, empty: true },
  );
  const flat = collectDecorations(decos);

  assert.equal(
    flat.some((d) => d.from === line.from && d.to === line.from + 2),
    false,
    "prefix replacement should be disabled when caret is in checklist marker region",
  );
  assert.equal(
    flat.some((d) => d.from === line.from + 2 && d.to === line.from + 5),
    false,
    "checkbox replacement should be disabled when caret is in checklist marker region",
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
