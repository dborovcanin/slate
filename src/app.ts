import {
  getOrCreateNote,
  getNote,
  getNoteMeta,
  getNoteRevision,
  unlockNoteAccess,
  listNotesMetaFiltered,
  createNoteWithContext,
  getNoteCollectionIds,
  listCollections,
  createCollection,
  renameCollection,
  updateCollectionDescription,
  deleteCollection,
  purgeCollection,
  listCollectionDefaultTags,
  setCollectionDefaultTags,
  setNoteCollections,
  deleteNote,
  exportToFile,
  exportToPdf,
  getThemeConfigOrDefault,
  getRuntimeFlagsOrDefault,
  setNoteModules,
  type Note,
  type NoteModules,
  type RuntimeFlags,
  type ThemeConfig,
  type NoteSummary,
  type Collection,
  type PdfExportPalette,
} from "./api";
import {
  mountEditor,
  reconfigureEditor,
  setEditorContent,
  focusEditor,
  getEditorView,
  invalidateEditorWikiLinkForNoteId,
  jumpEditorToLine,
  flushSave,
  hasPendingLocalChanges,
  insertTextAtCursor,
  insertImagesFromPathsAtCursor,
  performFormatAndSave,
} from "./editor/editor";
import {
  openSwitcher,
  closeSwitcher,
  isSwitcherOpen,
  refreshSwitcher,
} from "./switcher/switcher";
import {
  openCollectionSwitcher,
  closeCollectionSwitcher,
  isCollectionSwitcherOpen,
} from "./switcher/collection-switcher";
import { parseCollectionTagsInput } from "./collections/tags";
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
import { isNonMarkdownFileNoteId, syntaxLanguageForNoteId } from "./editor/file-note.ts";

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
const LARGE_NOTE_FULL_FEATURE_LINE_LIMIT = 30_000;
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

function noteLineCount(body: string): number {
  if (body.length === 0) return 1;
  let count = 1;
  for (let i = 0; i < body.length; i += 1) {
    if (body.charCodeAt(i) === 10) count += 1;
  }
  return count;
}

function largeNoteReducedFeatures(note: Note | null): boolean {
  return !!note && noteLineCount(note.body) > LARGE_NOTE_FULL_FEATURE_LINE_LIMIT;
}

function autosaveEnabled(): boolean {
  return appConfig?.autosave ?? true;
}

function allowCtrlSSaveShortcut(): boolean {
  if (autosaveEnabled()) return true;
  const vimMode = getEditorView()?.dom.dataset.vimMode;
  return !vimMode || vimMode === "normal";
}

function applyNoteSummaries(summaries: NoteSummary[]) {
  state.setNoteSummaries(summaries);
}

function currentWorkingCollectionId(): string | null {
  return state.workingCollection?.id ?? null;
}

async function refreshNoteSummaries(activeId?: string | null): Promise<NoteSummary[]> {
  const activeNoteId = activeId ?? state.activeNote?.id ?? null;
  let workingCollectionId = currentWorkingCollectionId();
  if (workingCollectionId) {
    try {
      const collections = await listCollections();
      const exists = collections.some((entry) => entry.id === workingCollectionId);
      if (!exists) {
        state.setWorkingCollection(null);
        workingCollectionId = null;
      }
    } catch {
      // Keep current selection on transient collection list failures.
    }
  }
  let summaries: NoteSummary[];
  try {
    summaries = await listNotesMetaFiltered(workingCollectionId, activeNoteId);
  } catch (error) {
    const message = errorMessageOf(error);
    if (workingCollectionId && message.toLowerCase().includes("collection")) {
      state.setWorkingCollection(null);
      summaries = await listNotesMetaFiltered(null, activeNoteId);
    } else {
      throw error;
    }
  }
  applyNoteSummaries(summaries);
  return summaries;
}

function showSaveError(message: string) {
  const trimmed = message.trim();
  if (trimmed.length === 0) {
    showToast("Save failed");
    return;
  }
  showToast(trimmed);
}

