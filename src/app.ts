import {
  getOrCreateNote,
  listNotes,
  createNote,
  deleteNote,
  exportToFile,
} from "./api";
import {
  mountEditor,
  setEditorContent,
  focusEditor,
  flushSave,
} from "./editor/editor";
import { openSwitcher, closeSwitcher, isSwitcherOpen } from "./switcher/switcher";
import { state } from "./state";
import { save } from "@tauri-apps/plugin-dialog";
import { getCurrentWindow } from "@tauri-apps/api/window";

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

function deriveFilename(body: string): string {
  const firstLine = body.split("\n").find((l) => l.trim().length > 0)?.trim();
  if (!firstLine) return "note";
  return firstLine
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-|-$/g, "")
    .slice(0, 40) || "note";
}

async function handleExportClipboard() {
  const note = state.activeNote;
  if (!note) return;
  await flushSave();
  try {
    await navigator.clipboard.writeText(note.body);
    showToast("Copied to clipboard");
  } catch (e) {
    console.error("Clipboard write failed:", e);
  }
}

async function handleExportFileResult(path: string | null) {
  const note = state.activeNote;
  if (!path || !note) return;
  try {
    await exportToFile(path, note.body);
    showToast("Exported to " + path.split("/").pop());
  } catch (e) {
    console.error("File export failed:", e);
  }
}

async function handleExportFile() {
  const note = state.activeNote;
  if (!note) return;
  await flushSave();

  const filename = deriveFilename(note.body);
  const path = await save({
    defaultPath: `${filename}.md`,
    filters: [
      { name: "Markdown", extensions: ["md"] },
      { name: "Plain Text", extensions: ["txt"] },
    ],
  });

  await handleExportFileResult(path);
}

async function handleHideWindow() {
  const win = getCurrentWindow();
  await flushSave();
  await win.hide();
}

function setupKeyboardShortcuts() {
  document.addEventListener("keydown", (e) => {
    // Ctrl+P — toggle switcher
    if (e.ctrlKey && !e.shiftKey && e.key === "p") {
      e.preventDefault();
      if (isSwitcherOpen()) {
        closeSwitcher();
        focusEditor();
      } else {
        openSwitcher(switchToNote);
      }
      return;
    }

    // Ctrl+N — new note
    if (e.ctrlKey && !e.shiftKey && e.key === "n") {
      e.preventDefault();
      handleCreateNote();
      return;
    }

    // Ctrl+Shift+E — export to file
    if (e.ctrlKey && e.shiftKey && e.key === "E") {
      e.preventDefault();
      handleExportFile();
      return;
    }

    // Ctrl+E — export to clipboard
    if (e.ctrlKey && !e.shiftKey && e.key === "e") {
      e.preventDefault();
      handleExportClipboard();
      return;
    }

    // Ctrl+W — hide window
    if (e.ctrlKey && !e.shiftKey && e.key === "w") {
      e.preventDefault();
      handleHideWindow();
      return;
    }

    // Ctrl+Backspace — delete note
    if (e.ctrlKey && e.key === "Backspace" && !isSwitcherOpen()) {
      e.preventDefault();
      handleDeleteNote();
      return;
    }

    // Ctrl+↑ — previous note
    if (e.ctrlKey && e.key === "ArrowUp" && !isSwitcherOpen()) {
      e.preventDefault();
      handlePrevNote();
      return;
    }

    // Ctrl+↓ — next note
    if (e.ctrlKey && e.key === "ArrowDown" && !isSwitcherOpen()) {
      e.preventDefault();
      handleNextNote();
      return;
    }

    // Escape — close switcher
    if (e.key === "Escape" && isSwitcherOpen()) {
      e.preventDefault();
      closeSwitcher();
      focusEditor();
      return;
    }
  });
}

let toastEl: HTMLElement | null = null;
let toastTimer: number | null = null;

function showToast(message: string) {
  if (!toastEl) {
    toastEl = document.createElement("div");
    toastEl.className = "toast";
    document.body.appendChild(toastEl);
  }
  toastEl.textContent = message;
  toastEl.classList.add("visible");
  if (toastTimer !== null) clearTimeout(toastTimer);
  toastTimer = window.setTimeout(() => {
    toastEl!.classList.remove("visible");
  }, 1500);
}

let statusTitleEl: HTMLElement;
let statusMetaEl: HTMLElement;

function createStatusBar(container: HTMLElement) {
  const bar = document.createElement("div");
  bar.className = "status-bar";

  statusTitleEl = document.createElement("span");
  statusTitleEl.className = "status-bar-title";

  statusMetaEl = document.createElement("span");
  statusMetaEl.className = "status-bar-meta";

  const hint = document.createElement("span");
  hint.className = "status-bar-hint";
  hint.textContent = "Ctrl+P search";

  statusMetaEl.appendChild(hint);
  bar.appendChild(statusTitleEl);
  bar.appendChild(statusMetaEl);
  container.appendChild(bar);

  updateStatusBar();
}

function updateStatusBar() {
  const entry = state.notes.find((n) => n.id === state.activeNote?.id);
  statusTitleEl.textContent = entry?.title ?? "Untitled";

  const count = state.notes.length;
  const idx = state.notes.findIndex((n) => n.id === state.activeNote?.id);
  const countEl = statusMetaEl.querySelector(".status-count") as HTMLElement | null;
  const text = `${idx + 1}/${count}`;
  if (countEl) {
    countEl.textContent = text;
  } else {
    const span = document.createElement("span");
    span.className = "status-count";
    span.textContent = text;
    statusMetaEl.prepend(span);
  }
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

  createStatusBar(container);
  mountEditor(editorEl);
  setupKeyboardShortcuts();

  state.on(() => updateStatusBar());
}
