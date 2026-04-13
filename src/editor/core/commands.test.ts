import test from "node:test";
import assert from "node:assert/strict";
import { executeCommand, listCommandSuggestions } from "./commands.ts";

function snapshot(text: string, head = 0, anchor = head) {
  return { text, selection: { anchor, head } };
}

function formatValue(value: number): string {
  if (Number.isInteger(value)) return `${value}`;
  return value.toFixed(10).replace(/\.?0+$/, "");
}

async function unitAwareEvaluator(expression: string): Promise<string | null> {
  if (!/\d/.test(expression)) return null;
  const compact = expression.replace(/\s+/g, "").replace(/[()]/g, "");
  if (!compact) return null;

  const divisionMatch = compact.match(/^(.+)\/([-+]?\d+(?:\.\d+)?)$/);
  if (divisionMatch) {
    const left = divisionMatch[1] ?? "";
    const right = Number.parseFloat(divisionMatch[2] ?? "");
    if (!left || !Number.isFinite(right) || right === 0) return null;

    const leftMatch = left.match(/^([-+]?\d+(?:\.\d+)?)(km|m)?$/i);
    if (!leftMatch) return null;
    const value = Number.parseFloat(leftMatch[1] ?? "");
    const unit = (leftMatch[2] ?? "").toLowerCase();
    if (!Number.isFinite(value)) return null;
    if (unit === "m") return `${formatValue(value / right)}m`;
    if (unit === "km") return `${formatValue(value / right)}km`;
    return formatValue(value / right);
  }

  const terms = compact.split("+").filter(Boolean);
  if (terms.length === 0) return null;

  let unitAnchor: "" | "m" | "km" = "";
  let meterTotal = 0;
  let numberTotal = 0;

  for (const term of terms) {
    const match = term.match(/^([-+]?\d+(?:\.\d+)?)(km|m)?$/i);
    if (!match) return null;
    const value = Number.parseFloat(match[1] ?? "");
    const unit = (match[2] ?? "").toLowerCase();
    if ((unit === "m" || unit === "km") && unitAnchor === "") {
      unitAnchor = unit;
    }
    if (unit === "km") {
      meterTotal += value * 1000;
      continue;
    }
    if (unit === "m") {
      meterTotal += value;
      continue;
    }

    if (unitAnchor === "m") {
      meterTotal += value;
    } else if (unitAnchor === "km") {
      meterTotal += value * 1000;
    } else {
      numberTotal += value;
    }
  }

  if (unitAnchor === "m") return `${formatValue(meterTotal)}m`;
  if (unitAnchor === "km") return `${formatValue(meterTotal / 1000)}km`;
  return formatValue(numberTotal);
}

async function numberLeakingEvaluator(expression: string): Promise<string | null> {
  const trimmed = expression.trim().toLowerCase();
  if (trimmed === "number") return "28";

  const compact = trimmed.replace(/\s+/g, "").replace(/[()]/g, "");
  const divisionMatch = compact.match(/^(.+)\/([-+]?\d+(?:\.\d+)?)$/);
  if (divisionMatch) {
    const left = Number.parseFloat(divisionMatch[1] ?? "");
    const right = Number.parseFloat(divisionMatch[2] ?? "");
    if (!Number.isFinite(left) || !Number.isFinite(right) || right === 0) return null;
    return formatValue(left / right);
  }

  if (!/\d/.test(trimmed)) return null;
  const parts = trimmed.match(/[-+]?\d+(?:\.\d+)?/g) ?? [];
  if (parts.length === 0) return null;
  const total = parts.reduce((acc, token) => acc + Number.parseFloat(token), 0);
  return formatValue(total);
}

test("core command suggestions are mode-aware", () => {
  const editorValues = listCommandSuggestions("editor", "").map((entry) => entry.value);
  assert.deepEqual(editorValues, [
    "sum",
    "sum list",
    "sum row",
    "sum column",
    "sum doc",
    "avg",
    "avg list",
    "avg row",
    "avg column",
    "avg doc",
    "date",
    "format",
  ]);

  const vimValues = listCommandSuggestions("vim", "").map((entry) => entry.value);
  assert.equal(vimValues.includes("q"), true);
});

test("core executeCommand computes sum and returns insertion operation", async () => {
  const result = await executeCommand(snapshot("10\n20", 0), "sum", {
    mode: "editor",
  });

  assert.equal(result.message.includes("sum(paragraph) = 30.00"), true);
  assert.equal(result.operations.length, 1);
  assert.deepEqual(result.operations[0]?.changes[0], { from: 0, to: 0, insert: "30.00" });
  assert.deepEqual(result.operations[0]?.selection, { anchor: 5 });
});

