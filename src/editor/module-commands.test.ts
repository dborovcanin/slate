import test, { before } from "node:test";
import assert from "node:assert/strict";
import { executeCommand, listCommandSuggestions } from "./core/commands.ts";
import {
  DEFAULT_RUNTIME_FLAGS,
  DEFAULT_THEME_CONFIG,
  type Note,
  type NoteModules,
} from "../api.ts";
import { effectiveModules, modulesForNote } from "./module-gating.ts";
import { ensureWasmReady } from "./wasm.ts";

before(async () => {
  await ensureWasmReady();
});

function snapshot(text: string, head = 0, anchor = head) {
  return { text, selection: { anchor, head } };
}

function modules(): NoteModules {
  return { math: true, table: true, variables: true, style: true, cross_note: true };
}

test("module command reports status and updates selected module", async () => {
  let current = modules();
  const getNoteModules = () => current;
  const setNoteModules = async (next: NoteModules) => {
    current = next;
  };

  const status = await executeCommand(snapshot(""), ":module status", {
    mode: "editor",
    getNoteModules,
    setNoteModules,
  });
  assert.equal(
    status.message,
    "modules math=on table=on variables=on style=on cross_note=on",
  );

  const off = await executeCommand(snapshot(""), ":module math off", {
    mode: "editor",
    getNoteModules,
    setNoteModules,
  });
  assert.equal(
    off.message,
    "modules math=off table=on variables=on style=on cross_note=on",
  );
  assert.equal(current.math, false);

  const toggle = await executeCommand(snapshot(""), ":module style toggle", {
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

test("module suggestions are shown for empty input and module prefix", () => {
  const empty = listCommandSuggestions("editor", "");
  assert.ok(empty.length >= 5);
  assert.ok(empty.some((entry) => entry.value === "module status"));

  const filtered = listCommandSuggestions("editor", "module math");
  assert.ok(filtered.length > 0);
  assert.ok(filtered.every((entry) => entry.value.startsWith("module math")));
});

function noteWithModules(modules: NoteModules): Note {
  return {
    id: "n1",
    body: "",
    modules,
    access_mode: "none",
    is_unlocked: true,
    created_at: "",
    updated_at: "",
  };
}

test("effectiveModules gates variables by per-note module", () => {
  const config = { ...DEFAULT_THEME_CONFIG };
  const note = noteWithModules({
    math: true,
    table: true,
    variables: false,
    style: true,
    cross_note: true,
  });
  const resolved = modulesForNote(note, config);
  const loaded = effectiveModules(resolved, DEFAULT_RUNTIME_FLAGS);
  assert.equal(loaded.variables, false);
});

test("effectiveModules keeps variables enabled when note module is on", () => {
  const config = { ...DEFAULT_THEME_CONFIG };
  const note = noteWithModules({
    math: true,
    table: true,
    variables: true,
    style: true,
    cross_note: true,
  });
  const resolved = modulesForNote(note, config);
  const loaded = effectiveModules(resolved, DEFAULT_RUNTIME_FLAGS);
  assert.equal(loaded.variables, true);
});
