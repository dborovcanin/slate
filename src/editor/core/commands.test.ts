import test, { before } from "node:test";
import assert from "node:assert/strict";
import { executeCommand, listCommandSuggestions } from "./commands.ts";
import { ensureWasmReady } from "../wasm.ts";

before(async () => { await ensureWasmReady(); });

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

test("core command suggestions are mode-aware", async () => {
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
    "notify",
    "notify-delete",
    "module status",
    "module math on",
    "module math off",
    "module math toggle",
    "module table on",
    "module table off",
    "module table toggle",
    "module variables on",
    "module variables off",
    "module variables toggle",
    "module style on",
    "module style off",
    "module style toggle",
    "format",
    "clip-watch",
    "clip-watch-stop",
    "fold",
    "unfold",
    "fold-toggle",
    "clist",
    "ulist",
    "olist",
    "note lock",
    "note unlock",
    "note encrypt",
    "note decrypt",
    "note unprotect",
  ]);

  const vimValues = listCommandSuggestions("vim", "").map((entry) => entry.value);
  assert.equal(vimValues.includes("q"), true);
});

test("core executeCommand handles module status/on/off/toggle", async () => {
  let current = { math: true, table: true, variables: true, style: true };
  const getNoteModules = () => current;
  const setNoteModules = async (next: typeof current) => {
    current = next;
  };

  const status = await executeCommand(snapshot("", 0), "modules", {
    mode: "editor",
    getNoteModules,
    setNoteModules,
  });
  assert.equal(status.message, "modules math=on table=on variables=on style=on");
  assert.deepEqual(status.operations, []);

  const off = await executeCommand(snapshot("", 0), "module math off", {
    mode: "editor",
    getNoteModules,
    setNoteModules,
  });
  assert.equal(off.message, "modules math=off table=on variables=on style=on");
  assert.equal(current.math, false);

  const toggle = await executeCommand(snapshot("", 0), "modules style toggle", {
    mode: "editor",
    getNoteModules,
    setNoteModules,
  });
  assert.equal(toggle.message, "modules math=off table=on variables=on style=off");
  assert.equal(current.style, false);
});

