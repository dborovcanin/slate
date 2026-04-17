import {
  getOrCreateNote,
  listNotes,
  createNote,
  deleteNote,
  exportToFile,
  getThemeConfigOrDefault,
  type ThemeConfig,
} from "./api";
import {
  mountEditor,
  setEditorContent,
  focusEditor,
  flushSave,
  insertTextAtCursor,
  performFormatAndSave,
} from "./editor/editor";
import {
  openSwitcher,
  closeSwitcher,
  isSwitcherOpen,
  refreshSwitcher,
} from "./switcher/switcher";
import { state } from "./state";
import { confirm, save } from "@tauri-apps/plugin-dialog";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { changeFontSize, cycleFont, getFontLabel } from "./theme/theme";
import { openDatePicker } from "./editor/date-picker";
import { startupMark } from "./perf/startup.ts";

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
  await handleDeleteNoteById(active.id);
}

async function handleDeleteNoteById(noteId: string) {
  const noteEntry = state.notes.find((n) => n.id === noteId);
  if (!noteEntry) return;

  if (!(await confirm(`Delete "${noteEntry.title}"? This cannot be undone.`))) {
    return;
  }

  const deletingActive = state.activeNote?.id === noteId;
  const adjacentId =
    deletingActive ? state.getAdjacentNoteId(1) ?? state.getAdjacentNoteId(-1) : null;

  if (deletingActive) {
    await flushSave();
  }

  await deleteNote(noteId);
  state.removeNote(noteId);

  if (!deletingActive) {
    return;
  }

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
    showToast("Clipboard export failed");
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
    showToast("File export failed");
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

async function handleExitWindow() {
  const win = getCurrentWindow();
  await flushSave();
  await win.close();
}

async function handleInsertDate() {
  const cfg = await getThemeConfigOrDefault();
  const value = await openDatePicker(cfg.date_format);
  if (!value) return;
  insertTextAtCursor(value);
  focusEditor();
}

function runAction(action: () => Promise<void> | void) {
  void Promise.resolve()
    .then(action)
    .catch((e) => {
      console.error("Action failed:", e);
      showToast("Action failed");
    });
}

function isPlusKey(e: KeyboardEvent): boolean {
  return e.key === "+" || e.key === "=" || e.code === "NumpadAdd";
}

function isMinusKey(e: KeyboardEvent): boolean {
  return e.key === "-" || e.key === "_" || e.code === "NumpadSubtract";
}

function setupKeyboardShortcuts() {
  document.addEventListener("keydown", (e) => {
    const key = e.key.toLowerCase();

    // Ctrl+Alt++ / Ctrl+Alt+- — cycle font family
    if (e.ctrlKey && e.altKey && isPlusKey(e)) {
      e.preventDefault();
      const selection = cycleFont(1);
      showToast(`Font: ${getFontLabel(selection.font)} (${selection.fontSize}px)`);
      return;
    }

    if (e.ctrlKey && e.altKey && isMinusKey(e)) {
      e.preventDefault();
      const selection = cycleFont(-1);
      showToast(`Font: ${getFontLabel(selection.font)} (${selection.fontSize}px)`);
      return;
    }

    // Ctrl++ / Ctrl+- — increase/decrease editor font size
    if (e.ctrlKey && !e.altKey && isPlusKey(e)) {
      e.preventDefault();
      const selection = changeFontSize(1);
      showToast(`Font size: ${selection.fontSize}px`);
      return;
    }

    if (e.ctrlKey && !e.altKey && isMinusKey(e)) {
      e.preventDefault();
      const selection = changeFontSize(-1);
      showToast(`Font size: ${selection.fontSize}px`);
      return;
    }

    // Ctrl+P — toggle switcher
    if (e.ctrlKey && !e.shiftKey && key === "p") {
      e.preventDefault();
      if (isSwitcherOpen()) {
        closeSwitcher();
        focusEditor();
      } else {
        openSwitcher(switchToNote, (noteId) => {
          void handleDeleteNoteById(noteId).catch((e) => {
            console.error("Action failed:", e);
            showToast("Action failed");
          });
        });
      }
      return;
    }

    // Ctrl+N — new note
    if (e.ctrlKey && !e.shiftKey && key === "n") {
      e.preventDefault();
      runAction(handleCreateNote);
      return;
    }

    // Ctrl+Shift+E — export to file
    if (e.ctrlKey && e.shiftKey && key === "e") {
      e.preventDefault();
      runAction(handleExportFile);
      return;
    }

    // Ctrl+Shift+D — date picker insert
    if (e.ctrlKey && e.shiftKey && key === "d") {
      e.preventDefault();
      runAction(handleInsertDate);
      return;
    }

    // Ctrl+E — export to clipboard
    if (e.ctrlKey && !e.shiftKey && key === "e") {
      e.preventDefault();
      runAction(handleExportClipboard);
      return;
    }

    // Ctrl+W — hide window
    if (e.ctrlKey && !e.shiftKey && key === "w") {
      e.preventDefault();
      runAction(handleHideWindow);
      return;
    }

    // Ctrl+Shift+Backspace — delete note
    if (e.ctrlKey && e.shiftKey && e.key === "Backspace" && !isSwitcherOpen()) {
      e.preventDefault();
      runAction(handleDeleteNote);
      return;
    }

    // Ctrl+↑ — previous note
    if (e.ctrlKey && e.key === "ArrowUp" && !isSwitcherOpen()) {
      e.preventDefault();
      runAction(handlePrevNote);
      return;
    }

    // Ctrl+↓ — next note
    if (e.ctrlKey && e.key === "ArrowDown" && !isSwitcherOpen()) {
      e.preventDefault();
      runAction(handleNextNote);
      return;
    }

    // Escape — close switcher
    if (e.key === "Escape" && isSwitcherOpen()) {
      e.preventDefault();
      closeSwitcher();
      focusEditor();
      return;
    }

    // Ctrl+S - format & save
    if (e.ctrlKey && !e.shiftKey && key === "s") {
      e.preventDefault();
      runAction(performFormatAndSave);
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
let clipWatchActive = false;

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

  const hintEl = statusMetaEl.querySelector(".status-bar-hint");
  let watchEl = statusMetaEl.querySelector(".status-clip-watch") as HTMLElement | null;
  if (clipWatchActive) {
    if (!watchEl) {
      watchEl = document.createElement("span");
      watchEl.className = "status-clip-watch";
      watchEl.textContent = "clip-watch ON";
    }
    if (hintEl) {
      statusMetaEl.insertBefore(watchEl, hintEl);
    } else {
      statusMetaEl.appendChild(watchEl);
    }
  } else if (watchEl) {
    watchEl.remove();
  }
}

export async function initApp(configSource?: ThemeConfig | Promise<ThemeConfig>) {
  startupMark("ui_init_app_start");
  const container = document.getElementById("app");
  if (!container) throw new Error("Missing #app element");

  const editorEl = document.createElement("div");
  editorEl.id = "editor";
  editorEl.style.flex = "1";
  editorEl.style.overflow = "hidden";
  container.appendChild(editorEl);

  const [note, notes, config] = await Promise.all([
    getOrCreateNote(),
    listNotes(),
    Promise.resolve(configSource ?? getThemeConfigOrDefault()),
  ]);
  startupMark("ui_data_loaded");
  state.setActiveNote(note);
  state.setNotes(notes);

  createStatusBar(container);
  mountEditor(editorEl, {
    markdownAutoformat: config.markdown_autoformat,
    checklistAutoReorder: config.checklist_auto_reorder,
    formatOnSave: config.format_on_save,
    vimMode: !!config.vim_mode,
    dateFormat: config.date_format,
    dateTimeFormat: config.date_time_format,
    variablesEnabled: config.variables_enabled,
    variableAutocompleteMinChars: config.variables_autocomplete_min_chars,
    onExitCommand: handleExitWindow,
    onClipWatchStateChange: (active) => {
      clipWatchActive = active;
      updateStatusBar();
    },
    onClipWatchPaste: (text) => {
      const suffix = text.includes("\n") ? " (multiline)" : "";
      showToast(`clip-watch pasted${suffix}`);
    },
  });
  startupMark("ui_editor_mounted");
  setupKeyboardShortcuts();

  if (config.vim_mode) {
    showToast("Vim mode: :sum, :sum list/row/column/doc, :avg, :avg list/row/column/doc, :date, :notify, :format, :clip-watch, :clip-watch-stop, :q");
  }

  state.on(() => {
    updateStatusBar();
    refreshSwitcher();
  });
}
