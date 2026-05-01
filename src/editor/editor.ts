import { Annotation, EditorState, Prec, type Extension, type Text } from "@codemirror/state";
import {
  EditorView,
  keymap,
  drawSelection,
  highlightActiveLine,
  placeholder,
  type ViewUpdate,
} from "@codemirror/view";
import { defaultKeymap, deleteGroupBackward, history, historyKeymap, cursorGroupLeft, cursorGroupRight } from "@codemirror/commands";
import { state } from "../state";
import {
  saveNote,
  type NoteModules,
} from "../api";
import { calcExtensions } from "./calc-decoration";
import { commandModeExtension } from "./command-picker";
import {
  markdownRichTextExtensions,
  requestMarkdownDecorationRefresh,
} from "./markdown-decoration";
import {
  markdownEditingExtensions,
  runTableCellNavigationCommand,
  runTableHeaderDeleteColumnCommand,
} from "./markdown-editing";
import { foldingExtensions } from "./folding.ts";
import { notifyExtensions } from "./notify-decoration";
import { makeVariableCompletionSource, variableAutocompleteExtensions } from "./variable-autocomplete";
import {
  invalidateWikiLinkCompletionCaches,
  wikiLinkCompletionSource,
  wikiLinkExtensions,
} from "./wiki-link.ts";
import { autocompletion } from "@codemirror/autocomplete";
import { editorSearchExtensions } from "./search";
import { vimModeExtension } from "./vim";
import { editorContextMenuExtensions } from "./context-menu";
import {
  imageImportDomHandlers,
  insertImagePathsAtCursor,
} from "./image-import";
import { startupMark } from "../perf/startup.ts";
import {
  editorProfilerNowMs,
  recordEditorProfilerSample,
} from "../perf/editor-profiler.ts";
import {
  AutosaveSnapshotTracker,
  type AutosaveSnapshot,
} from "./autosave-snapshot.ts";

let view: EditorView | null = null;
let saveTimer: number | null = null;
let mountedExtensions: Extension[] = [];
let suppressProgrammaticDocSync = false;
let localDirty = false;
let saveInFlight = false;
let currentAutosaveEnabled = true;
let onSaveError: ((message: string) => void) | null = null;
let lastSaveErrorMessage = "";
let lastSaveErrorAt = 0;
const SAVE_DEBOUNCE_MS = 500;
const SAVE_ERROR_THROTTLE_MS = 1500;
const TITLE_PREVIEW_LIMIT = 60;
const LARGE_DOC_STATE_RESET_THRESHOLD = 200_000;
const suppressEditorSyncAnnotation = Annotation.define<boolean>();
const autosaveSnapshots = new AutosaveSnapshotTracker({
  onSample: (sample) => {
    recordEditorProfilerSample(sample.name, sample.durationMs, {
      reason: sample.reason,
      metrics: sample.metrics,
    });
  },
});

function errorMessageOf(error: unknown): string {
  if (error instanceof Error) return error.message;
  if (typeof error === "string") return error;
  return String(error);
}

function reportSaveError(message: string) {
  if (!onSaveError) return;
  const now = Date.now();
  if (message === lastSaveErrorMessage && now - lastSaveErrorAt < SAVE_ERROR_THROTTLE_MS) {
    return;
  }
  lastSaveErrorMessage = message;
  lastSaveErrorAt = now;
  onSaveError(message);
}

function deriveTitleFromDoc(doc: Text): string {
  for (let i = 1; i <= doc.lines; i++) {
    const line = doc.line(i).text.trim();
    if (line.length === 0) continue;
    return line.length > TITLE_PREVIEW_LIMIT
      ? `${line.slice(0, TITLE_PREVIEW_LIMIT)}...`
      : line;
  }
  return "Untitled";
}

function scheduleSave() {
  if (!currentAutosaveEnabled) return;
  if (saveTimer !== null) clearTimeout(saveTimer);
  saveTimer = window.setTimeout(flushSave, SAVE_DEBOUNCE_MS);
}

async function snapshotBodyForSave(doc: Text): Promise<AutosaveSnapshot> {
  const startedAt = editorProfilerNowMs();
  const snapshot = await autosaveSnapshots.bodyForDoc(doc);
  recordEditorProfilerSample(
    "editor.autosave.snapshotWait",
    editorProfilerNowMs() - startedAt,
    {
      reason: snapshot.source,
      metrics: {
        docLength: snapshot.docLength,
        lineCount: snapshot.lineCount,
        bodyLength: snapshot.body.length,
      },
    },
  );
  return snapshot;
}

