import test from "node:test";
import assert from "node:assert/strict";
import type { EditOperation } from "./types.ts";
import {
  rewriteLineWithChecklistToggleSuffix,
  runDocChangeRules,
  runEnterRules,
  runTabRules,
} from "./text-rules.ts";

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

  const orderedContinueText = "1.1 task";
  const orderedContinueSnapshot = {
    text: orderedContinueText,
    selection: { anchor: orderedContinueText.length, head: orderedContinueText.length },
  };
  const orderedContinueOp = runEnterRules(orderedContinueSnapshot, { markdownAutoformat: true });
  assert.ok(orderedContinueOp);
  assert.equal(applyOperation(orderedContinueText, orderedContinueOp), "1.1 task\n1.2 ");

  const exitText = "- ";
  const exitSnapshot = {
    text: exitText,
    selection: { anchor: exitText.length, head: exitText.length },
  };
  const exitOp = runEnterRules(exitSnapshot, { markdownAutoformat: true });
  assert.ok(exitOp);
  assert.equal(applyOperation(exitText, exitOp), "");
});

test("runTabRules indents and outdents markdown list items for sublists", () => {
  const text = "- parent\n  - child\nplain";
  const indentOp = runTabRules(
    {
      text,
      selection: { anchor: 0, head: "  - child".length + 3 },
    },
    { markdownAutoformat: true },
  );
  assert.ok(indentOp);
  assert.equal(applyOperation(text, indentOp), "  * parent\n    -> child\nplain");

  const outdentOp = runTabRules(
    {
      text: "  * parent\n    -> child",
      selection: { anchor: 0, head: "  * parent\n    -> child".length },
    },
    { markdownAutoformat: true, outdent: true },
  );
  assert.ok(outdentOp);
  assert.equal(applyOperation("  * parent\n    -> child", outdentOp), "- parent\n  * child");
});

test("runTabRules changes ordered list marker depth to hierarchical form", () => {
  const text = "1. parent";
  const indentOp = runTabRules(
    {
      text,
      selection: { anchor: text.length, head: text.length },
    },
    { markdownAutoformat: true },
  );
  assert.ok(indentOp);
  assert.equal(applyOperation(text, indentOp), "  1.1 parent");

  const outdentOp = runTabRules(
    {
      text: "  1.1.1 child",
      selection: { anchor: 0, head: "  1.1.1 child".length },
    },
    { markdownAutoformat: true, outdent: true },
  );
  assert.ok(outdentOp);
  assert.equal(applyOperation("  1.1.1 child", outdentOp), "1.1 child");
});
