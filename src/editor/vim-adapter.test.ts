import assert from "node:assert/strict";
import test from "node:test";
import { VIM_KEY_KIND } from "./wasm.ts";
import { buildUiVimContext, toUiVimKeyInput } from "./vim-adapter.ts";

type EventLike = {
  key?: string;
  code?: string;
  ctrlKey?: boolean;
  altKey?: boolean;
  metaKey?: boolean;
};

function eventLike(overrides: EventLike): EventLike {
  return {
    key: "",
    code: "",
    ctrlKey: false,
    altKey: false,
    metaKey: false,
    ...overrides,
  };
}

test("toUiVimKeyInput maps navigation and control keys", () => {
  assert.deepEqual(toUiVimKeyInput(eventLike({ key: "Escape", code: "Escape" })), {
    kind: VIM_KEY_KIND.ESC,
  });
  assert.deepEqual(toUiVimKeyInput(eventLike({ key: "ArrowUp", code: "ArrowUp" })), {
    kind: VIM_KEY_KIND.ARROW_UP,
  });
  assert.deepEqual(toUiVimKeyInput(eventLike({ key: "Delete" })), {
    kind: VIM_KEY_KIND.DELETE,
  });
});

test("toUiVimKeyInput maps home/end and ctrl chords", () => {
  assert.deepEqual(toUiVimKeyInput(eventLike({ key: "Home", code: "Home" })), {
    kind: VIM_KEY_KIND.CHAR,
    charCode: "0".charCodeAt(0),
  });
  assert.deepEqual(toUiVimKeyInput(eventLike({ key: "End", code: "End" })), {
    kind: VIM_KEY_KIND.CHAR,
    charCode: "$".charCodeAt(0),
  });
  assert.deepEqual(toUiVimKeyInput(eventLike({ key: "W", ctrlKey: true })), {
    kind: VIM_KEY_KIND.CTRL,
    charCode: "w".charCodeAt(0),
  });
});

test("buildUiVimContext maps frontend context to wasm context shape", () => {
  assert.deepEqual(
    buildUiVimContext({
      hasSearchMatches: true,
      lineCount: 42,
    }),
    {
      has_search_matches: true,
      line_count: 42,
    },
  );
});
