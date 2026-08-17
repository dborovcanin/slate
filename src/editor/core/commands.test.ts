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
    "remind",
    "remind toggle",
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
    "module cross_note on",
    "module cross_note off",
    "module cross_note toggle",
    "collection choose",
    "collection clear",
    "collection create",
    "collection delete",
    "collection update",
    "collection purge",
    "collection join",
    "collection leave",
    "format",
    "clip-watch on",
    "clip-watch off",
    "fold",
    "unfold",
    "fold-toggle",
    "format clear",
    "paragraph title",
    "paragraph clist",
    "paragraph olist",
    "paragraph ulist",
    "format bold",
    "format code",
    "format italic",
    "format strike",
    "note lock",
    "note unlock",
    "note encrypt",
    "note decrypt",
    "note unprotect",
    "export pdf",
    "export md",
    "export txt",
    "backup export",
    "backup load",
  ]);

  const vimValues = listCommandSuggestions("vim", "").map((entry) => entry.value);
  assert.equal(vimValues.includes("q"), true);
});

test("core executeCommand handles module status/on/off/toggle", async () => {
  let current = { math: true, table: true, variables: true, style: true, cross_note: true };
  const getNoteModules = () => current;
  const setNoteModules = async (next: typeof current) => {
    current = next;
  };

  const status = await executeCommand(snapshot("", 0), "modules", {
    mode: "editor",
    getNoteModules,
    setNoteModules,
  });
  assert.equal(
    status.message,
    "modules math=on table=on variables=on style=on cross_note=on",
  );
  assert.deepEqual(status.operations, []);

  const off = await executeCommand(snapshot("", 0), "module math off", {
    mode: "editor",
    getNoteModules,
    setNoteModules,
  });
  assert.equal(
    off.message,
    "modules math=off table=on variables=on style=on cross_note=on",
  );
  assert.equal(current.math, false);

  const toggle = await executeCommand(snapshot("", 0), "modules style toggle", {
    mode: "editor",
    getNoteModules,
    setNoteModules,
  });
  assert.equal(
    toggle.message,
    "modules math=off table=on variables=on style=off cross_note=on",
  );
  assert.equal(current.style, false);
});