async function currentStableSnapshotForSave(): Promise<AutosaveSnapshot | null> {
  if (!view) return null;
  for (;;) {
    const doc = view.state.doc;
    const snapshot = await snapshotBodyForSave(doc);
    if (!view || view.state.doc === doc) return snapshot;
  }
}

export async function flushSave(
  force = false,
  forceWrite = false,
  options?: { throwOnError?: boolean; suppressErrorCallback?: boolean },
): Promise<boolean> {
  if (saveTimer !== null) {
    clearTimeout(saveTimer);
    saveTimer = null;
  }
  if (!force && !currentAutosaveEnabled) return true;
  if (backendDetached) return true;
  const note = state.activeNote;
  if (!note) return true;
  const flushStartedAt = editorProfilerNowMs();
  const snapshot = view ? await currentStableSnapshotForSave() : null;
  const body = snapshot?.body ?? note.body;
  const snapshotVersion = snapshot?.version ?? autosaveSnapshots.version;
  const bodyLineCount = snapshot?.lineCount ?? body.split("\n").length;
  saveInFlight = true;
  try {
    const saveStartedAt = editorProfilerNowMs();
    const saved = await saveNote(note.id, body, {
      expectedRevision: note.updated_at,
      force: forceWrite,
    });
    recordEditorProfilerSample(
      "editor.autosave.saveNote",
      editorProfilerNowMs() - saveStartedAt,
      {
        reason: force ? "manual" : "autosave",
        metrics: {
          bodyLength: body.length,
          lineCount: bodyLineCount,
          forceWrite: forceWrite ? 1 : 0,
        },
      },
    );
    if (state.activeNote?.id === note.id) {
      state.updateBody(body, saved.updated_at);
    }
    if (snapshotVersion === autosaveSnapshots.version && state.activeNote?.id === note.id) {
      localDirty = false;
    } else {
      localDirty = true;
      scheduleSave();
    }
    recordEditorProfilerSample(
      "editor.autosave.flush",
      editorProfilerNowMs() - flushStartedAt,
      {
        reason: force ? "manual_success" : "autosave_success",
        metrics: {
          bodyLength: body.length,
          lineCount: bodyLineCount,
          staleDuringSave: snapshotVersion === autosaveSnapshots.version ? 0 : 1,
        },
      },
    );
    return true;
  } catch (e) {
    recordEditorProfilerSample(
      "editor.autosave.flush",
      editorProfilerNowMs() - flushStartedAt,
      {
        reason: force ? "manual_error" : "autosave_error",
        metrics: {
          bodyLength: body.length,
          lineCount: bodyLineCount,
        },
      },
    );
    const message = errorMessageOf(e);
    console.error("Failed to save note:", e);
    if (!options?.suppressErrorCallback) {
      reportSaveError(message);
    }
    if (options?.throwOnError) {
      throw new Error(message);
    }
    return false;
  } finally {
    saveInFlight = false;
  }
}

// The title comes from the first non-empty line, so only re-derive when a
// change touches the opening region of the doc.
const TITLE_REGION_BYTES = 2000;

function editTouchesTitleRegion(update: ViewUpdate): boolean {
  let touches = false;
  update.changes.iterChangedRanges((fromA, _toA, fromB) => {
    if (fromA < TITLE_REGION_BYTES || fromB < TITLE_REGION_BYTES) touches = true;
  });
  return touches;
}

const onUpdate = EditorView.updateListener.of((update) => {
  if (update.docChanged) {
    if (backendDetached) return;
    const active = state.activeNote;
    if (active && active.access_mode !== "none" && !active.is_unlocked) {
      return;
    }
    if (suppressProgrammaticDocSync) return;
    if (
      update.transactions.some((transaction) =>
        transaction.annotation(suppressEditorSyncAnnotation),
      )
    ) {
      return;
    }
    localDirty = true;
    autosaveSnapshots.markDirty(update.state.doc);
    if (editTouchesTitleRegion(update)) {
      state.updateDraftTitle(deriveTitleFromDoc(update.state.doc));
    }
    scheduleSave();
  }
});

