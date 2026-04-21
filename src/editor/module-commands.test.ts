import test from "node:test";
import assert from "node:assert/strict";
import {
  listNoteModuleCommandSuggestions,
  tryExecuteNoteModuleCommand,
} from "./module-commands.ts";
import type { NoteModules } from "../api.ts";

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
