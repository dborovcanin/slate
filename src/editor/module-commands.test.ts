import test from "node:test";
import assert from "node:assert/strict";
import {
  listNoteModuleCommandSuggestions,
  tryExecuteNoteModuleCommand,
} from "./module-commands.ts";
import {
  DEFAULT_RUNTIME_FLAGS,
  DEFAULT_THEME_CONFIG,
  type Note,
  type NoteModules,
} from "../api.ts";
import { effectiveModules, modulesForNote } from "./module-gating.ts";

function modules(): NoteModules {
  return { math: true, table: true, variables: true, style: true };
}

test("module command reports status and updates selected module", async () => {
  let current = modules();
  const getNoteModules = () => current;
  const setNoteModules = async (next: NoteModules) => {
    current = next;
  };

  const status = await tryExecuteNoteModuleCommand(":module status", {
    getNoteModules,
    setNoteModules,
  });
  assert.equal(status, "modules math=on table=on variables=on style=on");

  const off = await tryExecuteNoteModuleCommand(":module off math", {
    getNoteModules,
    setNoteModules,
  });
  assert.equal(off, "modules math=off table=on variables=on style=on");
  assert.equal(current.math, false);

  const toggle = await tryExecuteNoteModuleCommand(":module toggle style", {
    getNoteModules,
    setNoteModules,
  });
  assert.equal(toggle, "modules math=off table=on variables=on style=off");
  assert.equal(current.style, false);
});

test("module suggestions are shown for empty input and module prefix", () => {
  const empty = listNoteModuleCommandSuggestions("");
  assert.ok(empty.length >= 5);
  assert.ok(empty.some((entry) => entry.value === "module status"));

  const filtered = listNoteModuleCommandSuggestions("module off");
  assert.ok(filtered.length > 0);
  assert.ok(filtered.every((entry) => entry.value.startsWith("module off")));
});

function noteWithModules(modules: NoteModules): Note {
  return {
    id: "n1",
    body: "",
    modules,
    created_at: "",
    updated_at: "",
  };
}

test("effectiveModules gates variables by per-note module even when global legacy flag is true", () => {
  const config = {
    ...DEFAULT_THEME_CONFIG,
    variables_enabled: true,
  };
  const note = noteWithModules({ math: true, table: true, variables: false, style: true });
  const resolved = modulesForNote(note, config);
  const loaded = effectiveModules(resolved, DEFAULT_RUNTIME_FLAGS);
  assert.equal(loaded.variables, false);
});

test("effectiveModules keeps variables enabled when note module is on even if legacy flag is false", () => {
  const config = {
    ...DEFAULT_THEME_CONFIG,
    variables_enabled: false,
  };
  const note = noteWithModules({ math: true, table: true, variables: true, style: true });
  const resolved = modulesForNote(note, config);
  const loaded = effectiveModules(resolved, DEFAULT_RUNTIME_FLAGS);
  assert.equal(loaded.variables, true);
});
