import test from "node:test";
import assert from "node:assert/strict";
import { listCommandSuggestions } from "./command-engine.ts";

test("editor mode exposes only editing commands", () => {
  const values = listCommandSuggestions("editor", "").map((entry) => entry.value);
  assert.deepEqual(values, ["sum", "sum list", "sum table", "sum doc", "date", "format"]);
  assert.equal(values.includes("q"), false);
});

test("vim mode exposes vim-specific commands", () => {
  const values = listCommandSuggestions("vim", "").map((entry) => entry.value);
  assert.equal(values.includes("q"), true);
});

test("command suggestions filter by query", () => {
  const values = listCommandSuggestions("editor", "fo").map((entry) => entry.value);
  assert.deepEqual(values, ["format"]);
});