test("core executeCommand module on/off is idempotent and requires persistence runtime", async () => {
  let current = { math: true, table: true, variables: true, style: true };
  const getNoteModules = () => current;
  let writes = 0;
  const setNoteModules = async (next: typeof current) => {
    writes += 1;
    current = next;
  };

  const alreadyOn = await executeCommand(snapshot("", 0), "module math on", {
    mode: "editor",
    getNoteModules,
    setNoteModules,
  });
  assert.equal(alreadyOn.message, "module math already on");
  assert.equal(writes, 0);

  const off = await executeCommand(snapshot("", 0), "module math off", {
    mode: "editor",
    getNoteModules,
    setNoteModules,
  });
  assert.equal(off.message, "modules math=off table=on variables=on style=on");
  assert.equal(writes, 1);

  const alreadyOff = await executeCommand(snapshot("", 0), "module math off", {
    mode: "editor",
    getNoteModules,
    setNoteModules,
  });
  assert.equal(alreadyOff.message, "module math already off");
  assert.equal(writes, 1);

  const unavailable = await executeCommand(snapshot("", 0), "module style off", {
    mode: "editor",
    getNoteModules,
  });
  assert.equal(unavailable.message, "module unavailable");
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

test("core executeCommand handles notify with one-line reminder payload", async () => {
  let payload:
    | {
      noteId: string;
      lineNumber: number;
      remindAtMs: number;
      displayAt: string;
      lineText: string;
    }
    | null = null;

  const result = await executeCommand(snapshot("alpha\nbeta", 6), "notify", {
    mode: "editor",
    activeNoteId: "note-1",
    pickDateTime: async () => ({
      insertText: "2026-04-15 14:34",
      remindAtMs: 1_776_000_000_000,
      displayAt: "15.04.2026. 14:34",
      hasTime: true,
    }),
    upsertReminder: async (reminder) => {
      payload = reminder;
    },
  });

  assert.equal(result.operations.length, 0);
  assert.equal(result.message, "notify set ⏰ 15.04.2026. 14:34");
  assert.deepEqual(payload, {
    noteId: "note-1",
    lineNumber: 2,
    remindAtMs: 1_776_000_000_000,
    displayAt: "15.04.2026. 14:34",
    lineText: "beta",
  });
});

test("core executeCommand surfaces notify persistence errors from non-Error rejects", async () => {
  const result = await executeCommand(snapshot("alpha", 0), "notify", {
    mode: "editor",
    activeNoteId: "note-1",
    pickDateTime: async () => ({
      insertText: "2026-04-15 14:34",
      remindAtMs: 1_776_000_000_000,
      displayAt: "15.04.2026. 14:34",
      hasTime: true,
    }),
    upsertReminder: async () => {
      throw { message: "table reminders has no column named line_text" };
    },
  });

  assert.equal(
    result.message,
    "notify failed: table reminders has no column named line_text",
  );
  assert.equal(result.operations.length, 0);
});

test("core executeCommand handles notify-delete for current line", async () => {
  let payload:
    | {
      noteId: string;
      lineNumber: number;
    }
    | null = null;

  const result = await executeCommand(snapshot("alpha\nbeta", 6), "notify-delete", {
    mode: "editor",
    activeNoteId: "note-1",
    deleteReminder: async (reminder) => {
      payload = reminder;
      return true;
    },
  });

  assert.equal(result.operations.length, 0);
  assert.equal(result.message, "notify deleted on line 2");
  assert.deepEqual(payload, {
    noteId: "note-1",
    lineNumber: 2,
  });
});

test("core executeCommand handles notify-delete miss", async () => {
  const result = await executeCommand(snapshot("alpha", 0), "notify-delete", {
    mode: "editor",
    activeNoteId: "note-1",
    deleteReminder: async () => false,
  });

  assert.equal(result.operations.length, 0);
  assert.equal(result.message, "notify-delete: no reminder on line 1");
});

test("core executeCommand handles clip-watch start/stop", async () => {
  const starts: string[] = [];
  const stops: string[] = [];

  const started = await executeCommand(snapshot("alpha", 0), "clip-watch", {
    mode: "editor",
    startClipboardWatch: async () => {
      starts.push("start");
      return true;
    },
  });
  assert.equal(started.message, "clip-watch started");
  assert.equal(started.operations.length, 0);
  assert.equal(starts.length, 1);

  const alreadyActive = await executeCommand(snapshot("alpha", 0), "clip-watch", {
    mode: "editor",
    startClipboardWatch: async () => false,
  });
  assert.equal(alreadyActive.message, "clip-watch already active");

  const stopped = await executeCommand(snapshot("alpha", 0), "clip-watch-stop", {
    mode: "editor",
    stopClipboardWatch: async () => {
      stops.push("stop");
      return true;
    },
  });
  assert.equal(stopped.message, "clip-watch stopped");
  assert.equal(stops.length, 1);

  const notActive = await executeCommand(snapshot("alpha", 0), "clip-watch-stop", {
    mode: "editor",
    stopClipboardWatch: async () => false,
  });
  assert.equal(notActive.message, "clip-watch not active");
});

test("core executeCommand handles fold/unfold/toggle via runtime", async () => {
  const actions: string[] = [];
  const runFoldCommand = (action: "fold" | "unfold" | "fold-toggle") => {
    actions.push(action);
    return { changed: true, message: `fold: ${action}` };
  };

  const folded = await executeCommand(snapshot("# h\nx", 0), "fold", {
    mode: "editor",
    runFoldCommand,
  });
  assert.equal(folded.message, "fold: fold");
  assert.equal(folded.operations.length, 0);

  const toggled = await executeCommand(snapshot("# h\nx", 0), "za", {
    mode: "editor",
    runFoldCommand,
  });
  assert.equal(toggled.message, "fold: fold-toggle");
  assert.equal(toggled.operations.length, 0);

  const unfolded = await executeCommand(snapshot("# h\nx", 0), "unfold", {
    mode: "editor",
    runFoldCommand,
  });
  assert.equal(unfolded.message, "fold: unfold");
  assert.equal(unfolded.operations.length, 0);

  assert.deepEqual(actions, ["fold", "fold-toggle", "unfold"]);
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

test("core executeCommand clist converts selected lines to checkboxes", async () => {
  const text = ["alpha", "- beta", "1. gamma", "tail"].join("\n");
  const tailStart = text.indexOf("\ntail");
  const result = await executeCommand(snapshot(text, tailStart, 0), "clist", {
    mode: "editor",
  });

  assert.equal(result.message, "converted 3 lines to checklist");
  assert.equal(result.operations.length, 1);
  assert.deepEqual(result.operations[0]?.changes[0], {
    from: 0,
    to: tailStart,
    insert: "- [ ] alpha\n- [ ] beta\n1. [ ] gamma",
  });
});

test("core executeCommand checklist alias still resolves", async () => {
  const text = ["alpha", "tail"].join("\n");
  const tailStart = text.indexOf("\ntail");
  const result = await executeCommand(snapshot(text, tailStart, 0), "checklist", {
    mode: "editor",
  });

  assert.equal(result.message, "converted 1 line to checklist");
  assert.equal(result.operations.length, 1);
  assert.deepEqual(result.operations[0]?.changes[0], {
    from: 0,
    to: tailStart,
    insert: "- [ ] alpha",
  });
});

test("core executeCommand ulist converts only current line without selection", async () => {
  const text = ["alpha", "1. beta", "gamma"].join("\n");
  const betaStart = text.indexOf("1. beta");
  const result = await executeCommand(snapshot(text, betaStart + 2), "ulist", {
    mode: "editor",
  });

  assert.equal(result.message, "converted 1 line to unordered list");
  assert.equal(result.operations.length, 1);
  assert.deepEqual(result.operations[0]?.changes[0], {
    from: betaStart,
    to: betaStart + "1. beta".length,
    insert: "- beta",
  });
});

test("core executeCommand olist converts selected lines into ordered items", async () => {
  const text = ["alpha", "- [x] beta", "- gamma", "tail"].join("\n");
  const tailStart = text.indexOf("\ntail");
  const result = await executeCommand(snapshot(text, tailStart, 0), "olist", {
    mode: "editor",
  });

  assert.equal(result.message, "converted 3 lines to ordered list");
  assert.equal(result.operations.length, 1);
  assert.deepEqual(result.operations[0]?.changes[0], {
    from: 0,
    to: tailStart,
    insert: "1. alpha\n2. beta\n3. gamma",
  });
});

test("core executeCommand olist advances numbering across already-numbered lines", async () => {
  const text = ["1. alpha", "3. beta", "tail"].join("\n");
  const tailStart = text.indexOf("\ntail");
  const result = await executeCommand(snapshot(text, tailStart, 0), "olist", {
    mode: "editor",
  });

  assert.equal(result.message, "converted 1 line to ordered list");
  assert.equal(result.operations.length, 1);
  assert.deepEqual(result.operations[0]?.changes[0], {
    from: 0,
    to: tailStart,
    insert: "1. alpha\n2. beta",
  });
});

test("core executeCommand clist in vim mode treats endpoint lines as selected", async () => {
  const text = ["alpha", "beta", "gamma"].join("\n");
  const betaStart = text.indexOf("beta");
  const result = await executeCommand(snapshot(text, betaStart, 0), "clist", {
    mode: "vim",
  });

  assert.equal(result.message, "converted 2 lines to checklist");
  assert.equal(result.operations.length, 1);
  assert.deepEqual(result.operations[0]?.changes[0], {
    from: 0,
    to: betaStart + "beta".length,
    insert: "- [ ] alpha\n- [ ] beta",
  });
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
