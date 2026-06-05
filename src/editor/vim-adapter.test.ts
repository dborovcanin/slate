import assert from "node:assert/strict";
import test, { before } from "node:test";
import { VIM_INTENT, VIM_KEY_KIND, VimSession, ensureWasmReady } from "./wasm.ts";
import { buildUiVimContext, runUiVimPipeline, toUiVimKeyInput } from "./vim-adapter.ts";

before(async () => {
  await ensureWasmReady();
});

type EventLike = {
  key?: string;
  code?: string;
  ctrlKey?: boolean;
  altKey?: boolean;
  metaKey?: boolean;
  getModifierState?: (keyArg: string) => boolean;
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

test("toUiVimKeyInput treats AltGraph printable chars as plain chars", () => {
  assert.deepEqual(
    toUiVimKeyInput(
      eventLike({
        key: "@",
        ctrlKey: true,
        altKey: true,
        getModifierState: (keyArg: string) => keyArg === "AltGraph",
      }),
    ),
    {
      kind: VIM_KEY_KIND.CHAR,
      charCode: "@".charCodeAt(0),
    },
  );
});

test("buildUiVimContext maps frontend context to wasm context shape", () => {
  assert.deepEqual(
    buildUiVimContext({
      hasSearchMatches: true,
      lineCount: 42,
      macroRecording: false,
    }),
    {
      has_search_matches: true,
      line_count: 42,
      macro_recording: false,
    },
  );
});

test("runUiVimPipeline stops macro recording when context reports active recording", () => {
  const session = new VimSession("normal");
  const baseContext = {
    hasSearchMatches: false,
    lineCount: 42,
    macroRecording: false,
  };
  const startPending = runUiVimPipeline(session, eventLike({ key: "q" }), baseContext);
  assert.equal(startPending.kind, "handled");
  const start = runUiVimPipeline(session, eventLike({ key: "a" }), baseContext);
  assert.equal(start.kind, "handled");
  assert.equal(start.step.actions[0]?.intent, VIM_INTENT.START_MACRO_RECORD);

  const stop = runUiVimPipeline(
    session,
    eventLike({ key: "q" }),
    { ...baseContext, macroRecording: true },
  );
  assert.equal(stop.kind, "handled");
  assert.equal(stop.step.actions[0]?.intent, VIM_INTENT.STOP_MACRO_RECORD);
});

test("VimSession exposes macro pending kind for UI diagnostics", () => {
  const session = new VimSession("normal");
  assert.equal(session.macroPendingKind(), 0);

  const q = toUiVimKeyInput(eventLike({ key: "q" }));
  assert.ok(q);
  session.step(q!, { has_search_matches: false, line_count: 1, macro_recording: false });
  assert.equal(session.macroPendingKind(), 1);

  const esc = toUiVimKeyInput(eventLike({ key: "Escape", code: "Escape" }));
  assert.ok(esc);
  session.step(esc!, { has_search_matches: false, line_count: 1, macro_recording: false });
  assert.equal(session.macroPendingKind(), 0);

  const at = toUiVimKeyInput(eventLike({ key: "@", code: "Digit2", ctrlKey: true, altKey: true, getModifierState: (keyArg: string) => keyArg === "AltGraph" }));
  assert.ok(at);
  session.step(at!, { has_search_matches: false, line_count: 1, macro_recording: false });
  assert.equal(session.macroPendingKind(), 2);
});

test("macro pending state clears when invalid register char is typed", () => {
  const session = new VimSession("normal");
  const baseContext = { hasSearchMatches: false, lineCount: 10, macroRecording: false };
  const startPending = runUiVimPipeline(session, eventLike({ key: "q" }), baseContext);
  assert.equal(startPending.kind, "handled");
  assert.equal(session.macroPendingKind(), 1);

  const invalid = runUiVimPipeline(session, eventLike({ key: "?" }), baseContext);
  assert.equal(invalid.kind, "handled");
  assert.equal(invalid.step.actions[0]?.intent, VIM_INTENT.SWALLOW);
  assert.equal(session.macroPendingKind(), 0);
});