interface EditorMountOptions {
  plainTextMode?: boolean;
  detachBackend?: boolean;
  disableCalc?: boolean;
  disableMarkdownDecorations?: boolean;
  disableFolding?: boolean;
  disableNotify?: boolean;
  disableAutocomplete?: boolean;
  tableEnabled?: boolean;
  markdownAutoformat?: boolean;
  checklistAutoReorder?: boolean;
  autosave?: boolean;
  formatOnSave?: boolean;
  vimMode?: boolean;
  dateFormat?: string;
  dateTimeFormat?: string;
  onWriteCommand?: (options?: { force?: boolean }) => Promise<void> | void;
  onSaveError?: (message: string) => void;
  variablesEnabled?: boolean;
  variableAutocompleteMinChars?: number;
  onExitCommand?: () => Promise<void> | void;
  onClipWatchStateChange?: (active: boolean) => void;
  onClipWatchPaste?: (text: string) => void;
  getNoteModules?: () => NoteModules | null;
  setNoteModules?: (modules: NoteModules) => Promise<void> | void;
  onNavigateToNote?: (noteId: string, heading?: string) => void;
}

let currentFormatOnSave = false;
let backendDetached = false;
let tableModuleEnabled = true;

function isLeftArrowKey(key: string): boolean {
  return key === "ArrowLeft" || key === "Left";
}

function isRightArrowKey(key: string): boolean {
  return key === "ArrowRight" || key === "Right";
}

function isLeftArrowEvent(event: KeyboardEvent): boolean {
  return (
    isLeftArrowKey(event.key) ||
    event.code === "ArrowLeft" ||
    event.keyCode === 37
  );
}

function isRightArrowEvent(event: KeyboardEvent): boolean {
  return (
    isRightArrowKey(event.key) ||
    event.code === "ArrowRight" ||
    event.keyCode === 39
  );
}

function tableCellNavigationDomHandler() {
  return Prec.highest(
    EditorView.domEventHandlers({
      keydown: (event, view) => {
        // Do not override Vim normal/visual modes.
        const vimMode = view.dom.dataset.vimMode;
        if (vimMode && vimMode !== "insert") return false;

        const isMod = event.ctrlKey || event.metaKey;
        if (!isMod || event.altKey || event.shiftKey) return false;
        if (!tableModuleEnabled) return false;

        if (isLeftArrowKey(event.key)) {
          event.preventDefault();
          return moveTableCellOrWord(view, true, true);
        }
        if (isRightArrowKey(event.key)) {
          event.preventDefault();
          return moveTableCellOrWord(view, false, false);
        }
        return false;
      },
    }),
  );
}

function snapEditorScrollToPixels() {
  // Most wheel mice report ±3 line deltas per notch. Normalize that to ~1 line
  // while keeping sub-line movement for smaller deltas.
  const normalizeLineDelta = (delta: number): number => {
    const abs = Math.abs(delta);
    if (abs < 0.001) return 0;
    return Math.sign(delta) * Math.max(0.5, abs / 3);
  };

  const clamp = (value: number, min: number, max: number) =>
    Math.min(max, Math.max(min, value));

  const wheelScale = (event: WheelEvent, view: EditorView): number => {
    if (event.deltaMode === WheelEvent.DOM_DELTA_LINE) {
      const rootLineHeight = Number.parseFloat(
        document.documentElement.style.getPropertyValue("--line-height") || "",
      );
      if (Number.isFinite(rootLineHeight) && rootLineHeight > 0) {
        return rootLineHeight;
      }
      const lineHeight = Number.parseFloat(window.getComputedStyle(view.contentDOM).lineHeight || "");
      return Number.isFinite(lineHeight) && lineHeight > 0 ? lineHeight : 24;
    }
    if (event.deltaMode === WheelEvent.DOM_DELTA_PAGE) {
      return view.scrollDOM.clientHeight || 1;
    }
    return 1;
  };

  return Prec.highest(
    EditorView.domEventHandlers({
      wheel: (event, view) => {
        if (event.ctrlKey || event.metaKey) return false;

        const scroller = view.scrollDOM;
        const scale = wheelScale(event, view);
        const lineMode = event.deltaMode === WheelEvent.DOM_DELTA_LINE;
        const deltaY = lineMode ? normalizeLineDelta(event.deltaY) * scale : event.deltaY * scale;
        const deltaX = lineMode ? normalizeLineDelta(event.deltaX) * scale : event.deltaX * scale;
        if (Math.abs(deltaY) < 0.001 && Math.abs(deltaX) < 0.001) return false;

        const maxTop = Math.max(0, scroller.scrollHeight - scroller.clientHeight);
        const maxLeft = Math.max(0, scroller.scrollWidth - scroller.clientWidth);
        const dpr = Math.max(1, window.devicePixelRatio || 1);

        const nextTop = Math.round(clamp(scroller.scrollTop + deltaY, 0, maxTop) * dpr) / dpr;
        const nextLeft = Math.round(clamp(scroller.scrollLeft + deltaX, 0, maxLeft) * dpr) / dpr;
        if (
          Math.abs(nextTop - scroller.scrollTop) < 0.001 &&
          Math.abs(nextLeft - scroller.scrollLeft) < 0.001
        ) {
          return false;
        }

        event.preventDefault();
        scroller.scrollTop = nextTop;
        scroller.scrollLeft = nextLeft;
        return true;
      },
    }),
  );
}

