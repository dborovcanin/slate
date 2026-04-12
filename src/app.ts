import {
  getOrCreateNote,
  listNotes,
  createNote,
  deleteNote,
  saveNote as apiSaveNote,
  type Note,
} from "./api";
import {
  mountEditor,
  setEditorContent,
  focusEditor,
  flushSave,
} from "./editor/editor";
import { openSwitcher, closeSwitcher, isSwitcherOpen } from "./switcher/switcher";
import { state } from "./state";

async function switchToNote(id: string) {
  await flushSave();
  const notes = await listNotes();
  const note = notes.find((n) => n.id === id);
  if (!note) return;
  state.setActiveNote(note);
  state.setNotes(notes);
  setEditorContent(note.body);
  focusEditor();
}

async function handleCreateNote() {
  await flushSave();
  const note = await createNote();
  state.addNote(note);
  state.setActiveNote(note);
  setEditorContent("");
  focusEditor();
}

async function handleDeleteNote() {
  const active = state.activeNote;
  if (!active) return;
  const adjacentId = state.getAdjacentNoteId(1) ?? state.getAdjacentNoteId(-1);
  await flushSave();
  await deleteNote(active.id);
  state.removeNote(active.id);

  if (adjacentId) {
    await switchToNote(adjacentId);
  } else {
    const note = await createNote();
    state.addNote(note);
    state.setActiveNote(note);
    setEditorContent("");
    focusEditor();
  }
}

async function handlePrevNote() {
  const id = state.getAdjacentNoteId(-1);
  if (id) await switchToNote(id);
}

async function handleNextNote() {
  const id = state.getAdjacentNoteId(1);
  if (id) await switchToNote(id);
}

function setupKeyboardShortcuts() {
  document.addEventListener("keydown", (e) => {
    if (e.ctrlKey && e.key === "p") {
      e.preventDefault();
      if (isSwitcherOpen()) {
        closeSwitcher();
        focusEditor();
      } else {
        openSwitcher(switchToNote);
      }
      return;
    }

    if (e.ctrlKey && e.key === "n") {
      e.preventDefault();
      handleCreateNote();
      return;
    }

    if (e.ctrlKey && e.key === "Backspace" && !isSwitcherOpen()) {
      e.preventDefault();
      handleDeleteNote();
      return;
    }

    if (e.ctrlKey && e.key === "ArrowUp" && !isSwitcherOpen()) {
      e.preventDefault();
      handlePrevNote();
      return;
    }

    if (e.ctrlKey && e.key === "ArrowDown" && !isSwitcherOpen()) {
      e.preventDefault();
      handleNextNote();
      return;
    }

    if (e.key === "Escape" && isSwitcherOpen()) {
      e.preventDefault();
      closeSwitcher();
      focusEditor();
      return;
    }
  });
}

export async function initApp() {
  const container = document.getElementById("app");
  if (!container) throw new Error("Missing #app element");

  const editorEl = document.createElement("div");
  editorEl.id = "editor";
  editorEl.style.flex = "1";
  editorEl.style.overflow = "hidden";
  container.appendChild(editorEl);

  const note = await getOrCreateNote();
  const notes = await listNotes();
  state.setActiveNote(note);
  state.setNotes(notes);

  mountEditor(editorEl);
  setupKeyboardShortcuts();
}
