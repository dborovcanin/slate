import test from "node:test";
import assert from "node:assert/strict";
import { formatDateByPattern } from "./date-picker.ts";

test("formatDateByPattern supports strftime-style tokens", () => {
  const date = new Date("2026-07-03T00:00:00Z");
  assert.equal(formatDateByPattern(date, "%Y-%m-%d"), "2026-07-03");
  assert.equal(formatDateByPattern(date, "%d.%m.%Y"), "03.07.2026");
  assert.equal(formatDateByPattern(date, "%m/%d/%Y"), "07/03/2026");
  assert.equal(formatDateByPattern(date, "%B %d, %Y"), "July 03, 2026");
});

test("formatDateByPattern supports common uppercase tokens", () => {
  const date = new Date("2026-11-14T00:00:00Z");
  assert.equal(formatDateByPattern(date, "DD/MM/YYYY"), "14/11/2026");
  assert.equal(formatDateByPattern(date, "MMMM D, YYYY"), "November 14, 2026");
});
