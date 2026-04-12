import {
  cursorCharLeft,
  cursorCharRight,
  cursorGroupBackward,
  cursorGroupForward,
  cursorLineDown,
  cursorLineEnd,
  cursorLineStart,
  cursorLineUp,
  deleteCharForward,
  deleteLine,
  redo,
  undo,
} from "@codemirror/commands";
import { EditorView } from "@codemirror/view";

type VimMode = "insert" | "normal";

function isPrintableTextKey(event: KeyboardEvent): boolean {
  return event.key.length === 1 && !event.metaKey && !event.ctrlKey && !event.altKey;
}

function moveToDocStart(view: EditorView): boolean {
  const first = view.state.doc.line(1).from;
  view.dispatch({ selection: { anchor: first }, scrollIntoView: true });
  return true;
}

function moveToDocEnd(view: EditorView): boolean {
  const last = view.state.doc.line(view.state.doc.lines);
  view.dispatch({ selection: { anchor: last.from }, scrollIntoView: true });
  return true;
}

function insertLineBelow(view: EditorView): boolean {
  const line = view.state.doc.lineAt(view.state.selection.main.head);
  const pos = line.to;
  view.dispatch({
    changes: { from: pos, to: pos, insert: "\n" },
    selection: { anchor: pos + 1 },
    scrollIntoView: true,
  });
  return true;
}

function insertLineAbove(view: EditorView): boolean {
  const line = view.state.doc.lineAt(view.state.selection.main.head);
  const pos = line.from;
  view.dispatch({
    changes: { from: pos, to: pos, insert: "\n" },
    selection: { anchor: pos },
    scrollIntoView: true,
  });
  return true;
}

export function vimModeExtension() {
  let mode: VimMode = "insert";
  let pendingDelete = false;
  let pendingGo = false;
  let countBuffer = "";

  const clearPending = () => {
    pendingDelete = false;
    pendingGo = false;
  };

  const hasCount = () => countBuffer.length > 0;
  const consumeCount = () => {
    const count = countBuffer.length > 0 ? parseInt(countBuffer, 10) : 1;
    countBuffer = "";
    return Number.isFinite(count) && count > 0 ? count : 1;
  };
  const appendCountDigit = (digit: string) => {
    countBuffer += digit;
  };
  const runCounted = (
    view: EditorView,
    command: (target: EditorView) => boolean,
    explicitCount?: number,
  ) => {
    const count = explicitCount ?? consumeCount();
    for (let i = 0; i < count; i++) {
      const ok = command(view);
      if (!ok) break;
    }
    return true;
  };

  const setMode = (view: EditorView, next: VimMode) => {
    mode = next;
    clearPending();
    countBuffer = "";
    view.dom.classList.toggle("cm-vim-normal", next === "normal");
    view.dom.classList.toggle("cm-vim-insert", next === "insert");
    view.dom.dataset.vimMode = next;
  };

  return EditorView.domEventHandlers({
    focus: (_event, view) => {
      setMode(view, mode);
      return false;
    },
    keydown: (event, view) => {
      if (mode === "insert") {
        if (event.key === "Escape") {
          event.preventDefault();
          setMode(view, "normal");
          return true;
        }
        return false;
      }

      // Normal mode
      if (event.key === "Escape") {
        event.preventDefault();
        clearPending();
        countBuffer = "";
        return true;
      }

      if (
        event.key.length === 1 &&
        event.key >= "0" &&
        event.key <= "9" &&
        !event.ctrlKey &&
        !event.altKey &&
        !event.metaKey
      ) {
        if (event.key === "0" && !hasCount() && !pendingDelete && !pendingGo) {
          event.preventDefault();
          return runCounted(view, cursorLineStart);
        }

        event.preventDefault();
        appendCountDigit(event.key);
        return true;
      }

      if (pendingDelete) {
        clearPending();
        if (event.key === "d") {
          event.preventDefault();
          return runCounted(view, deleteLine);
        }
      }

      if (pendingGo) {
        clearPending();
        if (event.key === "g") {
          event.preventDefault();
          const count = consumeCount();
          if (count > 1) {
            const lineNo = Math.min(count, view.state.doc.lines);
            const pos = view.state.doc.line(lineNo).from;
            view.dispatch({ selection: { anchor: pos }, scrollIntoView: true });
            return true;
          }
          return moveToDocStart(view);
        }
      }

      if (event.ctrlKey && event.key.toLowerCase() === "r") {
        event.preventDefault();
        return runCounted(view, redo);
      }

      switch (event.key) {
        case "i":
          event.preventDefault();
          setMode(view, "insert");
          return true;
        case "a":
          event.preventDefault();
          cursorCharRight(view);
          setMode(view, "insert");
          return true;
        case "I":
          event.preventDefault();
          cursorLineStart(view);
          setMode(view, "insert");
          return true;
        case "A":
          event.preventDefault();
          cursorLineEnd(view);
          setMode(view, "insert");
          return true;
        case "o":
          event.preventDefault();
          insertLineBelow(view);
          setMode(view, "insert");
          return true;
        case "O":
          event.preventDefault();
          insertLineAbove(view);
          setMode(view, "insert");
          return true;
        case "h":
          event.preventDefault();
          return runCounted(view, cursorCharLeft);
        case "j":
          event.preventDefault();
          return runCounted(view, cursorLineDown);
        case "k":
          event.preventDefault();
          return runCounted(view, cursorLineUp);
        case "l":
          event.preventDefault();
          return runCounted(view, cursorCharRight);
        case "w":
          event.preventDefault();
          return runCounted(view, cursorGroupForward);
        case "b":
          event.preventDefault();
          return runCounted(view, cursorGroupBackward);
        case "0":
          event.preventDefault();
          return runCounted(view, cursorLineStart);
        case "$":
          event.preventDefault();
          return runCounted(view, cursorLineEnd);
        case "x":
          event.preventDefault();
          return runCounted(view, deleteCharForward);
        case "u":
          event.preventDefault();
          return runCounted(view, undo);
        case "d":
          event.preventDefault();
          pendingDelete = true;
          return true;
        case "g":
          event.preventDefault();
          pendingGo = true;
          return true;
        case "G":
          event.preventDefault();
          if (hasCount()) {
            const count = consumeCount();
            const lineNo = Math.min(count, view.state.doc.lines);
            const pos = view.state.doc.line(lineNo).from;
            view.dispatch({ selection: { anchor: pos }, scrollIntoView: true });
            return true;
          }
          return moveToDocEnd(view);
      }

      if (isPrintableTextKey(event)) {
        event.preventDefault();
        return true;
      }

      if (
        event.key === "Backspace" ||
        event.key === "Delete" ||
        event.key === "Enter" ||
        event.key === "Tab"
      ) {
        event.preventDefault();
        return true;
      }

      return false;
    },
  });
}