test("core executeCommand module on/off is idempotent and requires persistence runtime", async () => {
  let current = { math: true, table: true, variables: true, style: true, cross_note: true };
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
  assert.equal(
    off.message,
    "modules math=off table=on variables=on style=on cross_note=on",
  );
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

test("core executeCommand delegates sum and avg to the host capability", async () => {
  const calls: Array<{ text: string; rawInput: string; mode: string }> = [];
  const executeMathCommand = async (
    mathSnapshot: { text: string },
    rawInput: string,
    mode: string,
  ) => {
    calls.push({ text: mathSnapshot.text, rawInput, mode });
    return {
      message: "sum(paragraph) = 30.00",
      operations: [{ changes: [{ from: 0, to: 0, insert: "30.00" }], selection: { anchor: 5 } }],
      clipboardText: "30.00",
      quitRequested: false,
    };
  };

  let copied: string | null = null;
  const result = await executeCommand(snapshot("10\n20", 0), "sum", {
    mode: "editor",
    executeMathCommand: executeMathCommand as never,
    copyText: (text: string) => {
      copied = text;
    },
  });

  assert.deepEqual(calls, [{ text: "10\n20", rawInput: "sum", mode: "editor" }]);
  assert.equal(result.message, "sum(paragraph) = 30.00");
  assert.deepEqual(result.operations[0]?.changes[0], { from: 0, to: 0, insert: "30.00" });
  assert.equal(copied, "30.00");

  await executeCommand(snapshot("1\n2", 0), "avg column", {
    mode: "editor",
    executeMathCommand: executeMathCommand as never,
  });
  assert.equal(calls.length, 2);
  assert.equal(calls[1]?.rawInput, "avg column");
});

test("core executeCommand reports sum and avg unavailable without the host capability", async () => {
  const sumResult = await executeCommand(snapshot("10\n20", 0), "sum", { mode: "editor" });
  assert.equal(sumResult.message, "sum unavailable");
  assert.equal(sumResult.operations.length, 0);

  const avgResult = await executeCommand(snapshot("10\n20", 0), "avg", { mode: "editor" });
  assert.equal(avgResult.message, "avg unavailable");
  assert.equal(avgResult.operations.length, 0);
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

test("core executeCommand handles vim write and write+quit", async () => {
  let writeCalls = 0;
  let quitCalls = 0;

  const editorW = await executeCommand(snapshot("", 0), "w", { mode: "editor" });
  assert.equal(editorW.message, "unknown command: w");

  const vimW = await executeCommand(snapshot("alpha", 0), "w", {
    mode: "vim",
    onWrite: async () => {
      writeCalls += 1;
    },
  });
  assert.equal(vimW.message, "written");
  assert.equal(writeCalls, 1);
  assert.equal(quitCalls, 0);

  const vimWq = await executeCommand(snapshot("alpha", 0), "wq", {
    mode: "vim",
    onWrite: async () => {
      writeCalls += 1;
    },
    onQuit: async () => {
      quitCalls += 1;
    },
  });
  assert.equal(vimWq.message, "written and quit");
  assert.equal(writeCalls, 2);
  assert.equal(quitCalls, 1);
});

test("core executeCommand handles remind with one-line reminder payload", async () => {
  let payload:
    | {
      noteId: string;
      lineNumber: number;
      remindAtMs: number;
      displayAt: string;
      lineText: string;
    }
    | null = null;

  const result = await executeCommand(snapshot("alpha\nbeta", 6), "remind", {
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
  assert.equal(result.message, "remind set ⏰ 15.04.2026. 14:34");
  assert.deepEqual(payload, {
    noteId: "note-1",
    lineNumber: 2,
    remindAtMs: 1_776_000_000_000,
    displayAt: "15.04.2026. 14:34",
    lineText: "beta",
  });
});

test("core executeCommand surfaces remind persistence errors from non-Error rejects", async () => {
  const result = await executeCommand(snapshot("alpha", 0), "remind", {
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
    "remind failed: table reminders has no column named line_text",
  );
  assert.equal(result.operations.length, 0);
});

test("core executeCommand handles remind toggle for current line", async () => {
  let payload:
    | {
      noteId: string;
      lineNumber: number;
    }
    | null = null;

  const result = await executeCommand(snapshot("alpha\nbeta", 6), "remind toggle", {
    mode: "editor",
    activeNoteId: "note-1",
    deleteReminder: async (reminder) => {
      payload = reminder;
      return true;
    },
  });

  assert.equal(result.operations.length, 0);
  assert.equal(result.message, "remind removed on line 2");
  assert.deepEqual(payload, {
    noteId: "note-1",
    lineNumber: 2,
  });
});

test("core executeCommand handles remind toggle miss by setting reminder", async () => {
  const payloads: Array<{
    noteId: string;
    lineNumber: number;
    remindAtMs: number;
    displayAt: string;
    lineText: string;
  }> = [];
  const result = await executeCommand(snapshot("alpha", 0), "remind toggle", {
    mode: "editor",
    activeNoteId: "note-1",
    deleteReminder: async () => false,
    pickDateTime: async () => ({
      insertText: "2026-04-15 14:34",
      remindAtMs: 1_776_000_000_000,
      displayAt: "15.04.2026. 14:34",
      hasTime: true,
    }),
    upsertReminder: async (reminder) => {
      payloads.push(reminder);
    },
  });

  assert.equal(result.operations.length, 0);
  assert.equal(result.message, "remind set ⏰ 15.04.2026. 14:34");
  assert.equal(payloads.length, 1);
});

test("core executeCommand handles clip-watch start/stop", async () => {
  const starts: string[] = [];
  const stops: string[] = [];

  const started = await executeCommand(snapshot("alpha", 0), "clip-watch on", {
    mode: "editor",
    startClipboardWatch: async () => {
      starts.push("start");
      return true;
    },
  });
  assert.equal(started.message, "clip-watch started");
  assert.equal(started.operations.length, 0);
  assert.equal(starts.length, 1);

  const alreadyActive = await executeCommand(snapshot("alpha", 0), "clip-watch on", {
    mode: "editor",
    startClipboardWatch: async () => false,
  });
  assert.equal(alreadyActive.message, "clip-watch already active");

  const stopped = await executeCommand(snapshot("alpha", 0), "clip-watch off", {
    mode: "editor",
    stopClipboardWatch: async () => {
      stops.push("stop");
      return true;
    },
  });
  assert.equal(stopped.message, "clip-watch stopped");
  assert.equal(stops.length, 1);

  const notActive = await executeCommand(snapshot("alpha", 0), "clip-watch off", {
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

test("core executeCommand title converts selected lines to headings", async () => {
  const text = ["- task", "## follow-up", "tail"].join("\n");
  const tailStart = text.indexOf("\ntail");
  const result = await executeCommand(snapshot(text, tailStart, 0), "title", {
    mode: "editor",
  });

  assert.equal(result.message, "converted 2 lines to title");
  assert.equal(result.operations.length, 1);
  assert.deepEqual(result.operations[0]?.changes[0], {
    from: 0,
    to: tailStart,
    insert: "# task\n# follow-up",
  });
});

