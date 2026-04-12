import test from "node:test";
import assert from "node:assert/strict";
import { fuzzyFilter, fuzzyMatch } from "./fuzzy.ts";

test("fuzzyMatch returns positions for ordered characters", () => {
  const match = fuzzyMatch("nt", "Note");
  assert.ok(match);
  assert.deepEqual(match.positions, [0, 2]);
  assert.ok(match.score > 0);
});

test("fuzzyMatch returns null when query cannot be matched", () => {
  const match = fuzzyMatch("xyz", "Note");
  assert.equal(match, null);
});

test("fuzzyFilter sorts better matches first", () => {
  const items = ["my note", "new task", "other"];
  const results = fuzzyFilter("nt", items, (item) => item);
  assert.equal(results.length, 2);
  assert.equal(results[0].item, "new task");
  assert.equal(results[1].item, "my note");
});
