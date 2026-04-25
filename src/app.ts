import {
  getOrCreateNote,
  getNote,
  getNoteMeta,
  getNoteRevision,
  unlockNoteAccess,
  listNotesMeta,
  createNote,
  deleteNote,
  exportToFile,
  getThemeConfigOrDefault,
  getRuntimeFlagsOrDefault,
  setNoteModules,
  type Note,
  type NoteModules,
  type RuntimeFlags,
  type ThemeConfig,
  type NoteSummary,
} from "./api";
import {
  mountEditor,
  reconfigureEditor,
  setEditorContent,
  focusEditor,
  flushSave,
  hasPendingLocalChanges,
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
import { save } from "@tauri-apps/plugin-dialog";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { changeFontSize, cycleFont, getFontLabel } from "./theme/theme";
import { openDatePicker } from "./editor/date-picker";
import { startupMark } from "./perf/startup.ts";
import {
  effectiveModules,
  modulesForNote,
  normalizeModules,
} from "./editor/module-gating.ts";

function ensureSummaryIncludesActive(noteId: string, body: string, summaries: NoteSummary[]): NoteSummary[] {
  if (summaries.some((summary) => summary.id === noteId)) return summaries;
  const active = state.activeNote;
  const activeEntry = state.notes.find((entry) => entry.id === noteId);
  return [
    {
      id: noteId,
      title: activeEntry?.title ?? (active && active.is_unlocked ? body.split("\n").find((line) => line.trim().length > 0)?.trim() ?? "Untitled" : "Untitled"),
      body_prefix: body.slice(0, 200),
      access_mode: active?.id === noteId ? active.access_mode : "none",
      is_unlocked: active?.id === noteId ? active.is_unlocked : true,
      updated_at: active?.id === noteId ? active.updated_at : "",
    },
    ...summaries,
  ];
}

function moduleIndicatorText(modules: NoteModules): string {
  const labels: string[] = [];
  if (modules.math) labels.push("math");
  if (modules.table) labels.push("table");
  if (modules.variables) labels.push("variables");
  if (modules.style) labels.push("style");
  if (labels.length === 0) return "modules OFF";
  return `modules ${labels.join(" ")}`;
}

const ACTIVE_NOTE_SYNC_INTERVAL_MS = 2500;
const NOTE_CHANGED_EVENT = "slate://note-changed";
let activeNoteSyncTimer: number | null = null;
let activeNoteSyncInFlight = false;
let stopBackendNoteChangeListener: UnlistenFn | null = null;
let appConfig: ThemeConfig | null = null;
let appRuntimeFlags: RuntimeFlags | null = null;

interface BackendNoteChangedEvent {
  id: string;
  updatedAt?: string | null;
  deleted?: boolean;
}

function applyNoteSummaries(summaries: NoteSummary[]) {
  const active = state.activeNote;
  if (!active) {
    state.setNoteSummaries(summaries);
    return;
  }
  state.setNoteSummaries(ensureSummaryIncludesActive(active.id, active.body, summaries));
}

function editorOptionsForNote(note: Note | null) {
  if (!appConfig || !appRuntimeFlags) {
    throw new Error("Editor options requested before app config initialization");
  }
  const noteModules = modulesForNote(note, appConfig);
  const loaded = effectiveModules(noteModules, appRuntimeFlags);
  return {
    plainTextMode: appRuntimeFlags.plain_text_mode,
    detachBackend: appRuntimeFlags.backend_detach,
    disableCalc: appRuntimeFlags.calc_disable || !loaded.math,
    disableMarkdownDecorations: appRuntimeFlags.markdown_disable || !loaded.style,
    disableFolding: appRuntimeFlags.folding_disable,
    disableNotify: appRuntimeFlags.notify_disable,
    disableAutocomplete: !loaded.variables,
    tableEnabled: loaded.table,
    markdownAutoformat: appConfig.markdown_autoformat && loaded.style,
    checklistAutoReorder: appConfig.checklist_auto_reorder && loaded.style,
    formatOnSave: appConfig.format_on_save,
    vimMode: !!appConfig.vim_mode,
    dateFormat: appConfig.date_format,
    dateTimeFormat: appConfig.date_time_format,
    variablesEnabled: loaded.variables,
    variableAutocompleteMinChars: appConfig.variables_autocomplete_min_chars,
    onExitCommand: handleExitWindow,
    onClipWatchStateChange: (active: boolean) => {
      clipWatchActive = active;
      updateStatusBar();
    },
    onClipWatchPaste: (text: string) => {
      const suffix = text.includes("\n") ? " (multiline)" : "";
      showToast(`clip-watch pasted${suffix}`);
    },
    getNoteModules: () =>
      appConfig ? modulesForNote(state.activeNote, appConfig) : null,
    setNoteModules: (modules: NoteModules) => persistActiveNoteModules(modules),
  };
}

function reconfigureEditorForNote(note: Note | null) {
  if (!appConfig || !appRuntimeFlags) return;
  reconfigureEditor(editorOptionsForNote(note));
}

async function persistActiveNoteModules(modules: NoteModules) {
  const active = state.activeNote;
  if (!active) throw new Error("No active note");
  const activeId = active.id;
  const nextModules = normalizeModules(modules);
  if (
    active.modules.math === nextModules.math &&
    active.modules.table === nextModules.table &&
    active.modules.variables === nextModules.variables &&
    active.modules.style === nextModules.style
  ) {
    return;
  }

  const saved = await setNoteModules(activeId, nextModules);
  if (state.activeNote?.id !== activeId) {
    return;
  }
  state.setActiveNote(saved);
  reconfigureEditorForNote(saved);
}

async function syncActiveNoteIfBackendChanged() {
  if (activeNoteSyncInFlight) return;
  const active = state.activeNote;
  if (!active) return;
  if (hasPendingLocalChanges()) return;

  const activeId = active.id;
  const activeUpdatedAt = active.updated_at;
  activeNoteSyncInFlight = true;
  try {
    const remoteRevision = await getNoteRevision(activeId);
    if (state.activeNote?.id !== activeId || hasPendingLocalChanges()) {
      return;
    }
    if (!remoteRevision || remoteRevision === activeUpdatedAt) {
      return;
    }

    const latest = await getNote(activeId);
    if (!latest) return;
    if (state.activeNote?.id !== activeId || hasPendingLocalChanges()) {
      return;
    }
    const sameBody = latest.body === active.body;
    state.setActiveNote(latest);
    reconfigureEditorForNote(latest);
    if (!sameBody) {
      setEditorContent(latest.body, { forceStateReset: true });
      focusEditor();
    }

    void listNotesMeta()
      .then((summaries) => {
        applyNoteSummaries(summaries);
      })
      .catch(() => {
        // Best-effort note list refresh after backend sync.
      });
  } catch {
    // Keep current UI state on intermittent refresh failures.
  } finally {
    activeNoteSyncInFlight = false;
  }
}

function startActiveNoteSyncLoop() {
  if (activeNoteSyncTimer !== null) {
    window.clearInterval(activeNoteSyncTimer);
  }
  activeNoteSyncTimer = window.setInterval(() => {
    void syncActiveNoteIfBackendChanged();
  }, ACTIVE_NOTE_SYNC_INTERVAL_MS);
}

async function startBackendNoteChangeListener() {
  if (stopBackendNoteChangeListener) {
    stopBackendNoteChangeListener();
    stopBackendNoteChangeListener = null;
  }
  stopBackendNoteChangeListener = await listen<BackendNoteChangedEvent>(
    NOTE_CHANGED_EVENT,
    (event) => {
      const payload = event.payload;
      if (!payload?.id) return;

      if (state.activeNote?.id === payload.id) {
        void syncActiveNoteIfBackendChanged();
      }
      void listNotesMeta()
        .then((summaries) => {
          applyNoteSummaries(summaries);
        })
        .catch(() => {
          // Keep current list when event-driven metadata refresh fails.
        });
    },
  );
}

function openNoteSwitcher() {
  openSwitcher(switchToNote, (noteId) => {
    void handleDeleteNoteById(noteId).catch((error) => {
      console.error("Action failed:", error);
      showToast("Action failed");
    });
  });
}

function errorMessageOf(error: unknown): string {
  return error instanceof Error ? error.message : typeof error === "string" ? error : String(error);
}

async function unlockProtectedNoteWithRetry(noteId: string, title: string): Promise<Note | null> {
  while (true) {
    const password = await promptPasswordInApp(
      `Enter password to open "${title}".`,
      "Open",
    );
    if (password === null) {
      return null;
    }

    try {
      return await unlockNoteAccess(noteId, password);
    } catch (error) {
      const message = errorMessageOf(error);
      if (message.includes("invalid password")) {
        showToast("Invalid password");
        continue;
      }
      if (message.includes("password")) {
        showToast("Password required");
        continue;
      }
      console.error("Open note failed:", error);
      showToast("Open failed");
      return null;
    }
  }
}

async function switchToNote(id: string) {
  await flushSave();
  const summary = await getNoteMeta(id);
  if (!summary) {
    showToast("Note not found");
    return;
  }

  let note: Note | null = null;
  if (summary.access_mode !== "none" && !summary.is_unlocked) {
    note = await unlockProtectedNoteWithRetry(id, summary.title);
    if (!note) {
      openNoteSwitcher();
      return;
    }
  } else {
    note = await getNote(id);
    if (!note) {
      showToast("Note not found");
      return;
    }
  }

  state.setActiveNote(note);
  reconfigureEditorForNote(note);
  setEditorContent(note.body, { forceStateReset: true });
  focusEditor();

  void listNotesMeta()
    .then((summaries) => {
      applyNoteSummaries(summaries);
    })
    .catch(() => {
      // Keep existing note list when metadata refresh fails.
    });
}

async function unlockStartupNoteIfNeeded(note: Note, summaries: NoteSummary[]): Promise<Note> {
  if (note.access_mode === "none" || note.is_unlocked) {
    return note;
  }
  const title = summaries.find((entry) => entry.id === note.id)?.title ?? note.id;
  const unlocked = await unlockProtectedNoteWithRetry(note.id, title);
  if (!unlocked) {
    openNoteSwitcher();
    return note;
  }
  return unlocked;
}

async function unlockStartupActiveNoteAfterMount(note: Note, summaries: NoteSummary[]) {
  if (note.access_mode === "none" || note.is_unlocked) {
    return;
  }

  const unlocked = await unlockStartupNoteIfNeeded(note, summaries);
  if (!unlocked.is_unlocked) {
    return;
  }
  if (state.activeNote?.id !== note.id || hasPendingLocalChanges()) {
    return;
  }

  state.setActiveNote(unlocked);
  reconfigureEditorForNote(unlocked);
  setEditorContent(unlocked.body, { forceStateReset: true });
  focusEditor();

  const refreshed = await listNotesMeta().catch(() => summaries);
  applyNoteSummaries(refreshed);
}

async function handleCreateNote() {
  await flushSave();
  const note = await createNote();
  state.addNote(note);
  state.setActiveNote(note);
  reconfigureEditorForNote(note);
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

  if (!(await confirmInApp(`Delete "${noteEntry.title}"? This cannot be undone.`))) {
    return;
  }

  const deletingActive = state.activeNote?.id === noteId;
  const adjacentId =
    deletingActive ? state.getAdjacentNoteId(1) ?? state.getAdjacentNoteId(-1) : null;

  if (deletingActive) {
    await flushSave();
  }

  let deletePassword: string | null = null;
  if (noteEntry.accessMode !== "none") {
    deletePassword = await promptPasswordInApp(
      `Enter password to delete "${noteEntry.title}".`,
      "Delete",
    );
    if (deletePassword === null) return;
  }

  try {
    await deleteNote(noteId, deletePassword);
  } catch (error) {
    const message =
      error instanceof Error ? error.message : typeof error === "string" ? error : String(error);
    if (message.includes("invalid password")) {
      showToast("Invalid password");
    } else if (message.includes("password required")) {
      showToast("Password required");
    } else {
      console.error("Delete note failed:", error);
      showToast("Delete failed");
    }
    return;
  }
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
    reconfigureEditorForNote(note);
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
  const latest = state.activeNote;
  if (!latest) return;
  try {
    await navigator.clipboard.writeText(latest.body);
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
  const latest = state.activeNote;
  if (!latest) return;

  const filename = deriveFilename(latest.body);
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

function confirmInApp(
  message: string,
  labels: { confirm?: string; cancel?: string } = {},
): Promise<boolean> {
  return new Promise((resolve) => {
    const restoreTarget =
      document.activeElement instanceof HTMLElement ? document.activeElement : null;
    const confirmText = labels.confirm ?? "Delete";
    const cancelText = labels.cancel ?? "Cancel";

    const overlay = document.createElement("div");
    overlay.className = "app-confirm-overlay";
    overlay.setAttribute("role", "dialog");
    overlay.setAttribute("aria-modal", "true");
    overlay.setAttribute("aria-label", "Confirm action");

    const panel = document.createElement("div");
    panel.className = "app-confirm-panel";

    const messageEl = document.createElement("p");
    messageEl.className = "app-confirm-message";
    messageEl.textContent = message;

    const actions = document.createElement("div");
    actions.className = "app-confirm-actions";

    const cancelBtn = document.createElement("button");
    cancelBtn.type = "button";
    cancelBtn.className = "app-confirm-btn";
    cancelBtn.textContent = cancelText;

    const confirmBtn = document.createElement("button");
    confirmBtn.type = "button";
    confirmBtn.className = "app-confirm-btn app-confirm-btn-danger";
    confirmBtn.textContent = confirmText;

    actions.appendChild(cancelBtn);
    actions.appendChild(confirmBtn);
    panel.appendChild(messageEl);
    panel.appendChild(actions);
    overlay.appendChild(panel);
    document.body.appendChild(overlay);

    let finished = false;

    const finish = (result: boolean) => {
      if (finished) return;
      finished = true;
      window.removeEventListener("keydown", onKeydown, true);
      overlay.remove();
      if (restoreTarget && restoreTarget.isConnected) {
        restoreTarget.focus();
      }
      resolve(result);
    };

    const onKeydown = (event: KeyboardEvent) => {
      if (event.key !== "Tab") {
        event.preventDefault();
        event.stopPropagation();
      }

      if (event.key === "Escape") {
        finish(false);
        return;
      }

      if (
        event.key === "Enter" &&
        !event.ctrlKey &&
        !event.metaKey &&
        !event.altKey &&
        !event.shiftKey
      ) {
        finish(document.activeElement !== cancelBtn);
      }
    };

    overlay.addEventListener("mousedown", (event) => {
      if (event.target === overlay) {
        finish(false);
      }
    });

    cancelBtn.addEventListener("click", () => finish(false));
    confirmBtn.addEventListener("click", () => finish(true));
    window.addEventListener("keydown", onKeydown, true);
    cancelBtn.focus();
  });
}

function promptPasswordInApp(message: string, confirmText = "Continue"): Promise<string | null> {
  return new Promise((resolve) => {
    const restoreTarget =
      document.activeElement instanceof HTMLElement ? document.activeElement : null;

    const overlay = document.createElement("div");
    overlay.className = "app-confirm-overlay";
    overlay.setAttribute("role", "dialog");
    overlay.setAttribute("aria-modal", "true");
    overlay.setAttribute("aria-label", "Password required");

    const panel = document.createElement("div");
    panel.className = "app-confirm-panel";

    const messageEl = document.createElement("p");
    messageEl.className = "app-confirm-message";
    messageEl.textContent = message;

    const input = document.createElement("input");
    input.type = "password";
    input.className = "app-confirm-input";
    input.placeholder = "Password";
    input.autocomplete = "current-password";

    const actions = document.createElement("div");
    actions.className = "app-confirm-actions";

    const cancelBtn = document.createElement("button");
    cancelBtn.type = "button";
    cancelBtn.className = "app-confirm-btn";
    cancelBtn.textContent = "Cancel";

    const confirmBtn = document.createElement("button");
    confirmBtn.type = "button";
    confirmBtn.className = "app-confirm-btn app-confirm-btn-danger";
    confirmBtn.textContent = confirmText;

    actions.appendChild(cancelBtn);
    actions.appendChild(confirmBtn);
    panel.appendChild(messageEl);
    panel.appendChild(input);
    panel.appendChild(actions);
    overlay.appendChild(panel);
    document.body.appendChild(overlay);

    let finished = false;

    const finish = (value: string | null) => {
      if (finished) return;
      finished = true;
      window.removeEventListener("keydown", onKeydown, true);
      overlay.remove();
      if (restoreTarget && restoreTarget.isConnected) {
        restoreTarget.focus();
      }
      resolve(value);
    };

    const onKeydown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.preventDefault();
        event.stopPropagation();
        finish(null);
        return;
      }
      if (
        event.key === "Enter" &&
        !event.ctrlKey &&
        !event.metaKey &&
        !event.altKey &&
        !event.shiftKey
      ) {
        event.preventDefault();
        event.stopPropagation();
        finish(input.value.trim().length > 0 ? input.value : null);
        return;
      }
      if (event.key === "Tab") {
        const focusables = [input, cancelBtn, confirmBtn];
        const first = focusables[0];
        const last = focusables[focusables.length - 1];
        const active = document.activeElement;
        if (event.shiftKey && active === first) {
          event.preventDefault();
          last.focus();
        } else if (!event.shiftKey && active === last) {
          event.preventDefault();
          first.focus();
        }
      }
    };

    overlay.addEventListener("mousedown", (event) => {
      if (event.target === overlay) {
        finish(null);
      }
    });

    cancelBtn.addEventListener("click", () => finish(null));
    confirmBtn.addEventListener("click", () =>
      finish(input.value.trim().length > 0 ? input.value : null),
    );
    window.addEventListener("keydown", onKeydown, true);
    input.focus();
  });
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

    // Ctrl+P — title switcher
    if (e.ctrlKey && !e.shiftKey && key === "p") {
      e.preventDefault();
      if (isSwitcherOpen()) {
        closeSwitcher();
        focusEditor();
      } else {
        openNoteSwitcher();
      }
      return;
    }

    // Ctrl+Shift+P — content search
    if (e.ctrlKey && e.shiftKey && key === "p") {
      e.preventDefault();
      if (isSwitcherOpen()) {
        closeSwitcher();
        focusEditor();
      } else {
        openSwitcher(switchToNote, undefined, "content");
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
  let modulesEl = statusMetaEl.querySelector(".status-modules") as HTMLElement | null;
  if (appConfig && appRuntimeFlags) {
    const loaded = effectiveModules(
      modulesForNote(state.activeNote, appConfig),
      appRuntimeFlags,
    );
    const textModules = moduleIndicatorText(loaded);
    if (!modulesEl) {
      modulesEl = document.createElement("span");
      modulesEl.className = "status-modules";
    }
    modulesEl.textContent = textModules;
    if (hintEl) {
      statusMetaEl.insertBefore(modulesEl, hintEl);
    } else {
      statusMetaEl.appendChild(modulesEl);
    }
  } else if (modulesEl) {
    modulesEl.remove();
  }

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

  let note = await getOrCreateNote();
  let [summaries, config, runtimeFlags] = await Promise.all([
    listNotesMeta().catch(() => []),
    Promise.resolve(configSource ?? getThemeConfigOrDefault()),
    getRuntimeFlagsOrDefault(),
  ]);
  appConfig = config;
  appRuntimeFlags = runtimeFlags;
  startupMark("ui_data_loaded");
  state.setActiveNote(note);
  applyNoteSummaries(summaries);

  createStatusBar(container);
  mountEditor(editorEl, editorOptionsForNote(note));
  startupMark("ui_editor_mounted");
  setupKeyboardShortcuts();

  const activeFlags: string[] = [];
  if (runtimeFlags.plain_text_mode) activeFlags.push("SLATE_PLAIN_TEXT_MODE");
  if (runtimeFlags.backend_detach) activeFlags.push("SLATE_BACKEND_DETACH");
  if (runtimeFlags.calc_disable) activeFlags.push("SLATE_CALC_DISABLE");
  if (runtimeFlags.markdown_disable) activeFlags.push("SLATE_MARKDOWN_DISABLE");
  if (runtimeFlags.folding_disable) activeFlags.push("SLATE_FOLDING_DISABLE");
  if (runtimeFlags.notify_disable) activeFlags.push("SLATE_NOTIFY_DISABLE");
  if (runtimeFlags.autocomplete_disable) activeFlags.push("SLATE_AUTOCOMPLETE_DISABLE");
  if (activeFlags.length > 0) {
    showToast(`Runtime flags: ${activeFlags.join(", ")}`);
  } else if (config.vim_mode) {
    showToast("Vim mode: :sum, :sum list/row/column/doc, :avg, :avg list/row/column/doc, :date, :notify, :format, :clip-watch, :clip-watch-stop, :q");
  }

  state.on(() => {
    updateStatusBar();
    refreshSwitcher();
  });

  void startBackendNoteChangeListener().catch((error) => {
    console.error("Backend note-change listener failed:", error);
  });
  startActiveNoteSyncLoop();

  // Defer unlock prompt until after mount so init can complete and the window can show.
  void unlockStartupActiveNoteAfterMount(note, summaries).catch((error) => {
    console.error("Startup unlock follow-up failed:", error);
  });
}