function moveTableCellOrWord(
  view: EditorView,
  tableOutdent: boolean,
  fallbackLeft: boolean,
): boolean {
  if (!tableModuleEnabled) {
    return fallbackLeft ? cursorGroupLeft(view) : cursorGroupRight(view);
  }
  const main = view.state.selection.main;
  if (!main.empty) {
    return fallbackLeft ? cursorGroupLeft(view) : cursorGroupRight(view);
  }
  const line = view.state.doc.lineAt(main.head);
  const trimmed = line.text.trim();
  if (!trimmed.startsWith("|") || !trimmed.endsWith("|")) {
    return fallbackLeft ? cursorGroupLeft(view) : cursorGroupRight(view);
  }
  const moved = runTableCellNavigationCommand(view, {
    markdownAutoformat: true,
    outdent: tableOutdent,
  });
  if (moved) return true;
  return fallbackLeft ? cursorGroupLeft(view) : cursorGroupRight(view);
}

export async function performFormatAndSave(options?: {
  force?: boolean;
  throwOnError?: boolean;
  suppressErrorCallback?: boolean;
}) {
  if (!view) return;
  if (currentFormatOnSave) {
    const { executeCommand } = await import("./command-engine");
    await executeCommand(view, "format", { mode: "editor" });
  }
  await flushSave(true, options?.force ?? false, {
    throwOnError: options?.throwOnError ?? false,
    suppressErrorCallback: options?.suppressErrorCallback ?? false,
  });
}

function applyViewModeClasses(vimMode: boolean, plainTextMode: boolean) {
  if (!view) return;
  if (!vimMode || plainTextMode) {
    view.dom.classList.remove("cm-vim-normal", "cm-vim-insert", "cm-vim-visual");
    delete view.dom.dataset.vimMode;
  }
}

