import test from "node:test";
import assert from "node:assert/strict";
import { parseCollectionTagsInput } from "./tags.ts";

test("parseCollectionTagsInput trims, deduplicates, and preserves first-seen order", () => {
  assert.deepEqual(
    parseCollectionTagsInput(" alpha, Beta,alpha,  , BETA, gamma "),
    ["alpha", "Beta", "gamma"],
  );
});

test("parseCollectionTagsInput returns empty list for blank input", () => {
  assert.deepEqual(parseCollectionTagsInput(" , ,  "), []);
});
