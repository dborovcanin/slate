import test from "node:test";
import assert from "node:assert/strict";
import { __collectionSwitcherInternals } from "./collection-switcher.ts";

test("collection switcher filters by name and description", () => {
  __collectionSwitcherInternals.setItemsForTest([
    {
      id: null,
      name: "All collections",
      description: "Clear active session collection",
      isAll: true,
    },
    {
      id: "c1",
      name: "Projects",
      description: "Work items",
      isAll: false,
    },
    {
      id: "c2",
      name: "Personal",
      description: "Home tasks",
      isAll: false,
    },
  ]);

  const results = __collectionSwitcherInternals.buildItemsForTest("work");
  assert.equal(results.length, 1);
  assert.equal(results[0]?.item.name, "Projects");
});

test("collection switcher selection maps All to null collection", () => {
  const all = __collectionSwitcherInternals.toCollectionResultForTest({
    id: null,
    name: "All collections",
    description: "Clear active session collection",
    isAll: true,
  });
  assert.equal(all, null);

  const project = __collectionSwitcherInternals.toCollectionResultForTest({
    id: "c1",
    name: "Projects",
    description: "Work items",
    isAll: false,
  });
  assert.deepEqual(project, {
    id: "c1",
    name: "Projects",
    description: "Work items",
    created_at: "",
    updated_at: "",
  });
});

test("collection switcher uses Ctrl+E for edit shortcut", () => {
  assert.equal(
    __collectionSwitcherInternals.isCollectionEditShortcutForTest({
      ctrlKey: true,
      shiftKey: false,
      altKey: false,
      key: "e",
    } as KeyboardEvent),
    true,
  );
  assert.equal(
    __collectionSwitcherInternals.isCollectionEditShortcutForTest({
      ctrlKey: true,
      shiftKey: true,
      altKey: false,
      key: "e",
    } as KeyboardEvent),
    false,
  );
});
