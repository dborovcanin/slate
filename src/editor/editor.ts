import { EditorState } from "@codemirror/state";
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
import { applyEditOperation, snapshotFromView } from "./core/codemirror-adapter";
import { runTableCellNavigationRules } from "./core/text-rules";
import { markdownRichTextExtensions } from "./markdown-decoration";
import { markdownEditingExtensions } from "./markdown-editing";
import { variableAutocompleteExtensions } from "./variable-autocomplete";
import { vimModeExtension } from "./vim";

let view: EditorView | null = null;
let saveTimer: number | null = null;
const SAVE_DEBOUNCE_MS = 500;

function scheduleSave() {
  if (saveTimer !== null) clearTimeout(saveTimer);
  saveTimer = window.setTimeout(flushSave, SAVE_DEBOUNCE_MS);
}

export async function flushSave() {
  if (saveTimer !== null) {
    clearTimeout(saveTimer);
    saveTimer = null;
  }
  const note = state.activeNote;
  if (!note) return;
  try {
    await saveNote(note.id, note.body);
  } catch (e) {
    console.error("Failed to save note:", e);
  }
}

const onUpdate = EditorView.updateListener.of((update) => {
  if (update.docChanged) {
    const body = update.state.doc.toString();
    state.updateBody(body);
    scheduleSave();
  }
});

interface EditorMountOptions {
  markdownAutoformat?: boolean;
  formatOnSave?: boolean;
  vimMode?: boolean;
  dateFormat?: string;
  variablesEnabled?: boolean;
  variableAutocompleteMinChars?: number;
  onExitCommand?: () => Promise<void> | void;
}

let currentFormatOnSave = false;
let currentDateFormat = "%Y-%m-%d";

function moveTableCellOrWord(view: EditorView, outdent: boolean, markdownAutoformat: boolean): boolean {
  if (!markdownAutoformat) {
    return outdent ? cursorGroupLeft(view) : cursorGroupRight(view);
  }
  const operation = runTableCellNavigationRules(snapshotFromView(view), {
    markdownAutoformat,
    outdent,
  });
  if (operation) {
    applyEditOperation(view, operation);
    return true;
  }
  return outdent ? cursorGroupLeft(view) : cursorGroupRight(view);
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
  const markdownAutoformat = options.markdownAutoformat ?? true;
  
  currentFormatOnSave = !!options.formatOnSave;
  currentDateFormat = options.dateFormat || "%Y-%m-%d";

  const extensions = [
    EditorState.allowMultipleSelections.of(true),
    history(),
    drawSelection(),
    highlightActiveLine(),
    placeholder("Start typing..."),
    markdownRichTextExtensions(),
    markdownEditingExtensions({ autoformat: markdownAutoformat }),
    ...variableAutocompleteExtensions({
      enabled: options.variablesEnabled ?? true,
      minChars: options.variableAutocompleteMinChars ?? 3,
    }),
    ...calcExtensions({ variablesEnabled: options.variablesEnabled ?? true }),
    commandModeExtension({
      dateFormat: options.dateFormat,
      vimMode: !!options.vimMode,
      onExitCommand: options.onExitCommand,
    }),
    keymap.of([
      { key: "Ctrl-w", run: deleteGroupBackward },
      { key: "Ctrl-Backspace", run: deleteGroupBackward },
      { key: "Ctrl-ArrowLeft", run: (view) => moveTableCellOrWord(view, true, markdownAutoformat) },
      { key: "Ctrl-ArrowRight", run: (view) => moveTableCellOrWord(view, false, markdownAutoformat) },
      { key: "Ctrl-s", run: () => { performFormatAndSave(); return true; } },
    ]),
    keymap.of([...defaultKeymap, ...historyKeymap]),
    onUpdate,
    EditorView.lineWrapping,
    EditorView.contentAttributes.of({ "aria-label": "Note editor" }),
  ];

  if (options.vimMode) {
    extensions.push(
      vimModeExtension({
        dateFormat: options.dateFormat,
        onExitCommand: options.onExitCommand,
      }),
    );
  }

  const startState = EditorState.create({ doc, extensions });

  view = new EditorView({ state: startState, parent });
  view.focus();

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
