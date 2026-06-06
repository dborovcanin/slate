import test, { before } from "node:test";
import assert from "node:assert/strict";
import { EditorState, type ChangeSet } from "@codemirror/state";
import type { DecorationSet } from "@codemirror/view";
import {
  FenceCheckpointCache,
  buildMarkdownDecorationsForSpans,
  createVariableMatcher,
} from "./markdown-decoration.ts";
import {
  buildCalcDecorationsForSpans,
  makeCalcDecorationTestState,
} from "./calc-decoration.ts";
import { spliceDecorations } from "./decoration-share.ts";
import { ensureWasmReady } from "./wasm.ts";

before(async () => {
  await ensureWasmReady();
});

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

function buildMd(state: EditorState, fromLine: number, toLine: number): DecorationSet {
  const doc = state.doc;
  const fence = new FenceCheckpointCache();
  const sel = state.selection.main;
  return buildMarkdownDecorationsForSpans(
    doc,
    [{ fromLine, toLine }],
    [],
    { from: sel.from, to: sel.to, empty: sel.empty, assoc: sel.assoc },
    {
      getFenceStateBeforeLine: (n) => fence.getStateBeforeLine(doc, n),
      variableMatcher: createVariableMatcher([]),
    },
  );
}

function buildCalc(state: EditorState, fromLine: number, toLine: number): DecorationSet {
  return buildCalcDecorationsForSpans(state, [{ fromLine, toLine }]);
}

// Apply a single-line edit and assert that mapping the old whole-doc build
// through the change and splicing in a rebuild of just the edited line yields
// the same decorations as a full rebuild of the new document — for both layers.
function assertSpliceParity(opts: {
  name: string;
  lines: string[];
  results?: Array<[number, string]>;
  editLine: number; // 1-based
  from: number; // column within the line (0-based char offset)
  to: number;
  insert: string;
}): void {
  const oldText = opts.lines.join("\n");
  const lineStartProbe = EditorState.create({ doc: oldText }).doc.line(opts.editLine).from;
  const fromPos = lineStartProbe + opts.from;
  const toPos = lineStartProbe + opts.to;
  // Cursor sits on the edited line before the edit too (typing happens where the
  // caret already is), so selection-dependent reveal matches the full rebuild.
  const oldState = makeCalcDecorationTestState({
    doc: oldText,
    results: opts.results,
    selectionHead: fromPos,
  });
  const changes: ChangeSet = oldState.changes({
    from: fromPos,
    to: toPos,
    insert: opts.insert,
  });
  const newDoc = oldState.doc.toString();
  const newText =
    newDoc.slice(0, fromPos) + opts.insert + newDoc.slice(toPos);
  const cursor = fromPos + opts.insert.length;
  const newState = makeCalcDecorationTestState({
    doc: newText,
    results: opts.results,
    selectionHead: cursor,
  });

  const lineCount = newState.doc.lines;
  const editLineNew = newState.doc.lineAt(cursor).number;
  // Rebuild the edited line plus one neighbour each side: selection-dependent
  // marker reveal can associate with an adjacent line at a line boundary.
  const regionFromLine = Math.max(1, editLineNew - 1);
  const regionToLine = Math.min(lineCount, editLineNew + 1);
  const regionFrom = newState.doc.line(regionFromLine).from;
  const regionTo = newState.doc.line(regionToLine).to;

  for (const [name, buildFn] of [
    ["markdown", buildMd],
    ["calc", buildCalc],
  ] as const) {
    const full = buildFn(newState, 1, lineCount);
    const mappedOld = buildFn(oldState, 1, oldState.doc.lines).map(changes);
    const rebuilt = buildFn(newState, regionFromLine, regionToLine);
    const delta = spliceDecorations(mappedOld, regionFrom, regionTo, rebuilt);
    assert.deepEqual(
      decorationSignatures(delta),
      decorationSignatures(full),
      `${opts.name} / ${name} splice parity`,
    );
  }
}

test("splice parity: type a char in a plain paragraph line", () => {
  assertSpliceParity({
    name: "plain insert",
    lines: ["# Title", "alpha beta gamma", "more text here", "tail"],
    editLine: 2,
    from: 5,
    to: 5,
    insert: "X",
  });
});

test("splice parity: edit a heading line", () => {
  assertSpliceParity({
    name: "heading edit",
    lines: ["intro", "## Section heading", "body"],
    editLine: 2,
    from: 10,
    to: 10,
    insert: "ing more",
  });
});

test("splice parity: introduce an inline marker (bold)", () => {
  assertSpliceParity({
    name: "inline bold",
    lines: ["one", "make this bold soon", "two"],
    editLine: 2,
    from: 0,
    to: 0,
    insert: "**bold** ",
  });
});

test("splice parity: edit a line carrying a calc trailer", () => {
  assertSpliceParity({
    name: "calc trailer line",
    lines: ["top", "income times two", "bottom"],
    results: [[1, "200"]], // 0-based line index 1 -> the middle line
    editLine: 2,
    from: 0,
    to: 0,
    insert: "the ",
  });
});

test("splice parity: delete characters in a list item", () => {
  assertSpliceParity({
    name: "list delete",
    lines: ["- [ ] first task", "- [ ] second task here", "- [ ] third"],
    editLine: 2,
    from: 17,
    to: 22,
    insert: "",
  });
});
