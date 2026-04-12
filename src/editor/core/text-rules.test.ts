import test from "node:test";
import assert from "node:assert/strict";
import type { EditOperation } from "./types.ts";
import { rewriteLineWithChecklistToggleSuffix, runDocChangeRules, runEnterRules } from "./text-rules.ts";

function applyOperation(source: string, operation: EditOperation): string {
  const changes = [...operation.changes].sort((a, b) => b.from - a.from);
  let next = source;
  for (const change of changes) {
    next = `${next.slice(0, change.from)}${change.insert}${next.slice(change.to)}`;
  }
  return next;
}

test("rewriteLineWithChecklistToggleSuffix toggles and converts list items", () => {
  assert.equal(rewriteLineWithChecklistToggleSuffix("- [ ] task /x"), "- [x] task");
  assert.equal(rewriteLineWithChecklistToggleSuffix("- [x] task /x"), "- [ ] task");
  assert.equal(rewriteLineWithChecklistToggleSuffix("1. task /x"), "1. [x] task");
  assert.equal(rewriteLineWithChecklistToggleSuffix("- path/x"), null);
});

test("runDocChangeRules applies checklist toggle even with autoformat off", () => {
  const text = "- [ ] task /x";
  const snapshot = {
    text,
    selection: { anchor: text.length, head: text.length },
  };
  const op = runDocChangeRules(snapshot, { markdownAutoformat: false });
  assert.ok(op);
  assert.equal(applyOperation(text, op), "- [x] task");
});

test("runDocChangeRules formats markdown tables when enabled", () => {
  const text = "| a | b |\n| --- | --- |\n| 1 | 2 |";
  const snapshot = {
    text,
    selection: { anchor: text.length, head: text.length },
  };
  const op = runDocChangeRules(snapshot, { markdownAutoformat: true });
  assert.ok(op);
  assert.equal(applyOperation(text, op), "| a   | b   |\n| --- | --- |\n| 1   | 2   |");
});

test("runEnterRules continues and exits markdown lists", () => {
  const continueText = "- task";
  const continueSnapshot = {
    text: continueText,
    selection: { anchor: continueText.length, head: continueText.length },
  };
  const continueOp = runEnterRules(continueSnapshot, { markdownAutoformat: true });
  assert.ok(continueOp);
  assert.equal(applyOperation(continueText, continueOp), "- task\n- ");

  const exitText = "- ";
  const exitSnapshot = {
    text: exitText,
    selection: { anchor: exitText.length, head: exitText.length },
  };
  const exitOp = runEnterRules(exitSnapshot, { markdownAutoformat: true });
  assert.ok(exitOp);
  assert.equal(applyOperation(exitText, exitOp), "");
});