test("core executeCommand handles date and mode-gated q", async () => {
  const dateResult = await executeCommand(snapshot("", 0), "date", {
    mode: "editor",
    pickDate: async () => "2026-04-12",
  });
  assert.equal(dateResult.message, "inserted 2026-04-12");
  assert.equal(dateResult.operations.length, 1);

  const editorQ = await executeCommand(snapshot("", 0), "q", { mode: "editor" });
  assert.equal(editorQ.message, "unknown command: q");

  let quitCalled = false;
  const vimQ = await executeCommand(snapshot("", 0), "q", {
    mode: "vim",
    onQuit: async () => {
      quitCalled = true;
    },
  });
  assert.equal(vimQ.message, "quit");
  assert.equal(quitCalled, true);
});

test("core executeCommand supports unit-aware sum row", async () => {
  const text = [
    "| item | a | b |",
    "| --- | --- | --- |",
    "| x | 2m | 2km |",
    "| y | 3m | 4m |",
  ].join("\n");
  const result = await executeCommand(snapshot(text, 0), "sum row", {
    mode: "editor",
    evaluateExpression: unitAwareEvaluator,
  });

  assert.equal(result.message.includes("sum(row)"), true);
  assert.equal(result.operations.length, 1);
  assert.deepEqual(result.operations[0]?.changes[0], { from: 0, to: 0, insert: "2002.00 m\n7.00 m" });
});

test("core executeCommand strips approximate wording and rounds unit totals", async () => {
  const text = [
    "| item | value |",
    "| --- | --- |",
    "| x | 2km |",
  ].join("\n");
  const result = await executeCommand(snapshot(text, 0), "sum row", {
    mode: "editor",
    evaluateExpression: async (expression) => {
      if (expression.trim() === "2km") return "approximately 2.004 km";
      return null;
    },
  });

  assert.equal(result.message.includes("sum(row)"), true);
  assert.equal(result.operations.length, 1);
  assert.deepEqual(result.operations[0]?.changes[0], { from: 0, to: 0, insert: "2.00 km" });
});

test("core executeCommand supports unit-aware sum column via underscore alias", async () => {
  const text = [
    "| item | a | b |",
    "| --- | --- | --- |",
    "| x | 2m | 2km |",
    "| y | 3m | 4m |",
  ].join("\n");
  const result = await executeCommand(snapshot(text, 0), "sum_column", {
    mode: "editor",
    evaluateExpression: unitAwareEvaluator,
  });

  assert.equal(result.message.includes("sum(column)"), true);
  assert.equal(result.operations.length, 1);
  assert.deepEqual(result.operations[0]?.changes[0], { from: 0, to: 0, insert: "5.00 m\n2.00 km" });
});

test("core executeCommand ignores non-numeric header cells for sum column", async () => {
  const text = [
    "| header | number |",
    "| ------ | ------ |",
    "| a      | 3      |",
    "| b      | 4      |",
  ].join("\n");
  const result = await executeCommand(snapshot(text, 0), "sum column", {
    mode: "editor",
    evaluateExpression: numberLeakingEvaluator,
  });

  assert.equal(result.message.includes("sum(column)"), true);
  assert.equal(result.operations.length, 1);
  assert.deepEqual(result.operations[0]?.changes[0], { from: 0, to: 0, insert: "7.00" });
});

test("core executeCommand computes avg paragraph", async () => {
  const result = await executeCommand(snapshot("10\n20\n30", 0), "avg", {
    mode: "editor",
  });

  assert.equal(result.message.includes("avg(paragraph) = 20"), true);
  assert.equal(result.operations.length, 1);
  assert.deepEqual(result.operations[0]?.changes[0], { from: 0, to: 0, insert: "20.00" });
});

test("core executeCommand supports unit-aware avg row", async () => {
  const text = [
    "| item | a | b |",
    "| --- | --- | --- |",
    "| x | 2m | 2km |",
    "| y | 3m | 4m |",
  ].join("\n");
  const result = await executeCommand(snapshot(text, 0), "avg row", {
    mode: "editor",
    evaluateExpression: unitAwareEvaluator,
  });

  assert.equal(result.message.includes("avg(row)"), true);
  assert.equal(result.operations.length, 1);
  assert.deepEqual(result.operations[0]?.changes[0], { from: 0, to: 0, insert: "1001.00 m\n3.50 m" });
});

test("core executeCommand ignores non-numeric header cells for avg column", async () => {
  const text = [
    "| header | number |",
    "| ------ | ------ |",
    "| a      | 3      |",
    "| b      | 4      |",
  ].join("\n");
  const result = await executeCommand(snapshot(text, 0), "avg column", {
    mode: "editor",
    evaluateExpression: numberLeakingEvaluator,
  });

  assert.equal(result.message.includes("avg(column)"), true);
  assert.equal(result.operations.length, 1);
  assert.deepEqual(result.operations[0]?.changes[0], { from: 0, to: 0, insert: "3.50" });
});
