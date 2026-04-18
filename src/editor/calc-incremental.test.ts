import test, { before } from "node:test";
import assert from "node:assert/strict";
import { planIncrementalCalc } from "./calc-incremental.ts";
import { ensureWasmReady } from "./wasm.ts";

before(async () => {
  await ensureWasmReady();
});

test("planIncrementalCalc reuses unchanged prefix and suffix", () => {
  const prevLines = ["a", "2+2", "tail"];
  const prevResults = new Map<number, string>([[1, "4"]]);
  const nextLines = ["a", "3+3", "tail"];

  const plan = planIncrementalCalc(prevLines, prevResults, nextLines);
  assert.equal(plan.evalFrom, 1);
  assert.deepEqual(plan.evalLines, ["3+3"]);
  assert.equal(plan.baseResults.has(1), false);
});

test("planIncrementalCalc shifts suffix results on line insert", () => {
  const prevLines = ["head", "2+2", "tail"];
  const prevResults = new Map<number, string>([
    [1, "4"],
    [2, "10"],
  ]);
  const nextLines = ["head", "new", "2+2", "tail"];

  const plan = planIncrementalCalc(prevLines, prevResults, nextLines);
  assert.equal(plan.evalFrom, 1);
  assert.deepEqual(plan.evalLines, ["new"]);
  assert.equal(plan.baseResults.get(2), "4");
  assert.equal(plan.baseResults.get(3), "10");
});

test("planIncrementalCalc carries suffix results on delete-only edits", () => {
  const prevLines = ["head", "drop", "tail"];
  const prevResults = new Map<number, string>([
    [1, "4"],
    [2, "10"],
  ]);
  const nextLines = ["head", "tail"];

  const plan = planIncrementalCalc(prevLines, prevResults, nextLines);
  assert.equal(plan.evalFrom, 1);
  assert.deepEqual(plan.evalLines, []);
  assert.equal(plan.baseResults.has(1), true);
  assert.equal(plan.baseResults.get(1), "10");
  assert.equal(plan.baseResults.has(2), false);
});

test("planIncrementalCalc evaluates full doc when there is no previous snapshot", () => {
  const plan = planIncrementalCalc([], new Map(), ["2+2", "3+3"]);
  assert.equal(plan.evalFrom, 0);
  assert.deepEqual(plan.evalLines, ["2+2", "3+3"]);
});
