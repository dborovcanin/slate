import test from "node:test";
import assert from "node:assert/strict";
import { state } from "../state.ts";
import { __switcherInternals } from "./switcher.ts";

test("search filter resets to working collection on open", () => {
  state.setWorkingCollection({ id: "c-work", name: "Work" });
  __switcherInternals.setActiveCollectionFilterForTest(null);

  __switcherInternals.resetCollectionFilterFromWorkingCollectionForTest();

  assert.equal(__switcherInternals.getActiveCollectionFilterForTest(), "c-work");
  state.setWorkingCollection(null);
});

test("search filter resets to all when no working collection is set", () => {
  state.setWorkingCollection(null);
  __switcherInternals.setActiveCollectionFilterForTest("c-stale");

  __switcherInternals.resetCollectionFilterFromWorkingCollectionForTest();

  assert.equal(__switcherInternals.getActiveCollectionFilterForTest(), null);
});

test("Tab toggles between title and content switcher modes", () => {
  assert.equal(__switcherInternals.nextSwitcherModeForTest("title"), "content");
  assert.equal(__switcherInternals.nextSwitcherModeForTest("content"), "title");
});

test("Ctrl+L toggles collection filter between working collection and all", () => {
  state.setWorkingCollection({ id: "c-work", name: "Work" });
  __switcherInternals.setActiveCollectionFilterForTest(null);

  assert.equal(__switcherInternals.toggleCollectionFilterFromWorkingCollectionForTest(), true);
  assert.equal(__switcherInternals.getActiveCollectionFilterForTest(), "c-work");

  assert.equal(__switcherInternals.toggleCollectionFilterFromWorkingCollectionForTest(), true);
  assert.equal(__switcherInternals.getActiveCollectionFilterForTest(), null);

  state.setWorkingCollection(null);
});

test("Ctrl+L toggle helper is noop without working collection", () => {
  state.setWorkingCollection(null);
  __switcherInternals.setActiveCollectionFilterForTest(null);
  assert.equal(__switcherInternals.toggleCollectionFilterFromWorkingCollectionForTest(), false);
  assert.equal(__switcherInternals.getActiveCollectionFilterForTest(), null);
});
