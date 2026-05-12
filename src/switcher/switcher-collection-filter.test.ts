import test from "node:test";
import assert from "node:assert/strict";
import { state } from "../state.ts";
import { __switcherInternals } from "./switcher.ts";

test("content search filter resets to working collection on open", () => {
  state.setWorkingCollection({ id: "c-work", name: "Work" });
  __switcherInternals.setActiveCollectionFilterForTest(null);

  __switcherInternals.resetContentCollectionFilterFromWorkingCollectionForTest();

  assert.equal(__switcherInternals.getActiveCollectionFilterForTest(), "c-work");
  state.setWorkingCollection(null);
});

test("content search filter resets to all when no working collection is set", () => {
  state.setWorkingCollection(null);
  __switcherInternals.setActiveCollectionFilterForTest("c-stale");

  __switcherInternals.resetContentCollectionFilterFromWorkingCollectionForTest();

  assert.equal(__switcherInternals.getActiveCollectionFilterForTest(), null);
});

test("Tab toggles between title and content switcher modes", () => {
  assert.equal(__switcherInternals.nextSwitcherModeForTest("title"), "content");
  assert.equal(__switcherInternals.nextSwitcherModeForTest("content"), "title");
});
