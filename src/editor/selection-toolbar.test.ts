import test from "node:test";
import assert from "node:assert/strict";
import {
  clearInlineFormattingText,
  fallbackListLineText,
  fallbackTitleLineText,
  snippetForInsertBlock,
  toggleQuoteLineText,
} from "./selection-toolbar.ts";

test("clearInlineFormattingText removes common inline markdown markers", () => {
  assert.equal(
    clearInlineFormattingText("**bold** and *italic* with `code` plus ~~old~~"),
    "bold and italic with code plus old",
  );
});

test("fallbackTitleLineText preserves indentation and strips block prefixes", () => {
  assert.equal(fallbackTitleLineText("  - [x] ## Ship notes"), "  # Ship notes");
  assert.equal(fallbackTitleLineText("\t"), "\t# ");
});

test("fallbackListLineText creates lightweight line-local list transforms", () => {
  assert.equal(fallbackListLineText("  Ship notes", "unordered"), "  - Ship notes");
  assert.equal(fallbackListLineText("done", "checklist"), "- [ ] done");
  assert.equal(fallbackListLineText("old item", "ordered", 3), "3. old item");
});

test("toggleQuoteLineText toggles a single current-line quote prefix", () => {
  assert.equal(toggleQuoteLineText("  note"), "  > note");
  assert.equal(toggleQuoteLineText("  > note"), "  note");
});

test("snippetForInsertBlock returns cursor offsets inside editable block bodies", () => {
  const code = snippetForInsertBlock("code");
  assert.equal(code.text, "```\n\n```");
  assert.equal(code.cursorOffset, 4);

  const table = snippetForInsertBlock("table");
  assert.ok(table.text.includes("| Column 1 | Column 2 |"));
  assert.equal(table.text.slice(table.cursorOffset - 2, table.cursorOffset + 3), "|  | ");
});
