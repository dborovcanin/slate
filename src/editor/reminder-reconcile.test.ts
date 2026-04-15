import test from "node:test";
import assert from "node:assert/strict";
import { EditorState, Text } from "@codemirror/state";
import type { NoteReminder } from "../api.ts";
import {
  reconcileReminderLinesOnOpen,
  remindersEqual,
  remapReminderLinesForDocChange,
} from "./reminder-reconcile.ts";

function reminder(lineNumber: number, lineText: string): NoteReminder {
  return {
    note_id: "note-1",
    line_number: lineNumber,
    remind_at_ms: 1_777_000_000_000,
    display_at: "01.05.2026. 10:00",
    line_text: lineText,
    notified_at_ms: null,
    created_at: "2026-04-01T00:00:00Z",
    updated_at: "2026-04-01T00:00:00Z",
  };
}

test("reconcileReminderLinesOnOpen moves shifted reminders by matching line text", async () => {
  const doc = Text.of(["inserted", "alpha", "beta"]);
  const input = [reminder(1, "alpha")];
  const moves: Array<{ from: number; to: number; text: string }> = [];

  const reconciled = await reconcileReminderLinesOnOpen(
    "note-1",
    input,
    doc,
    async (_noteId, fromLineNumber, toLineNumber, lineText) => {
      moves.push({ from: fromLineNumber, to: toLineNumber, text: lineText });
      return true;
    },
  );

  assert.equal(reconciled.length, 1);
  assert.equal(reconciled[0]?.line_number, 2);
  assert.equal(reconciled[0]?.line_text, "alpha");
  assert.deepEqual(moves, [{ from: 1, to: 2, text: "alpha" }]);
});

test("reconcileReminderLinesOnOpen keeps reminder when line still matches", async () => {
  const doc = Text.of(["alpha"]);
  const input = [reminder(1, "alpha")];
  let moved = false;

  const reconciled = await reconcileReminderLinesOnOpen(
    "note-1",
    input,
    doc,
    async () => {
      moved = true;
      return true;
    },
  );

  assert.equal(reconciled.length, 1);
  assert.equal(reconciled[0]?.line_number, 1);
  assert.equal(moved, false);
});

test("reconcileReminderLinesOnOpen assigns duplicate texts to nearest distinct lines", async () => {
  const doc = Text.of(["x", "task", "y", "task", "z"]);
  const input = [reminder(1, "task"), reminder(2, "task")];
  const moves: Array<{ from: number; to: number }> = [];

  const reconciled = await reconcileReminderLinesOnOpen(
    "note-1",
    input,
    doc,
    async (_noteId, fromLineNumber, toLineNumber) => {
      moves.push({ from: fromLineNumber, to: toLineNumber });
      return true;
    },
  );

  assert.deepEqual(
    reconciled.map((entry) => entry.line_number),
    [2, 4],
  );
  assert.deepEqual(moves, [
    { from: 1, to: 2 },
    { from: 2, to: 4 },
  ]);
});

test("remindersEqual compares material reminder identity/position fields", () => {
  const left = [reminder(1, "alpha")];
  const right = [reminder(1, "alpha")];
  const different = [reminder(2, "alpha")];
  assert.equal(remindersEqual(left, right), true);
  assert.equal(remindersEqual(left, different), false);
});

test("remapReminderLinesForDocChange shifts reminder line down when line is inserted above", () => {
  const start = EditorState.create({ doc: "alpha\nbeta" });
  const tr = start.update({
    changes: { from: 0, to: 0, insert: "new\n" },
  });
  const remapped = remapReminderLinesForDocChange(
    new Map([[2, reminder(2, "beta")]]),
    tr.startState.doc,
    tr.changes,
    tr.newDoc,
  );
  assert.equal(remapped.get(3)?.line_number, 3);
  assert.equal(remapped.has(2), false);
});

test("remapReminderLinesForDocChange shifts reminder line up when line above is deleted", () => {
  const start = EditorState.create({ doc: "drop\nalpha\nbeta" });
  const firstLine = start.doc.line(1);
  const tr = start.update({
    changes: { from: firstLine.from, to: firstLine.to + 1, insert: "" },
  });
  const remapped = remapReminderLinesForDocChange(
    new Map([[2, reminder(2, "alpha")]]),
    tr.startState.doc,
    tr.changes,
    tr.newDoc,
  );
  assert.equal(remapped.get(1)?.line_number, 1);
  assert.equal(remapped.has(2), false);
});

test("remapReminderLinesForDocChange keeps reminders on distinct lines after collision", () => {
  const start = EditorState.create({ doc: "a\nb\nc" });
  const secondLine = start.doc.line(2);
  const tr = start.update({
    changes: { from: secondLine.from, to: secondLine.to + 1, insert: "" },
  });
  const remapped = remapReminderLinesForDocChange(
    new Map([
      [2, reminder(2, "b")],
      [3, reminder(3, "c")],
    ]),
    tr.startState.doc,
    tr.changes,
    tr.newDoc,
  );
  assert.equal(remapped.size, 2);
  const lineNumbers = [...remapped.values()]
    .map((entry) => entry.line_number)
    .sort((a, b) => a - b);
  assert.deepEqual(lineNumbers, [1, 2]);
});