function buildEditorExtensions(options: EditorMountOptions): {
  extensions: Extension[];
  plainTextMode: boolean;
  vimMode: boolean;
} {
  const plainTextMode = !!options.plainTextMode;
  const disableCalc = plainTextMode || !!options.disableCalc;
  const disableMarkdownDecorations = plainTextMode || !!options.disableMarkdownDecorations;
  const disableFolding = plainTextMode || !!options.disableFolding;
  const disableNotify = plainTextMode || !!options.disableNotify;
  const disableAutocomplete = plainTextMode || !!options.disableAutocomplete;
  const tableEnabled = !plainTextMode && (options.tableEnabled ?? true);
  const markdownAutoformat = options.markdownAutoformat ?? true;
  const checklistAutoReorder = options.checklistAutoReorder ?? true;
  const vimMode = !!options.vimMode;

  backendDetached = plainTextMode || !!options.detachBackend;
  currentFormatOnSave = !!options.formatOnSave;
  currentAutosaveEnabled = options.autosave ?? true;
  onSaveError = options.onSaveError ?? null;
  tableModuleEnabled = tableEnabled;

  const extensions = [
    EditorState.allowMultipleSelections.of(true),
    history(),
    drawSelection(),
    highlightActiveLine(),
    placeholder("Start typing..."),
    keymap.of([...defaultKeymap, ...historyKeymap]),
    onUpdate,
    EditorView.contentAttributes.of({ "aria-label": "Note editor" }),
  ];

  if (!plainTextMode) {
    extensions.push(
      ...(disableMarkdownDecorations ? [] : markdownRichTextExtensions()),
      ...(disableFolding ? [] : foldingExtensions()),
      markdownEditingExtensions({
        autoformat: markdownAutoformat,
        checklistAutoReorder,
        tableEnabled,
      }),
      ...(() => {
        if (disableAutocomplete) return [];
        const varSource = makeVariableCompletionSource({
          enabled: options.variablesEnabled ?? true,
          minChars: options.variableAutocompleteMinChars ?? 3,
        });
        const sources = [
          ...(varSource ? [varSource] : []),
          wikiLinkCompletionSource,
        ];
        return [
          autocompletion({
            override: sources,
            activateOnTyping: true,
            closeOnBlur: true,
            defaultKeymap: true,
            maxRenderedOptions: 20,
          }),
          ...wikiLinkExtensions(options.onNavigateToNote),
        ];
      })(),
      ...(disableCalc
        ? []
        : calcExtensions({
            variablesEnabled: options.variablesEnabled ?? true,
            tableEnabled,
            getActiveNoteId: () => state.activeNote?.id ?? null,
          })),
      ...(disableNotify
        ? []
        : notifyExtensions({
            getActiveNoteId: () => state.activeNote?.id ?? null,
          })),
      ...editorSearchExtensions(),
      commandModeExtension({
        dateFormat: options.dateFormat,
        dateTimeFormat: options.dateTimeFormat,
        vimMode: !!options.vimMode,
        onWriteCommand: options.onWriteCommand,
        onExitCommand: options.onExitCommand,
        onClipWatchStateChange: options.onClipWatchStateChange,
        onClipWatchPaste: options.onClipWatchPaste,
        getNoteModules: options.getNoteModules,
        setNoteModules: options.setNoteModules,
      }),
      ...editorContextMenuExtensions({
        dateFormat: options.dateFormat,
        dateTimeFormat: options.dateTimeFormat,
      }),
      imageImportDomHandlers(),
      tableCellNavigationDomHandler(),
      snapEditorScrollToPixels(),
      Prec.highest(keymap.of([
        {
          key: "Ctrl-w",
          run: (view) =>
            tableModuleEnabled ? runTableHeaderDeleteColumnCommand(view) : false,
          preventDefault: true,
        },
        { key: "Ctrl-w", run: deleteGroupBackward, preventDefault: true },
        { key: "Ctrl-ArrowLeft", run: (view) => moveTableCellOrWord(view, true, true), preventDefault: true },
        { key: "Ctrl-ArrowRight", run: (view) => moveTableCellOrWord(view, false, false), preventDefault: true },
        { key: "Mod-ArrowLeft", run: (view) => moveTableCellOrWord(view, true, true), preventDefault: true },
        { key: "Mod-ArrowRight", run: (view) => moveTableCellOrWord(view, false, false), preventDefault: true },
        {
          key: "Ctrl-s",
          run: (view) => {
            const vimMode = view.dom.dataset.vimMode;
            const allowManualSaveWithAutosaveOff = !vimMode || vimMode === "normal";
            if (!currentAutosaveEnabled && !allowManualSaveWithAutosaveOff) return true;
            performFormatAndSave();
            return true;
          },
          preventDefault: true,
        },
      ])),
      EditorView.lineWrapping,
    );
  }

  if (vimMode && !plainTextMode) {
    extensions.push(
      Prec.highest(
        vimModeExtension({
          dateFormat: options.dateFormat,
          dateTimeFormat: options.dateTimeFormat,
          onWriteCommand: options.onWriteCommand,
          onExitCommand: options.onExitCommand,
          onClipWatchStateChange: options.onClipWatchStateChange,
          onClipWatchPaste: options.onClipWatchPaste,
          getNoteModules: options.getNoteModules,
          setNoteModules: options.setNoteModules,
          onNavigateToNote: options.onNavigateToNote,
        }),
      ),
    );
  }

  return { extensions, plainTextMode, vimMode };
}

