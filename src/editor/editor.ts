import { Annotation, EditorState, Prec, type Text } from "@codemirror/state";
import {
  EditorView,
  keymap,
  drawSelection,
  highlightActiveLine,
  placeholder,
} from "@codemirror/view";
import { defaultKeymap, deleteGroupBackward, history, historyKeymap, cursorGroupLeft, cursorGroupRight } from "@codemirror/commands";
import { state } from "../state";
import { saveNote } from "../api";
import { calcExtensions } from "./calc-decoration";
import { commandModeExtension } from "./command-picker";
import { markdownRichTextExtensions } from "./markdown-decoration";
import {
  markdownEditingExtensions,
  runTableCellNavigationCommand,
  runTableHeaderDeleteColumnCommand,
} from "./markdown-editing";
import { foldingExtensions } from "./folding.ts";
import { notifyExtensions } from "./notify-decoration";
import { variableAutocompleteExtensions } from "./variable-autocomplete";
import { editorSearchExtensions } from "./search";
import { vimModeExtension } from "./vim";
import { startupMark } from "../perf/startup.ts";

let view: EditorView | null = null;
let saveTimer: number | null = null;
const SAVE_DEBOUNCE_MS = 500;
const TITLE_PREVIEW_LIMIT = 60;
const suppressEditorSyncAnnotation = Annotation.define<boolean>();

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
  if (saveTimer !== null) clearTimeout(saveTimer);
  saveTimer = window.setTimeout(flushSave, SAVE_DEBOUNCE_MS);
}

export async function flushSave() {
  if (saveTimer !== null) {
    clearTimeout(saveTimer);
    saveTimer = null;
  }
  if (backendDetached) return;
  const note = state.activeNote;
  if (!note) return;
  const body = view ? view.state.doc.toString() : note.body;
  try {
    await saveNote(note.id, body);
    state.updateBody(body);
  } catch (e) {
    console.error("Failed to save note:", e);
  }
}

const onUpdate = EditorView.updateListener.of((update) => {
  if (update.docChanged) {
    if (backendDetached) return;
    if (
      update.transactions.some((transaction) =>
        transaction.annotation(suppressEditorSyncAnnotation),
      )
    ) {
      return;
    }
    state.updateDraftTitle(deriveTitleFromDoc(update.state.doc));
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
  markdownAutoformat?: boolean;
  checklistAutoReorder?: boolean;
  formatOnSave?: boolean;
  vimMode?: boolean;
  dateFormat?: string;
  dateTimeFormat?: string;
  variablesEnabled?: boolean;
  variableAutocompleteMinChars?: number;
  onExitCommand?: () => Promise<void> | void;
  onClipWatchStateChange?: (active: boolean) => void;
  onClipWatchPaste?: (text: string) => void;
}

let currentFormatOnSave = false;
let currentDateFormat = "%Y-%m-%d";
let backendDetached = false;

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

export async function performFormatAndSave() {
  if (!view) return;
  if (currentFormatOnSave) {
    const { executeCommand } = await import("./command-engine");
    await executeCommand(view, "format", { mode: "editor" });
  }
  await flushSave();
}

export function mountEditor(parent: HTMLElement, options: EditorMountOptions = {}) {
  const note = state.activeNote;
  const doc = note?.body ?? "";
  const plainTextMode = !!options.plainTextMode;
  const disableCalc = plainTextMode || !!options.disableCalc;
  const disableMarkdownDecorations = plainTextMode || !!options.disableMarkdownDecorations;
  const disableFolding = plainTextMode || !!options.disableFolding;
  const disableNotify = plainTextMode || !!options.disableNotify;
  const disableAutocomplete = plainTextMode || !!options.disableAutocomplete;
  const markdownAutoformat = options.markdownAutoformat ?? true;
  const checklistAutoReorder = options.checklistAutoReorder ?? true;

  backendDetached = plainTextMode || !!options.detachBackend;
  currentFormatOnSave = !!options.formatOnSave;
  currentDateFormat = options.dateFormat || "%Y-%m-%d";

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
      }),
      ...variableAutocompleteExtensions({
        enabled: !disableAutocomplete && (options.variablesEnabled ?? true),
        minChars: options.variableAutocompleteMinChars ?? 3,
      }),
      ...(disableCalc
        ? []
        : calcExtensions({ variablesEnabled: options.variablesEnabled ?? true })),
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
        onExitCommand: options.onExitCommand,
        onClipWatchStateChange: options.onClipWatchStateChange,
        onClipWatchPaste: options.onClipWatchPaste,
      }),
      tableCellNavigationDomHandler(),
      snapEditorScrollToPixels(),
      Prec.highest(keymap.of([
        {
          key: "Ctrl-w",
          run: runTableHeaderDeleteColumnCommand,
          preventDefault: true,
        },
        { key: "Ctrl-w", run: deleteGroupBackward, preventDefault: true },
        { key: "Ctrl-ArrowLeft", run: (view) => moveTableCellOrWord(view, true, true), preventDefault: true },
        { key: "Ctrl-ArrowRight", run: (view) => moveTableCellOrWord(view, false, false), preventDefault: true },
        { key: "Mod-ArrowLeft", run: (view) => moveTableCellOrWord(view, true, true), preventDefault: true },
        { key: "Mod-ArrowRight", run: (view) => moveTableCellOrWord(view, false, false), preventDefault: true },
        {
          key: "Ctrl-s",
          run: () => {
            performFormatAndSave();
            return true;
          },
          preventDefault: true,
        },
      ])),
      EditorView.lineWrapping,
    );
  }

  if (options.vimMode && !plainTextMode) {
    extensions.push(
      Prec.highest(
        vimModeExtension({
          dateFormat: options.dateFormat,
          dateTimeFormat: options.dateTimeFormat,
          onExitCommand: options.onExitCommand,
          onClipWatchStateChange: options.onClipWatchStateChange,
          onClipWatchPaste: options.onClipWatchPaste,
        }),
      ),
    );
  }

  const startState = EditorState.create({ doc, extensions });

  view = new EditorView({ state: startState, parent });
  startupMark("ui_codemirror_ready");
  if (!options.vimMode || plainTextMode) {
    view.dom.classList.remove("cm-vim-normal", "cm-vim-insert", "cm-vim-visual");
    delete view.dom.dataset.vimMode;
  }
  view.focus();

  if (!plainTextMode) {
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

  window.addEventListener("beforeunload", flushSave);
  document.addEventListener("visibilitychange", () => {
    if (document.visibilityState === "hidden") flushSave();
  });
}

export function setEditorContent(body: string) {
  if (!view) return;
  const current = view.state.doc.toString();
  if (current === body) return;
  view.dispatch({
    changes: { from: 0, to: view.state.doc.length, insert: body },
    annotations: suppressEditorSyncAnnotation.of(true),
  });
}

export function focusEditor() {
  view?.focus();
}

export function getEditorView(): EditorView | null {
  return view;
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