function editorOptionsForNote(note: Note | null) {
  if (!appConfig || !appRuntimeFlags) {
    throw new Error("Editor options requested before app config initialization");
  }
  const noteModules = modulesForNote(note, appConfig);
  const loaded = effectiveModules(noteModules, appRuntimeFlags);
  const plainFileNote = !!note && isNonMarkdownFileNoteId(note.id);
  const largeNoteMode = largeNoteReducedFeatures(note);
  const plainCodeLanguage = plainFileNote
    ? (syntaxLanguageForNoteId(note.id) ?? "text")
    : null;
  return {
    plainTextMode: appRuntimeFlags.plain_text_mode,
    plainCodeLanguage,
    detachBackend: appRuntimeFlags.backend_detach,
    disableCalc: plainFileNote || largeNoteMode || appRuntimeFlags.calc_disable || !loaded.math,
    disableMarkdownDecorations:
      plainFileNote || largeNoteMode || appRuntimeFlags.markdown_disable || !loaded.style,
    disableFolding: plainFileNote || largeNoteMode || appRuntimeFlags.folding_disable,
    disableNotify: plainFileNote || largeNoteMode || appRuntimeFlags.notify_disable,
    disableAutocomplete: plainFileNote || largeNoteMode || !loaded.variables,
    tableEnabled: !plainFileNote && !largeNoteMode && loaded.table,
    markdownAutoformat: plainFileNote || largeNoteMode ? false : appConfig.markdown_autoformat,
    checklistAutoReorder:
      plainFileNote || largeNoteMode ? false : appConfig.checklist_auto_reorder,
    autosave: appConfig.autosave,
    formatOnSave: appConfig.format_on_save,
    vimMode: !!appConfig.vim_mode,
    dateFormat: appConfig.date_format,
    dateTimeFormat: appConfig.date_time_format,
    onWriteCommand: (options?: { force?: boolean }) =>
      performFormatAndSave({
        force: options?.force,
        throwOnError: true,
        suppressErrorCallback: true,
      }),
    onSaveError: showSaveError,
    onExportCommand: runExportCommand,
    variablesEnabled: plainFileNote || largeNoteMode ? false : loaded.variables,
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
    onMacroRecordingChange: (register: string | null) => {
      macroRecordingRegister = register;
      updateStatusBar();
    },
    onVimStatusMessage: (message: string) => {
      const trimmed = message.trim();
      if (trimmed.length > 0) {
        showToast(trimmed);
      }
    },
    getNoteModules: () =>
      appConfig ? modulesForNote(state.activeNote, appConfig) : null,
    setNoteModules: (modules: NoteModules) => persistActiveNoteModules(modules),
    onNavigateToNote: (noteId: string, heading?: string) => {
      void switchToNote(noteId, null, heading ?? null);
    },
    onCollectionCommand: handleCollectionCommand,
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

    void refreshNoteSummaries(state.activeNote?.id)
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
      void refreshNoteSummaries(state.activeNote?.id)
        .catch(() => {
          // Keep current list when event-driven metadata refresh fails.
        });
      invalidateEditorWikiLinkForNoteId(payload.id);
    },
  );
}

function openNoteSwitcher() {
  if (isCollectionSwitcherOpen()) {
    closeCollectionSwitcher();
  }
  openSwitcher((noteId, lineNumber) => switchToNote(noteId, lineNumber), (noteId) => {
    void handleDeleteNoteById(noteId).catch((error) => {
      console.error("Action failed:", error);
      showToast("Action failed");
    });
  });
}

function errorMessageOf(error: unknown): string {
  return error instanceof Error ? error.message : typeof error === "string" ? error : String(error);
}

function normalizeHeadingText(value: string): string {
  return value.trim().replace(/\s+/g, " ").toLowerCase();
}

