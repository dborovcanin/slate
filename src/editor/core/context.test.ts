import test from "node:test";
import assert from "node:assert/strict";
import { ResolvedContext } from "./context.ts";

function ctx(text: string, head: number, anchor = head) {
  return new ResolvedContext({ text, selection: { anchor, head } });
}

test("ResolvedContext line lookup follows cursor positions", () => {
  const resolved = ctx("a\nbc", 1);

  assert.equal(resolved.currentLine().number, 1);
  assert.equal(resolved.currentColumn(), 1);
  assert.equal(resolved.lineAt(2).number, 2);
  assert.equal(resolved.line(2).text, "bc");
});

test("ResolvedContext resolves paragraph, list, and table ranges", () => {
  const text = "- a\n- b\n\nc\nd\n\n| a | 1 |\n| b | 2 |";
  const resolved = ctx(text, 1);

  assert.deepEqual(resolved.listRangeAtLine(1), { startLine: 1, endLine: 2 });
  assert.deepEqual(resolved.paragraphRangeAtLine(4), { startLine: 4, endLine: 5 });
  assert.deepEqual(resolved.tableRangeAtLine(7), { startLine: 7, endLine: 8 });
  assert.equal(resolved.tableRangeAtLine(4), null);
});

test("ResolvedContext finds words around cursor", () => {
  const text = "hello world";
  assert.equal(ctx(text, 1).wordAt()?.text, "hello");
  assert.equal(ctx(text, 5).wordAt()?.text, "hello");
  assert.equal(ctx(text, 6).wordAt()?.text, "world");
  assert.equal(ctx(text, 11).wordAt()?.text, "world");
});
