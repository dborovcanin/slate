import test, { before } from "node:test";
import assert from "node:assert/strict";
import { executeCommand, listCommandSuggestions } from "./command-engine.ts";
import { ensureWasmReady } from "./wasm.ts";

before(async () => { await ensureWasmReady(); });

test("editor mode exposes only editing commands", async () => {
  const values = listCommandSuggestions("editor", "").map((entry) => entry.value);
  assert.deepEqual(values, [
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
    "format clist",
    "format ulist",
    "format olist",
    "note lock",
    "note unlock",
    "note encrypt",
    "note decrypt",
    "note unprotect",
    "export pdf",
    "export md",
    "export txt",
  ]);
  assert.equal(values.includes("q"), false);
});

test("perf suggestions appear only when perf prefix is typed", async () => {
  const emptyValues = listCommandSuggestions("editor", "").map((entry) => entry.value);
  assert.equal(emptyValues.includes("perf status"), false);

  const perfValues = listCommandSuggestions("editor", "perf").map((entry) => entry.value);
  assert.deepEqual(perfValues, [
    "perf status",
    "perf on",
    "perf off",
    "perf dump",
    "perf where",
    "perf clear",
  ]);
});

test("vim mode exposes vim-specific commands", async () => {
  const values = listCommandSuggestions("vim", "").map((entry) => entry.value);
  assert.equal(values.includes("q"), true);
  assert.equal(values.includes("w"), true);
  assert.equal(values.includes("wq"), true);
});

test("command suggestions filter by query", async () => {
  const values = listCommandSuggestions("editor", "fo").map((entry) => entry.value);
  assert.deepEqual(values, ["fold", "fold-toggle", "format", "format clist", "format olist", "format ulist", "unfold"]);
});

test("vim write surfaces host save errors", async () => {
  const message = await executeCommand({} as any, "w", {
    mode: "vim",
    onWriteCommand: async () => {
      throw new Error("note changed since last load; use :w! to force save");
    },
  });
  assert.equal(message, "write failed: note changed since last load; use :w! to force save");
});

test("export host command routes format/path and clipboard fallback", async () => {
  let called:
    | {
      format: "pdf" | "md" | "txt";
      path: string | null;
    }
    | null = null;

  const mdClipboard = await executeCommand({} as any, "export md", {
    mode: "editor",
    onExportCommand: async (options) => {
      called = options;
      return "ok-md";
    },
  });
  assert.equal(mdClipboard, "ok-md");
  assert.deepEqual(called, { format: "md", path: null });

  const pdfPath = await executeCommand({} as any, "export pdf /tmp/note.pdf", {
    mode: "editor",
    onExportCommand: async (options) => {
      called = options;
      return "ok-pdf";
    },
  });
  assert.equal(pdfPath, "ok-pdf");
  assert.deepEqual(called, { format: "pdf", path: "/tmp/note.pdf" });

  const pdfMissingPath = await executeCommand({} as any, "export pdf", {
    mode: "editor",
    onExportCommand: async () => "should-not-run",
  });
  assert.equal(pdfMissingPath, "usage: export pdf <path>");
});

test("collection host command routes choose/create/delete/update/purge/join/leave/clear", async () => {
  const seen: Array<{ action: string; collection: string | null }> = [];
  const choose = await executeCommand({} as any, "collection choose Work", {
    mode: "editor",
    onCollectionCommand: async (options) => {
      seen.push(options);
      return "working collection: Work";
    },
  });
  assert.equal(choose, "working collection: Work");
  assert.deepEqual(seen[0], { action: "choose", collection: "Work" });

  const create = await executeCommand({} as any, "collection create Work", {
    mode: "editor",
    onCollectionCommand: async (options) => {
      seen.push(options);
      return "collection created";
    },
  });
  assert.equal(create, "collection created");
  assert.deepEqual(seen[1], { action: "create", collection: "Work" });

  const update = await executeCommand({} as any, "collection update Work", {
    mode: "editor",
    onCollectionCommand: async (options) => {
      seen.push(options);
      return "collection updated";
    },
  });
  assert.equal(update, "collection updated");
  assert.deepEqual(seen[2], { action: "update", collection: "Work" });

  const join = await executeCommand({} as any, "collection join Work", {
    mode: "editor",
    onCollectionCommand: async (options) => {
      seen.push(options);
      return "joined";
    },
  });
  assert.equal(join, "joined");
  assert.deepEqual(seen[3], { action: "add", collection: "Work" });

  const leave = await executeCommand({} as any, "collection leave Work", {
    mode: "editor",
    onCollectionCommand: async (options) => {
      seen.push(options);
      return "left";
    },
  });
  assert.equal(leave, "left");
  assert.deepEqual(seen[4], { action: "remove", collection: "Work" });

  const purge = await executeCommand({} as any, "collection purge Work", {
    mode: "editor",
    onCollectionCommand: async (options) => {
      seen.push(options);
      return "purged";
    },
  });
  assert.equal(purge, "purged");
  assert.deepEqual(seen[5], { action: "purge", collection: "Work" });

  const del = await executeCommand({} as any, "collection delete Work", {
    mode: "editor",
    onCollectionCommand: async (options) => {
      seen.push(options);
      return "deleted";
    },
  });
  assert.equal(del, "deleted");
  assert.deepEqual(seen[6], { action: "delete", collection: "Work" });

  const clear = await executeCommand({} as any, "collection choose none", {
    mode: "editor",
    onCollectionCommand: async (options) => {
      seen.push(options);
      return "working collection cleared";
    },
  });
  assert.equal(clear, "working collection cleared");
  assert.deepEqual(seen[7], { action: "clear", collection: null });
});
