import { EditorState } from "@codemirror/state";
import {
  EditorView,
  keymap,
  drawSelection,
  highlightActiveLine,
  placeholder,
} from "@codemirror/view";
import { defaultKeymap, history, historyKeymap } from "@codemirror/commands";
import { state } from "../state";
import { saveNote } from "../api";
import { calcExtensions } from "./calc-decoration";

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

export function mountEditor(parent: HTMLElement) {
  const note = state.activeNote;
  const doc = note?.body ?? "";

  const extensions = [
    history(),
    drawSelection(),
    highlightActiveLine(),
    placeholder("Start typing..."),
    calcExtensions(),
    keymap.of([...defaultKeymap, ...historyKeymap]),
    onUpdate,
    EditorView.lineWrapping,
    EditorView.contentAttributes.of({ "aria-label": "Note editor" }),
  ];

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
