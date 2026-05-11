import assert from "node:assert/strict";
import test from "node:test";
import { EditorState, Transaction } from "@codemirror/state";
import { history, isolateHistory } from "@codemirror/commands";
import type { EditorView } from "@codemirror/view";
import { __vimUndoRedoInternals } from "./vim.ts";

function createTestEditorView(doc: string): {
  view: EditorView;
  getState: () => EditorState;
} {
  let state = EditorState.create({
    doc,
    extensions: [history({ newGroupDelay: 300 })],
  });
  const view = {
    get state() {
      return state;
    },
    dispatch(spec: unknown) {
      state = state.update(spec as never).state;
    },
  } as unknown as EditorView;
  return { view, getState: () => state };
}

test("runUndoLikeTui reports oldest boundary when nothing is undoable", () => {
  const { view } = createTestEditorView("alpha\nbeta");
  const messages: string[] = [];
  const handled = __vimUndoRedoInternals.runUndoLikeTui(view, 1, (message) => messages.push(message));
  assert.equal(handled, true);
  assert.deepEqual(messages, ["already at oldest change"]);
});

test("runRedoLikeTui reports newest boundary when nothing is redoable", () => {
  const { view } = createTestEditorView("alpha\nbeta");
  const messages: string[] = [];
  const handled = __vimUndoRedoInternals.runRedoLikeTui(view, 1, (message) => messages.push(message));
  assert.equal(handled, true);
  assert.deepEqual(messages, ["already at newest change"]);
});

test("runUndoLikeTui and runRedoLikeTui apply counted history groups", () => {
  const { view, getState } = createTestEditorView("a");
  const applyIsolatedInsert = (text: string) => {
    view.dispatch({
      changes: { from: getState().doc.length, to: getState().doc.length, insert: text },
      annotations: isolateHistory.of("full"),
    });
  };

  applyIsolatedInsert("1");
  applyIsolatedInsert("2");
  applyIsolatedInsert("3");
  assert.equal(getState().doc.toString(), "a123");

  const undoMessages: string[] = [];
  __vimUndoRedoInternals.runUndoLikeTui(view, 2, (message) => undoMessages.push(message));
  assert.equal(getState().doc.toString(), "a1");
  assert.deepEqual(undoMessages, ["undo (1 left)"]);

  const redoMessages: string[] = [];
  __vimUndoRedoInternals.runRedoLikeTui(view, 1, (message) => redoMessages.push(message));
  assert.equal(getState().doc.toString(), "a12");
  assert.deepEqual(redoMessages, ["redo (1 left)"]);
});

test("runUndoLikeTui preserves cursor line/col on final undo step", () => {
  const { view, getState } = createTestEditorView("abc\ndef");
  view.dispatch({
    changes: { from: 0, to: 0, insert: "X" },
    annotations: isolateHistory.of("full"),
  });
  const lineTwoBefore = getState().doc.line(2);
  const targetCol = 2;
  view.dispatch({
    selection: { anchor: lineTwoBefore.from + targetCol },
    annotations: Transaction.addToHistory.of(false),
  });

  const messages: string[] = [];
  __vimUndoRedoInternals.runUndoLikeTui(view, 1, (message) => messages.push(message));
  const after = getState();
  const lineTwoAfter = after.doc.line(2);
  assert.equal(after.selection.main.head, lineTwoAfter.from + targetCol);
  assert.deepEqual(messages, ["undo (0 left)"]);
});

test("status sink receives undo and redo lifecycle messages", () => {
  const { view, getState } = createTestEditorView("x");
  view.dispatch({
    changes: { from: 1, to: 1, insert: "1" },
    annotations: isolateHistory.of("full"),
  });
  view.dispatch({
    changes: { from: 2, to: 2, insert: "2" },
    annotations: isolateHistory.of("full"),
  });
  assert.equal(getState().doc.toString(), "x12");

  const statusMessages: string[] = [];
  __vimUndoRedoInternals.runUndoLikeTui(view, 5, (message) => statusMessages.push(message));
  __vimUndoRedoInternals.runRedoLikeTui(view, 5, (message) => statusMessages.push(message));

  assert.deepEqual(statusMessages, ["undo (0 left)", "redo (0 left)"]);
  assert.equal(getState().doc.toString(), "x12");
});
