import test from "node:test";
import assert from "node:assert/strict";
import {
  buildVariableSuggestions,
  extractCompletionPrefix,
} from "./variable-autocomplete.ts";
import type { VariableIndexEntry } from "../api.ts";

test("extractCompletionPrefix resolves current query span", () => {
  const prefix = extractCompletionPrefix("total cost + tax", 10);
  assert.deepEqual(prefix, {
    fromCol: 0,
    toCol: 10,
    query: "total cost",
  });
});

test("extractCompletionPrefix trims leading spaces and rejects trailing spaces", () => {
  const withLeading = extractCompletionPrefix("   total", 8);
  assert.deepEqual(withLeading, {
    fromCol: 3,
    toCol: 8,
    query: "total",
  });

  assert.equal(extractCompletionPrefix("total ", 6), null);
});

test("buildVariableSuggestions enforces min chars and excludes exact match", () => {
  const vars: VariableIndexEntry[] = [
    { name: "Total Cost", normalized: "total cost", line: 1 },
    { name: "Tax", normalized: "tax", line: 2 },
    { name: "Total Cost", normalized: "total cost", line: 4 },
    { name: "Total Revenue", normalized: "total revenue", line: 3 },
  ];

  assert.deepEqual(buildVariableSuggestions(vars, "to", 3, 8), []);

  const picks = buildVariableSuggestions(vars, "tot", 3, 8).map((entry) => entry.normalized);
  assert.deepEqual(picks, ["total cost", "total revenue"]);

  assert.deepEqual(buildVariableSuggestions(vars, "total cost", 3, 8), []);
});
