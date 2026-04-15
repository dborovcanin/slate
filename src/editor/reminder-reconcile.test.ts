import test from "node:test";
import assert from "node:assert/strict";
import { Text } from "@codemirror/state";
import type { NoteReminder } from "../api.ts";
import { reconcileReminderLinesOnOpen, remindersEqual } from "./reminder-reconcile.ts";

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
