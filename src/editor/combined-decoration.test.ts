import test, { before } from "node:test";
import assert from "node:assert/strict";
import type { DecorationSet, EditorView } from "@codemirror/view";
import { combinedVsStandaloneForTest } from "./markdown-decoration.ts";
import { makeCalcDecorationTestState } from "./calc-decoration.ts";
import { ensureWasmReady } from "./wasm.ts";

before(async () => {
  await ensureWasmReady();
});

// Stable, comparable signature for every decoration in a set, in document order.
// Captures position + the distinguishing spec fields (class / widget kind / side
// / block), which is enough to detect any structural divergence between the
// combined single-pass build and the standalone builders.
function decorationSignatures(set: DecorationSet): string[] {
  const out: string[] = [];
  const cursor = set.iter();
  while (cursor.value !== null) {
    const spec = (cursor.value.spec ?? {}) as Record<string, unknown>;
    const widget = spec.widget as { constructor?: { name?: string } } | undefined;
    out.push(
      JSON.stringify({
        from: cursor.from,
        to: cursor.to,
        class: spec.class ?? null,
        tagName: spec.tagName ?? null,
        side: spec.side ?? null,
        block: spec.block ?? null,
        widget: widget ? (widget.constructor?.name ?? "widget") : null,
      }),
    );
    cursor.next();
  }
  return out;
}

// 200-line doc: a code fence opens before and closes inside the markdown
// viewport window, and calc results are scattered across the (wider) calc
// window — so the parity check exercises margin slicing (markdown 24 vs calc 80)
// and fence-state seeding across the merged union span.
function buildFixtureDoc(): { text: string; calcResults: Array<[number, string]> } {
  const lines: string[] = [];
  for (let i = 1; i <= 200; i++) {
    if (i === 50) lines.push("```js");
    else if (i === 90) lines.push("```");
    else if (i >= 51 && i <= 89) lines.push(`const x${i} = ${i} + 1;`);
    else if (i % 17 === 0) lines.push(`## Heading ${i}`);
    else if (i % 13 === 0) lines.push(`- [ ] task ${i}`);
    else if (i % 11 === 0) lines.push(`| a${i} | b${i} |`);
    else if (i % 7 === 0) lines.push(`> quote ${i} with **bold** and \`code\``);
    else lines.push(`line ${i} with [[ABCD1234]] and *italic* text`);
  }
  // 0-based line indices spread across the calc window (some outside the md one).
  const calcResults: Array<[number, string]> = [
    [29, "42"],
    [54, "7"],
    [110, "9"],
    [149, "100"],
    [164, "3.14"],
  ];
  return { text: lines.join("\n"), calcResults };
}

const MD_MARGIN = 24;
const CALC_MARGIN = 80;

function fixtureView(): { view: EditorView; midLine: number } {
  const { text, calcResults } = buildFixtureDoc();
  const state = makeCalcDecorationTestState({
    doc: text,
    results: calcResults,
    variableIndex: [{ normalized: "italic" }] as never,
    selectionHead: 0,
  });
  // Visible range = a single line in the middle, so the markdown 24-line margin
  // is a strict subset of the calc 80-line margin.
  const midLine = 100;
  const mid = state.doc.line(midLine);
  const visibleRanges = [{ from: mid.from, to: state.doc.line(midLine + 1).to }] as const;
  return { view: { state, visibleRanges } as unknown as EditorView, midLine };
}

test("combined viewport pass matches standalone markdown + calc builders", () => {
  const { view } = fixtureView();
  const { combinedMarkdown, standaloneMarkdown, combinedCalc, standaloneCalc } =
    combinedVsStandaloneForTest(view);

  const combinedMd = decorationSignatures(combinedMarkdown);
  const standaloneMd = decorationSignatures(standaloneMarkdown);
  const combinedC = decorationSignatures(combinedCalc);
  const standaloneC = decorationSignatures(standaloneCalc);

  // Sanity: the fixture actually produces decorations on both layers, otherwise
  // the parity assertions below would pass trivially.
  assert.ok(combinedMd.length > 0, "expected markdown decorations in fixture");
  assert.ok(combinedC.length > 0, "expected calc decorations in fixture");

  assert.deepEqual(combinedMd, standaloneMd, "markdown decoration parity");
  assert.deepEqual(combinedC, standaloneC, "calc decoration parity");
});

test("combined pass slices each layer to its own margin", () => {
  const { view, midLine } = fixtureView();
  const { combinedMarkdown, combinedCalc } = combinedVsStandaloneForTest(view);
  const doc = view.state.doc;

  // Every markdown decoration must fall inside the markdown viewport window.
  const mdFrom = doc.line(Math.max(1, midLine - MD_MARGIN)).from;
  const mdTo = doc.line(Math.min(doc.lines, midLine + 1 + MD_MARGIN)).to;
  const mdCursor = combinedMarkdown.iter();
  while (mdCursor.value !== null) {
    assert.ok(
      mdCursor.from >= mdFrom && mdCursor.to <= mdTo,
      `markdown decoration at [${mdCursor.from}, ${mdCursor.to}] outside md window [${mdFrom}, ${mdTo}]`,
    );
    mdCursor.next();
  }

  // Calc results at lines 30 and 150 are outside the markdown window but inside
  // the calc 80-line window — so the calc layer must carry decorations beyond
  // the markdown window on both sides.
  const calcFrom = doc.line(Math.max(1, midLine - CALC_MARGIN)).from;
  const calcTo = doc.line(Math.min(doc.lines, midLine + 1 + CALC_MARGIN)).to;
  let sawBelowMd = false;
  let sawAboveMd = false;
  const calcCursor = combinedCalc.iter();
  while (calcCursor.value !== null) {
    assert.ok(
      calcCursor.from >= calcFrom && calcCursor.to <= calcTo,
      `calc decoration outside calc window`,
    );
    if (calcCursor.from < mdFrom) sawBelowMd = true;
    if (calcCursor.from > mdTo) sawAboveMd = true;
    calcCursor.next();
  }
  assert.ok(sawBelowMd, "expected calc decorations below the markdown window");
  assert.ok(sawAboveMd, "expected calc decorations above the markdown window");
});
