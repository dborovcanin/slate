import test from "node:test";
import assert from "node:assert/strict";
import { AppState, deriveTitle } from "./state.ts";
import type { Note } from "./api.ts";

function note(id: string, body: string, updatedAt: string): Note {
  return {
    id,
    body,
    modules: {
      math: true,
      table: true,
      variables: true,
      style: true,
    },
    access_mode: "none",
    is_unlocked: true,
    created_at: "2026-01-01T00:00:00Z",
    updated_at: updatedAt,
  };
}

test("deriveTitle uses first non-empty line", () => {
  assert.equal(deriveTitle("\n\n hello world \nnext"), "hello world");
  assert.equal(deriveTitle("   \n"), "Untitled");
});

test("updateBody updates active note title and moves note to top", () => {
  const appState = new AppState();
  const a = note("a", "older", "2026-01-01T00:00:00Z");
  const b = note("b", "active", "2026-01-01T00:01:00Z");
  appState.setNotes([a, b]);
  appState.setActiveNote(b);

  const events: string[] = [];
  appState.on((event) => events.push(event));
  appState.updateBody("fresh title\nline 2");

  assert.equal(appState.activeNote?.body, "fresh title\nline 2");
  assert.equal(appState.notes[0].id, "b");
  assert.equal(appState.notes[0].title, "fresh title");
  assert.ok(events.includes("note-changed"));
});

test("updateBody can preserve backend updated_at to avoid drift", () => {
  const appState = new AppState();
  const a = note("a", "first", "2026-01-01T00:00:00Z");
  appState.setNotes([a]);
  appState.setActiveNote(a);

  const backendUpdatedAt = "2026-01-01T00:42:00Z";
  appState.updateBody("first\nsecond", backendUpdatedAt);

  assert.equal(appState.activeNote?.updated_at, backendUpdatedAt);
  assert.equal(appState.notes[0]?.updatedAt, backendUpdatedAt);
});

test("updateDraftTitle updates note list title without mutating body", () => {
  const appState = new AppState();
  const a = note("a", "old body", "2026-01-01T00:00:00Z");
  appState.setNotes([a]);
  appState.setActiveNote(a);

  appState.updateDraftTitle("draft title");

  assert.equal(appState.notes[0]?.title, "draft title");
  assert.equal(appState.activeNote?.body, "old body");
});

test("getAdjacentNoteId returns null at boundaries", () => {
  const appState = new AppState();
  const a = note("a", "first", "2026-01-01T00:00:00Z");
  const b = note("b", "second", "2026-01-01T00:01:00Z");
  appState.setNotes([a, b]);
  appState.setActiveNote(a);

  assert.equal(appState.getAdjacentNoteId(-1), null);
  assert.equal(appState.getAdjacentNoteId(1), "b");
});
