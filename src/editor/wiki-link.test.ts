import test, { before } from "node:test";
import assert from "node:assert/strict";
import type { CompletionContext } from "@codemirror/autocomplete";
import type { EditorView } from "@codemirror/view";
import {
  createWikiLinkCompletionSource,
  tryNavigateWikiLinkFromMouseEvent,
} from "./wiki-link.ts";
import { ensureWasmReady } from "./wasm.ts";
import type { NoteSummary } from "../api.ts";

before(async () => {
  await ensureWasmReady();
});

function completionContext(text: string, from = 0): CompletionContext {
  return {
    matchBefore: () => ({ from, text }),
  } as unknown as CompletionContext;
}

test("wiki-link completion suggests notes and primes heading prompt", async () => {
  const note: NoteSummary = {
    id: "01HX4VHR9ABCDEFGHJKMNPQRS",
    title: "Section Notes",
    body_prefix: "",
    access_mode: "none",
    is_unlocked: true,
    updated_at: "",
  };
  const source = createWikiLinkCompletionSource({
    listNotesMeta: async () => [note],
    resolveWikiLink: async () => null,
    resolveWikiLinkHeadings: async () => [],
  });
  const result = await source(completionContext("[[sect"));
  assert.ok(result);
  assert.equal(result.options.length, 1);
  assert.equal(result.options[0].label, "Section Notes");

  const dispatched: Array<{ changes: { from: number; to: number; insert: string }; selection: { anchor: number } }> = [];
  const fakeView = {
    state: {
      doc: {
        sliceString: () => "]]",
      },
    },
    dispatch: (tx: { changes: { from: number; to: number; insert: string }; selection: { anchor: number } }) => {
      dispatched.push(tx);
    },
  } as unknown as EditorView;

  const apply = result.options[0].apply as (
    view: EditorView,
    completion: object,
    from: number,
    to: number,
  ) => void;
  apply(fakeView, {}, 0, 6);
  assert.equal(dispatched.length, 1);
  assert.equal(dispatched[0].changes.insert, "[[01HX4VHR#]]");
  assert.equal(dispatched[0].changes.to, 8);
  assert.equal(dispatched[0].selection.anchor, 11);
});

test("wiki-link completion suggests headings for short id + hash query", async () => {
  const source = createWikiLinkCompletionSource({
    listNotesMeta: async () => [],
    resolveWikiLink: async () => null,
    resolveWikiLinkHeadings: async () => ["Section 123", "Section 456: ABCD", "Appendix"],
  });
  const result = await source(completionContext("[[01HX4VHR#sec"));
  assert.ok(result);
  assert.equal(result.from, 11);
  assert.deepEqual(
    result.options.map((option) => option.label),
    ["Section 123", "Section 456: ABCD"],
  );
});

test("ctrl/meta click wiki-link navigation resolves and calls onNavigate", async () => {
  const navigations: Array<{ noteId: string; heading?: string }> = [];
  let prevented = false;
  let stopped = false;
  const handled = tryNavigateWikiLinkFromMouseEvent(
    {
      ctrlKey: true,
      metaKey: false,
      clientX: 1,
      clientY: 1,
      preventDefault: () => {
        prevented = true;
      },
      stopPropagation: () => {
        stopped = true;
      },
    } as unknown as MouseEvent,
    {
      posAtCoords: () => 3,
      state: {
        doc: {
          lineAt: () => ({ from: 0, text: "[[01HX4VHR#Intro]] tail" }),
        },
      },
    } as unknown as EditorView,
    (noteId, heading) => {
      navigations.push({ noteId, heading });
    },
    {
      listNotesMeta: async () => [],
      resolveWikiLink: async () => ({
        id: "01HX4VHR9ABCDEFGHJKMNPQRS",
        title: "Target Note",
        body_prefix: "",
        access_mode: "none",
        is_unlocked: true,
        updated_at: "",
      }),
      resolveWikiLinkHeadings: async () => [],
    },
  );

  assert.equal(handled, true);
  assert.equal(prevented, true);
  assert.equal(stopped, true);
  await Promise.resolve();
  assert.deepEqual(navigations, [{ noteId: "01HX4VHR9ABCDEFGHJKMNPQRS", heading: "Intro" }]);
});