export function mountEditor(parent: HTMLElement, options: EditorMountOptions = {}) {
  const note = state.activeNote;
  const doc = note?.body ?? "";
  const setup = buildEditorExtensions(options);

  const startState = EditorState.create({ doc, extensions: setup.extensions });
  mountedExtensions = setup.extensions;
  localDirty = false;
  saveInFlight = false;

  view = new EditorView({ state: startState, parent });
  autosaveSnapshots.reset(view.state.doc, doc);
  startupMark("ui_codemirror_ready");
  applyViewModeClasses(setup.vimMode, setup.plainTextMode);
  view.focus();

  if (!setup.plainTextMode) {
    // Capture Ctrl/Meta+Arrow before browser/CM defaults so table navigation is
    // always available when the editor has focus.
    window.addEventListener(
      "keydown",
      (event) => {
        if (!view || !view.hasFocus) return;
        const isMod = event.ctrlKey || event.metaKey;
        if (!isMod || event.altKey || event.shiftKey) return;

        if (isLeftArrowEvent(event)) {
          event.preventDefault();
          event.stopPropagation();
          moveTableCellOrWord(view, true, true);
          return;
        }
        if (isRightArrowEvent(event)) {
          event.preventDefault();
          event.stopPropagation();
          moveTableCellOrWord(view, false, false);
        }
      },
      true,
    );
  }

  window.addEventListener("beforeunload", () => {
    void flushSave();
  });
  document.addEventListener("visibilitychange", () => {
    if (document.visibilityState === "hidden") flushSave();
  });
}

export function reconfigureEditor(options: EditorMountOptions = {}) {
  if (!view) return;
  const setup = buildEditorExtensions(options);
  const main = view.state.selection.main;
  const nextState = EditorState.create({
    doc: view.state.doc,
    extensions: setup.extensions,
    selection: { anchor: main.anchor, head: main.head },
  });
  mountedExtensions = setup.extensions;
  suppressProgrammaticDocSync = true;
  try {
    view.setState(nextState);
  } finally {
    suppressProgrammaticDocSync = false;
  }
  applyViewModeClasses(setup.vimMode, setup.plainTextMode);
}

interface SetEditorContentOptions {
  forceStateReset?: boolean;
}

export function setEditorContent(body: string, options: SetEditorContentOptions = {}) {
  if (!view) return;
  const currentDoc = view.state.doc;
  const shouldResetState =
    !!options.forceStateReset ||
    currentDoc.length >= LARGE_DOC_STATE_RESET_THRESHOLD ||
    body.length >= LARGE_DOC_STATE_RESET_THRESHOLD;

  if (shouldResetState && mountedExtensions.length > 0) {
    const nextState = EditorState.create({ doc: body, extensions: mountedExtensions });
    suppressProgrammaticDocSync = true;
    try {
      view.setState(nextState);
      localDirty = false;
      autosaveSnapshots.reset(view.state.doc, body);
    } finally {
      suppressProgrammaticDocSync = false;
    }
    return;
  }

  if (currentDoc.length === body.length && currentDoc.toString() === body) return;
  view.dispatch({
    changes: { from: 0, to: view.state.doc.length, insert: body },
    annotations: suppressEditorSyncAnnotation.of(true),
  });
  localDirty = false;
  autosaveSnapshots.reset(view.state.doc, body);
}

export function focusEditor() {
  view?.focus();
}

export function jumpEditorToLine(lineNumber: number) {
  if (!view) return;
  const targetLine = Math.max(1, Math.min(view.state.doc.lines, Math.floor(lineNumber)));
  const line = view.state.doc.line(targetLine);
  view.dispatch({
    selection: { anchor: line.from },
    scrollIntoView: true,
  });
  view.focus();
}

export function getEditorView(): EditorView | null {
  return view;
}

export function invalidateEditorWikiLinkCache() {
  invalidateWikiLinkCompletionCaches();
  if (!view) return;
  requestMarkdownDecorationRefresh(view, { invalidateWikiLinkCache: true });
}

export function invalidateEditorWikiLinkForNoteId(noteId: string) {
  const shortId = noteId.slice(0, 8);
  if (!/^[0-9A-Za-z]{8}$/.test(shortId)) return;
  invalidateWikiLinkCompletionCaches(shortId);
  if (!view) return;
  requestMarkdownDecorationRefresh(view, { invalidatedShortIds: [shortId] });
}

export function hasPendingLocalChanges(): boolean {
  return localDirty || saveTimer !== null || saveInFlight;
}

export function insertTextAtCursor(text: string): boolean {
  if (!view) return false;
  const main = view.state.selection.main;
  const from = main.from;
  const to = main.to;
  view.dispatch({
    changes: { from, to, insert: text },
    selection: { anchor: from + text.length },
    scrollIntoView: true,
  });
  return true;
}

export async function insertImagesFromPathsAtCursor(paths: string[]): Promise<boolean> {
  if (!view) return false;
  return insertImagePathsAtCursor(view, paths);
}
