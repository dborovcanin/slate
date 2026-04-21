import test, { before } from "node:test";
import assert from "node:assert/strict";
import { listCommandSuggestions } from "./command-engine.ts";
import { ensureWasmReady } from "./wasm.ts";

before(async () => { await ensureWasmReady(); });

test("editor mode exposes only editing commands", async () => {
  const values = listCommandSuggestions("editor", "").map((entry) => entry.value);
  assert.deepEqual(values, [
    "perf status",
    "perf on",
    "perf off",
    "perf dump",
    "perf where",
    "perf clear",
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
    "module on math",
    "module off math",
    "module toggle math",
    "module on table",
    "module off table",
    "module toggle table",
    "module on variables",
    "module off variables",
    "module toggle variables",
    "module on style",
    "module off style",
    "module toggle style",
    "format",
    "clip-watch",
    "clip-watch-stop",
    "fold",
    "unfold",
    "fold-toggle",
    "clist",
    "ulist",
    "olist",
  ]);
  assert.equal(values.includes("q"), false);
});

test("vim mode exposes vim-specific commands", async () => {
  const values = listCommandSuggestions("vim", "").map((entry) => entry.value);
  assert.equal(values.includes("q"), true);
});

test("command suggestions filter by query", async () => {
  const values = listCommandSuggestions("editor", "fo").map((entry) => entry.value);
  assert.deepEqual(values, ["fold", "fold-toggle", "format", "unfold"]);
});
