import test from "node:test";
import assert from "node:assert/strict";
import { BACKGROUND_PRESETS, clampFontSize, COLOR_SCHEMES, FONT_PRESETS } from "./presets.ts";

test("theme presets include at least a dozen color schemes", () => {
  const names = Object.keys(COLOR_SCHEMES);
  assert.ok(names.length >= 12);
  assert.ok(names.includes("gruvbox-dark"));
  assert.ok(names.includes("dracula"));
  assert.ok(names.includes("white"));
  assert.ok(names.includes("dark"));
  assert.ok(names.includes("solarized-dark"));
  assert.ok(names.includes("catppuccin-mocha"));
});

test("background presets include requested patterns", () => {
  const names = Object.keys(BACKGROUND_PRESETS);
  assert.ok(names.includes("plain"));
  assert.ok(names.includes("lines"));
  assert.ok(names.includes("squares"));
  assert.ok(names.includes("dots"));
});

test("font presets include multiple monospaced options", () => {
  const names = Object.keys(FONT_PRESETS);
  assert.ok(names.length >= 6);
  assert.ok(names.includes("jetbrains-mono"));
  assert.ok(names.includes("fira-code"));
  assert.ok(names.includes("cascadia-code"));
});

test("font size clamp keeps values in supported range", () => {
  assert.equal(clampFontSize(5), 11);
  assert.equal(clampFontSize(14), 14);
  assert.equal(clampFontSize(99), 28);
});
