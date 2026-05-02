import test from "node:test";
import assert from "node:assert/strict";
import { __commandPickerInternals } from "./command-picker.ts";
import type { CommandSuggestion } from "./command-engine.ts";
import type { ListOverlayState } from "../overlays/overlay.ts";

function stateFixture(
  query: string,
  items: CommandSuggestion[],
  selectedIndex = 0,
): {
  state: ListOverlayState<CommandSuggestion>;
  refreshCount: () => number;
  renderCount: () => number;
} {
  let refreshes = 0;
  let renders = 0;
  const inputEl = { value: query } as HTMLInputElement;
  const state: ListOverlayState<CommandSuggestion> = {
    query,
    items,
    selectedIndex,
    inputEl,
    render: () => { renders += 1; },
    refresh: () => { refreshes += 1; },
  };
  return { state, refreshCount: () => refreshes, renderCount: () => renders };
}

test("Tab-selected completion fills highlighted command", () => {
  const { state, refreshCount } = stateFixture("fo", [
    { value: "fold", description: "fold block at cursor" },
    { value: "format", description: "format markdown" },
  ], 1);

  const changed = __commandPickerInternals.applySelectedCommandCompletion(state);
  assert.equal(changed, true);
  assert.equal(state.inputEl.value, "format");
  assert.equal(state.selectedIndex, 1);
  assert.equal(refreshCount(), 0);
});

test("selected completion keeps exact selected command text", () => {
  const { state, refreshCount } = stateFixture("format", [
    { value: "format", description: "format markdown" },
  ], 0);

  const changed = __commandPickerInternals.applySelectedCommandCompletion(state);
  assert.equal(changed, true);
  assert.equal(state.inputEl.value, "format");
  assert.equal(refreshCount(), 0);
});

test("cycle selection advances index and renders without refresh", () => {
  const { state, refreshCount, renderCount } = stateFixture("fo", [
    { value: "fold", description: "fold block at cursor" },
    { value: "format", description: "format markdown" },
    { value: "fold-toggle", description: "toggle fold block" },
  ], 1);

  const changed = __commandPickerInternals.cycleSuggestionSelection(state, false);
  assert.equal(changed, true);
  assert.equal(state.selectedIndex, 2);
  assert.equal(renderCount(), 1);
  assert.equal(refreshCount(), 0);
});

test("single-word completion still expands unique token", () => {
  const { state, refreshCount } = stateFixture("module v", [
    { value: "module variables on", description: "enable variables module" },
    { value: "module variables off", description: "disable variables module" },
  ], 0);

  const changed = __commandPickerInternals.applySingleWordCompletion(state);
  assert.equal(changed, true);
  assert.equal(state.inputEl.value, "module variables ");
  assert.equal(state.selectedIndex, 0);
  assert.equal(refreshCount(), 1);
});
