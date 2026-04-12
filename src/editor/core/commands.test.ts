import test from "node:test";
import assert from "node:assert/strict";
import { executeCommand, listCommandSuggestions } from "./commands.ts";

function snapshot(text: string, head = 0, anchor = head) {
  return { text, selection: { anchor, head } };
}

test("core command suggestions are mode-aware", () => {
  const editorValues = listCommandSuggestions("editor", "").map((entry) => entry.value);
  assert.deepEqual(editorValues, ["sum", "sum list", "sum table", "sum doc", "date", "format"]);

  const vimValues = listCommandSuggestions("vim", "").map((entry) => entry.value);
  assert.equal(vimValues.includes("q"), true);
});

test("core executeCommand computes sum and returns insertion operation", async () => {
  const result = await executeCommand(snapshot("10\n20", 0), "sum", {
    mode: "editor",
  });

  assert.equal(result.message.includes("sum(paragraph) = 30"), true);
  assert.equal(result.operations.length, 1);
  assert.deepEqual(result.operations[0]?.changes[0], { from: 0, to: 0, insert: "30" });
  assert.deepEqual(result.operations[0]?.selection, { anchor: 2 });
});

test("core executeCommand handles date and mode-gated q", async () => {
  const dateResult = await executeCommand(snapshot("", 0), "date", {
    mode: "editor",
    pickDate: async () => "2026-04-12",
  });
  assert.equal(dateResult.message, "inserted 2026-04-12");
  assert.equal(dateResult.operations.length, 1);

  const editorQ = await executeCommand(snapshot("", 0), "q", { mode: "editor" });
  assert.equal(editorQ.message, "unknown command: q");

  let quitCalled = false;
  const vimQ = await executeCommand(snapshot("", 0), "q", {
    mode: "vim",
    onQuit: async () => {
      quitCalled = true;
    },
  });
  assert.equal(vimQ.message, "quit");
  assert.equal(quitCalled, true);
});