function headingSlug(value: string): string {
  return normalizeHeadingText(value)
    .replace(/[`~!@#$%^&*()+={}\[\]|\\:;"'<>,.?/]/g, "")
    .replace(/\s+/g, "-");
}

function findHeadingLineNumber(body: string, heading: string): number | null {
  const query = normalizeHeadingText(heading);
  const querySlug = headingSlug(heading);
  if (!query && !querySlug) return null;
  const lines = body.split("\n");
  for (let idx = 0; idx < lines.length; idx++) {
    const line = lines[idx] ?? "";
    const match = /^\s{0,3}#{1,6}\s+(.+?)\s*#*\s*$/.exec(line);
    if (!match) continue;
    const title = normalizeHeadingText(match[1] ?? "");
    if (title === query || headingSlug(title) === querySlug) {
      return idx + 1;
    }
  }
  return null;
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

async function switchToNote(id: string, lineNumber?: number | null, heading?: string | null) {
  const saved = await flushSave();
  if (!saved) {
    return;
  }
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
  const headingLine = heading ? findHeadingLineNumber(note.body, heading) : null;
  if (typeof lineNumber === "number" && Number.isFinite(lineNumber) && lineNumber > 0) {
    jumpEditorToLine(lineNumber);
  } else if (typeof headingLine === "number" && headingLine > 0) {
    jumpEditorToLine(headingLine);
  } else {
    focusEditor();
  }

  void refreshNoteSummaries(state.activeNote?.id)
    .catch(() => {
      // Keep existing note list when metadata refresh fails.
    });
}

async function unlockStartupNoteIfNeeded(
  note: Note,
  summaries: readonly NoteSummary[] = [],
): Promise<Note> {
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

async function unlockStartupActiveNoteAfterMount(
  note: Note,
  summaries: readonly NoteSummary[] = [],
) {
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

  await refreshNoteSummaries(state.activeNote?.id).catch(() => summaries);
}

async function handleCreateNote() {
  const saved = await flushSave();
  if (!saved) {
    return;
  }
  const note = await createNoteWithContext(currentWorkingCollectionId());
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
    const saved = await flushSave();
    if (!saved) {
      return;
    }
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
    const note = await createNoteWithContext(currentWorkingCollectionId());
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

function findCollectionByName(collections: Collection[], rawName: string): Collection | null {
  const target = rawName.trim().toLowerCase();
  if (!target) return null;
  return collections.find((entry) => entry.name.trim().toLowerCase() === target) ?? null;
}

async function updateCollectionViaDialog(collection: Collection): Promise<string> {
  const initialTags = await listCollectionDefaultTags(collection.id);
  const next = await openCollectionDialogInApp({
    title: `Update collection: ${collection.name}`,
    confirmText: "Save",
    initialName: collection.name,
    initialDescription: collection.description,
    initialTags,
  });
  if (!next) {
    return "collection update canceled";
  }

  let current = collection;
  if (next.name !== collection.name) {
    current = await renameCollection(collection.id, next.name);
  }
  if (next.description !== current.description) {
    current = await updateCollectionDescription(current.id, next.description);
  }
  await setCollectionDefaultTags(current.id, next.tags);

  if (state.workingCollection?.id === collection.id) {
    state.setWorkingCollection({ id: collection.id, name: current.name });
  }
  await refreshNoteSummaries(state.activeNote?.id);
  return `collection updated: ${current.name}`;
}

async function openCollectionPickerForSession() {
  if (isSwitcherOpen()) {
    closeSwitcher();
  }
  openCollectionSwitcher({
    onChoose: (collection) => {
      runAction(async () => {
        if (!collection) {
          state.setWorkingCollection(null);
          await refreshNoteSummaries(state.activeNote?.id);
          showToast("Working collection cleared");
          return;
        }
        state.setWorkingCollection({ id: collection.id, name: collection.name });
        await refreshNoteSummaries(state.activeNote?.id);
        showToast(`Working collection: ${collection.name}`);
      });
    },
    onEdit: (collection) => {
      runAction(async () => {
        const message = await updateCollectionViaDialog(collection);
        if (message.trim().length > 0) {
          showToast(message);
        }
      });
    },
  });
}

async function handleCollectionCommand(options: {
  action: "choose" | "clear" | "create" | "delete" | "update" | "purge" | "add" | "remove";
  collection: string | null;
}): Promise<string> {
  if (options.action === "clear") {
    state.setWorkingCollection(null);
    await refreshNoteSummaries(state.activeNote?.id);
    return "working collection cleared";
  }

  const rawCollection = options.collection?.trim() ?? "";
  if (!rawCollection) {
    if (options.action === "choose") return "usage: collection choose <collection|none>";
    if (options.action === "create") return "usage: collection create <name>";
    if (options.action === "delete") return "usage: collection delete <name>";
    if (options.action === "update") return "usage: collection update <name>";
    if (options.action === "purge") return "usage: collection purge <name>";
    if (options.action === "add") return "usage: collection join <name>";
    return "usage: collection leave <name>";
  }
  if (options.action === "choose" && rawCollection.toLowerCase() === "none") {
    state.setWorkingCollection(null);
    await refreshNoteSummaries(state.activeNote?.id);
    return "working collection cleared";
  }

  if (options.action === "create") {
    const created = await createCollection(rawCollection, "");
    await refreshNoteSummaries(state.activeNote?.id);
    return `collection created: ${created.name}`;
  }

  const collections = await listCollections();
  const resolved = findCollectionByName(collections, rawCollection);
  if (!resolved) {
    return `collection not found: ${rawCollection}`;
  }

  if (options.action === "delete") {
    await deleteCollection(resolved.id);
    if (state.workingCollection?.id === resolved.id) {
      state.setWorkingCollection(null);
    }
    await refreshNoteSummaries(state.activeNote?.id);
    return `collection deleted: ${resolved.name}`;
  }

  if (options.action === "purge") {
    const activeId = state.activeNote?.id ?? null;
    let activeInPurgedCollection = false;
    if (activeId) {
      const activeCollections = await getNoteCollectionIds(activeId).catch(
        (): string[] => [],
      );
      activeInPurgedCollection = activeCollections.includes(resolved.id);
    }
    const deletedCount = await purgeCollection(resolved.id);
    if (state.workingCollection?.id === resolved.id) {
      state.setWorkingCollection(null);
    }
    await refreshNoteSummaries(state.activeNote?.id);
    if (activeInPurgedCollection) {
      const replacement = await createNoteWithContext(currentWorkingCollectionId());
      state.addNote(replacement);
      state.setActiveNote(replacement);
      reconfigureEditorForNote(replacement);
      setEditorContent("");
      focusEditor();
    }
    return `collection purged: ${resolved.name} (${deletedCount} notes deleted)`;
  }

  if (options.action === "update") {
    return updateCollectionViaDialog(resolved);
  }

  if (options.action === "choose") {
    state.setWorkingCollection({ id: resolved.id, name: resolved.name });
    await refreshNoteSummaries(state.activeNote?.id);
    return `working collection: ${resolved.name}`;
  }

  const active = state.activeNote;
  if (!active) return "no active note";
  const current = await getNoteCollectionIds(active.id);
  const hasMembership = current.includes(resolved.id);
  if (options.action === "add") {
    if (hasMembership) {
      return `note already in collection ${resolved.name}`;
    }
    await setNoteCollections(active.id, [...current, resolved.id]);
    return `added to collection ${resolved.name}`;
  }

  if (!hasMembership) {
    return `note is not in collection ${resolved.name}`;
  }
  await setNoteCollections(
    active.id,
    current.filter((id) => id !== resolved.id),
  );
  return `removed from collection ${resolved.name}`;
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

function clampChannel(value: number): number {
  if (!Number.isFinite(value)) return 0;
  return Math.max(0, Math.min(255, Math.round(value)));
}

function parseCssColorToRgb(input: string): { r: number; g: number; b: number } | null {
  const raw = input.trim();
  if (raw.length === 0) return null;
  const hex = raw.match(/^#([0-9a-f]{3}|[0-9a-f]{6})$/i);
  if (hex) {
    const value = hex[1]!;
    if (value.length === 3) {
      return {
        r: parseInt(value[0]! + value[0]!, 16),
        g: parseInt(value[1]! + value[1]!, 16),
        b: parseInt(value[2]! + value[2]!, 16),
      };
    }
    return {
      r: parseInt(value.slice(0, 2), 16),
      g: parseInt(value.slice(2, 4), 16),
      b: parseInt(value.slice(4, 6), 16),
    };
  }
  const rgb = raw.match(
    /^rgba?\(\s*([+\-]?\d*\.?\d+)\s*,\s*([+\-]?\d*\.?\d+)\s*,\s*([+\-]?\d*\.?\d+)/i,
  );
  if (rgb) {
    return {
      r: clampChannel(Number(rgb[1])),
      g: clampChannel(Number(rgb[2])),
      b: clampChannel(Number(rgb[3])),
    };
  }
  return null;
}

function buildPdfExportPalette(): PdfExportPalette {
  const styles = getComputedStyle(document.documentElement);
  const read = (name: string, fallback: { r: number; g: number; b: number }) =>
    parseCssColorToRgb(styles.getPropertyValue(name)) ?? fallback;
  return {
    fg: read("--fg", { r: 30, g: 32, b: 36 }),
    fg_dim: read("--fg-dim", { r: 109, g: 102, b: 91 }),
    accent: read("--accent", { r: 122, g: 90, b: 58 }),
    variable: read("--code-token-type", { r: 134, g: 99, b: 202 }),
    code_keyword: read("--code-token-keyword", { r: 96, g: 112, b: 181 }),
    code_string: read("--code-token-string", { r: 91, g: 158, b: 111 }),
    code_number: read("--code-token-number", { r: 214, g: 120, b: 67 }),
    code_comment: read("--code-token-comment", { r: 126, g: 138, b: 149 }),
    code_function: read("--code-token-function", { r: 52, g: 122, b: 165 }),
    code_type: read("--code-token-type", { r: 134, g: 99, b: 202 }),
  };
}

async function runExportCommand(options: {
  format: "pdf" | "md" | "txt";
  path: string | null;
}): Promise<string> {
  const note = state.activeNote;
  if (!note) return "no active note";
  await flushSave();
  const latest = state.activeNote;
  if (!latest) return "no active note";

  const path = options.path?.trim() ?? "";
  if (path.length === 0) {
    if (options.format === "pdf") {
      return "usage: export pdf <path>";
    }
    try {
      await navigator.clipboard.writeText(latest.body);
      return `exported ${options.format} to clipboard`;
    } catch (error) {
      const message =
        error instanceof Error
          ? error.message
          : typeof error === "string"
            ? error
            : String(error);
      throw new Error(`clipboard export failed: ${message}`);
    }
  }

  if (options.format === "pdf") {
    await exportToPdf(latest.id, path, latest.body, buildPdfExportPalette());
  } else {
    await exportToFile(path, latest.body);
  }
  return `exported ${options.format} to ${path}`;
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
    if (path.toLowerCase().endsWith(".pdf")) {
      await exportToPdf(note.id, path, note.body, buildPdfExportPalette());
    } else {
      await exportToFile(path, note.body);
    }
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
      { name: "PDF Document", extensions: ["pdf"] },
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

interface CollectionDialogResult {
  name: string;
  description: string;
  tags: string[];
}

function openCollectionDialogInApp(options: {
  title: string;
  confirmText: string;
  initialName: string;
  initialDescription: string;
  initialTags: string[];
}): Promise<CollectionDialogResult | null> {
  return new Promise((resolve) => {
    const restoreTarget =
      document.activeElement instanceof HTMLElement ? document.activeElement : null;

    const overlay = document.createElement("div");
    overlay.className = "app-confirm-overlay";
    overlay.setAttribute("role", "dialog");
    overlay.setAttribute("aria-modal", "true");
    overlay.setAttribute("aria-label", options.title);

    const panel = document.createElement("div");
    panel.className = "app-confirm-panel";

    const titleEl = document.createElement("p");
    titleEl.className = "app-confirm-message";
    titleEl.textContent = options.title;

    const nameField = document.createElement("label");
    nameField.className = "app-collection-field";
    const nameLabel = document.createElement("span");
    nameLabel.className = "app-collection-field-label";
    nameLabel.textContent = "Name";
    const nameInput = document.createElement("input");
    nameInput.type = "text";
    nameInput.className = "app-confirm-input";
    nameInput.value = options.initialName;
    nameInput.placeholder = "Collection name";
    nameField.appendChild(nameLabel);
    nameField.appendChild(nameInput);

    const descField = document.createElement("label");
    descField.className = "app-collection-field";
    const descLabel = document.createElement("span");
    descLabel.className = "app-collection-field-label";
    descLabel.textContent = "Description";
    const descInput = document.createElement("input");
    descInput.type = "text";
    descInput.className = "app-confirm-input";
    descInput.value = options.initialDescription;
    descInput.placeholder = "Short description";
    descField.appendChild(descLabel);
    descField.appendChild(descInput);

    const tagsField = document.createElement("label");
    tagsField.className = "app-collection-field";
    const tagsLabel = document.createElement("span");
    tagsLabel.className = "app-collection-field-label";
    tagsLabel.textContent = "Default tags (comma-separated)";
    const tagsInput = document.createElement("input");
    tagsInput.type = "text";
    tagsInput.className = "app-confirm-input";
    tagsInput.value = options.initialTags.join(", ");
    tagsInput.placeholder = "tag-a, tag-b";
    tagsField.appendChild(tagsLabel);
    tagsField.appendChild(tagsInput);

    const actions = document.createElement("div");
    actions.className = "app-confirm-actions";

    const cancelBtn = document.createElement("button");
    cancelBtn.type = "button";
    cancelBtn.className = "app-confirm-btn";
    cancelBtn.textContent = "Cancel";

    const confirmBtn = document.createElement("button");
    confirmBtn.type = "button";
    confirmBtn.className = "app-confirm-btn app-confirm-btn-danger";
    confirmBtn.textContent = options.confirmText;

    actions.appendChild(cancelBtn);
    actions.appendChild(confirmBtn);

    panel.appendChild(titleEl);
    panel.appendChild(nameField);
    panel.appendChild(descField);
    panel.appendChild(tagsField);
    panel.appendChild(actions);
    overlay.appendChild(panel);
    document.body.appendChild(overlay);

    let finished = false;
    const finish = (value: CollectionDialogResult | null) => {
      if (finished) return;
      finished = true;
      window.removeEventListener("keydown", onKeydown, true);
      overlay.remove();
      if (restoreTarget && restoreTarget.isConnected) {
        restoreTarget.focus();
      }
      resolve(value);
    };

    const submit = () => {
      const name = nameInput.value.trim();
      if (!name) {
        nameInput.focus();
        return;
      }
      finish({
        name,
        description: descInput.value.trim(),
        tags: parseCollectionTagsInput(tagsInput.value),
      });
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
        submit();
        return;
      }
      if (event.key === "Tab") {
        const focusables = [nameInput, descInput, tagsInput, cancelBtn, confirmBtn];
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
    confirmBtn.addEventListener("click", () => submit());
    window.addEventListener("keydown", onKeydown, true);
    nameInput.focus();
    nameInput.select();
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

    // Ctrl+P — note switcher
    if (e.ctrlKey && !e.shiftKey && key === "p") {
      e.preventDefault();
      if (isCollectionSwitcherOpen()) {
        closeCollectionSwitcher();
      }
      if (isSwitcherOpen()) {
        closeSwitcher();
        focusEditor();
      } else {
        openNoteSwitcher();
      }
      return;
    }

    // Ctrl+G — collection switcher
    if (e.ctrlKey && !e.shiftKey && key === "g") {
      e.preventDefault();
      if (isCollectionSwitcherOpen()) {
        closeCollectionSwitcher();
        focusEditor();
      } else {
        runAction(openCollectionPickerForSession);
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

    // Ctrl+Q — quit app window
    if (e.ctrlKey && !e.shiftKey && key === "q") {
      e.preventDefault();
      runAction(handleExitWindow);
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

    // Escape — close active picker
    if (e.key === "Escape") {
      if (isCollectionSwitcherOpen()) {
        e.preventDefault();
        closeCollectionSwitcher();
        focusEditor();
        return;
      }
      if (isSwitcherOpen()) {
        e.preventDefault();
        closeSwitcher();
        focusEditor();
        return;
      }
    }

    // Ctrl+S - format & save
    if (e.ctrlKey && !e.shiftKey && key === "s") {
      if (e.defaultPrevented) {
        return;
      }
      e.preventDefault();
      if (!allowCtrlSSaveShortcut()) {
        showToast("autosave off; use :w");
      } else {
        runAction(performFormatAndSave);
      }
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
let macroRecordingRegister: string | null = null;

function createStatusBar(container: HTMLElement) {
  const bar = document.createElement("div");
  bar.className = "status-bar";

  statusTitleEl = document.createElement("span");
  statusTitleEl.className = "status-bar-title";

  statusMetaEl = document.createElement("span");
  statusMetaEl.className = "status-bar-meta";

  const hint = document.createElement("span");
  hint.className = "status-bar-hint";
  hint.textContent = "Ctrl+P notes | Ctrl+G collections";

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
  let collectionEl = statusMetaEl.querySelector(".status-collection") as HTMLElement | null;
  if (state.workingCollection) {
    if (!collectionEl) {
      collectionEl = document.createElement("span");
      collectionEl.className = "status-collection";
    }
    collectionEl.textContent = `collection ${state.workingCollection.name}`;
    if (hintEl) {
      statusMetaEl.insertBefore(collectionEl, hintEl);
    } else {
      statusMetaEl.appendChild(collectionEl);
    }
  } else if (collectionEl) {
    collectionEl.remove();
  }

  let modulesEl = statusMetaEl.querySelector(".status-modules") as HTMLElement | null;
  if (appConfig && appRuntimeFlags) {
    let loaded = effectiveModules(
      modulesForNote(state.activeNote, appConfig),
      appRuntimeFlags,
    );
    if (largeNoteReducedFeatures(state.activeNote)) {
      loaded = {
        math: false,
        table: false,
        variables: false,
        style: false,
      };
    }
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

  let largeModeEl = statusMetaEl.querySelector(".status-large-note") as HTMLElement | null;
  if (largeNoteReducedFeatures(state.activeNote)) {
    if (!largeModeEl) {
      largeModeEl = document.createElement("span");
      largeModeEl.className = "status-large-note";
    }
    largeModeEl.textContent = `large-note mode >${LARGE_NOTE_FULL_FEATURE_LINE_LIMIT.toLocaleString()} lines`;
    if (hintEl) {
      statusMetaEl.insertBefore(largeModeEl, hintEl);
    } else {
      statusMetaEl.appendChild(largeModeEl);
    }
  } else if (largeModeEl) {
    largeModeEl.remove();
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

  let macroEl = statusMetaEl.querySelector(".status-macro-recording") as HTMLElement | null;
  if (appConfig?.vim_mode && macroRecordingRegister) {
    if (!macroEl) {
      macroEl = document.createElement("span");
      macroEl.className = "status-macro-recording";
    }
    macroEl.textContent = `REC @${macroRecordingRegister}`;
    if (hintEl) {
      statusMetaEl.insertBefore(macroEl, hintEl);
    } else {
      statusMetaEl.appendChild(macroEl);
    }
  } else if (macroEl) {
    macroEl.remove();
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

  const notePromise = getOrCreateNote();
  const configPromise = Promise.resolve(configSource ?? getThemeConfigOrDefault());
  const runtimeFlagsPromise = getRuntimeFlagsOrDefault();
  const note = await notePromise;
  const [config, runtimeFlags] = await Promise.all([
    configPromise,
    runtimeFlagsPromise,
  ]);
  appConfig = config;
  appRuntimeFlags = runtimeFlags;
  startupMark("ui_data_loaded");
  state.setActiveNote(note);

  createStatusBar(container);
  mountEditor(editorEl, editorOptionsForNote(note));
  startupMark("ui_editor_mounted");
  const win = getCurrentWindow();
  void win.onDragDropEvent((event) => {
    if (event.payload.type !== "drop") return;
    void insertImagesFromPathsAtCursor(event.payload.paths)
      .then((inserted) => {
        if (inserted) showToast("image inserted");
      })
      .catch((error) => {
        console.error("Window drag-drop image import failed:", error);
      });
  });
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
  } else if (largeNoteReducedFeatures(note)) {
    showToast(
      `Large-note mode: folding, decorations, and calc disabled above ${LARGE_NOTE_FULL_FEATURE_LINE_LIMIT.toLocaleString()} lines`,
    );
  } else if (config.vim_mode) {
    showToast("Vim mode: :sum, :sum list/row/column/doc, :avg, :avg list/row/column/doc, :date, :notify, :format, :clip-watch on, :clip-watch off, :w, :wq, :q");
  }

  state.on(() => {
    updateStatusBar();
    refreshSwitcher();
  });

  void startBackendNoteChangeListener().catch((error) => {
    console.error("Backend note-change listener failed:", error);
  });
  startActiveNoteSyncLoop();

  void listNotesMetaFiltered(currentWorkingCollectionId(), note.id)
    .then((summaries) => {
      applyNoteSummaries(summaries);
    })
    .catch(() => {
      // Keep startup non-blocking when note metadata fetch fails.
    });

  // Defer unlock prompt until after mount so init can complete and the window can show.
  void unlockStartupActiveNoteAfterMount(note).catch((error) => {
    console.error("Startup unlock follow-up failed:", error);
  });
}
