import test, { before } from "node:test";
import assert from "node:assert/strict";
import { executeCommand, listCommandSuggestions } from "./command-engine.ts";
import { ensureWasmReady } from "./wasm.ts";

before(async () => { await ensureWasmReady(); });

test("editor mode exposes only editing commands", async () => {
  const values = listCommandSuggestions("editor", "").map((entry) => entry.value);
  assert.deepEqual(values, [
    "sum",
    "sum list",
    "sum row",
    "sum column",
    "sum doc",
    "avg",
    "avg list",
    "avg row",
    "avg column",
    "avg doc",
    "date",
    "notify",
    "notify-delete",
    "module status",
    "module math on",
    "module math off",
    "module math toggle",
    "module table on",
    "module table off",
    "module table toggle",
    "module variables on",
    "module variables off",
    "module variables toggle",
    "module style on",
    "module style off",
    "module style toggle",
    "format",
    "clip-watch",
    "clip-watch-stop",
    "fold",
    "unfold",
    "fold-toggle",
    "clist",
    "ulist",
    "olist",
    "note lock",
    "note unlock",
    "note encrypt",
    "note decrypt",
    "note unprotect",
  ]);
  assert.equal(values.includes("q"), false);
});

test("perf suggestions appear only when perf prefix is typed", async () => {
  const emptyValues = listCommandSuggestions("editor", "").map((entry) => entry.value);
  assert.equal(emptyValues.includes("perf status"), false);

  const perfValues = listCommandSuggestions("editor", "perf").map((entry) => entry.value);
  assert.deepEqual(perfValues, [
    "perf status",
    "perf on",
    "perf off",
    "perf dump",
    "perf where",
    "perf clear",
  ]);
});

test("vim mode exposes vim-specific commands", async () => {
  const values = listCommandSuggestions("vim", "").map((entry) => entry.value);
  assert.equal(values.includes("q"), true);
  assert.equal(values.includes("w"), true);
  assert.equal(values.includes("wq"), true);
});

test("command suggestions filter by query", async () => {
  const values = listCommandSuggestions("editor", "fo").map((entry) => entry.value);
  assert.deepEqual(values, ["fold", "fold-toggle", "format", "unfold"]);
});

test("vim write surfaces host save errors", async () => {
  const message = await executeCommand({} as any, "w", {
    mode: "vim",
    onWriteCommand: async () => {
      throw new Error("note changed since last load; use :w! to force save");
    },
  });
  assert.equal(message, "write failed: note changed since last load; use :w! to force save");
});
