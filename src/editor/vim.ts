import { EditorSelection } from "@codemirror/state";
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
  selectCharLeft,
  selectCharRight,
  selectGroupBackward,
  selectGroupForward,
  selectLineDown,
  selectLineEnd,
  selectLineStart,
  selectLineUp,
  undo,
} from "@codemirror/commands";
import { EditorView } from "@codemirror/view";
import { isCommandPickerOpen, openCommandPicker } from "./command-picker";
import { computeBlockSpans } from "./vim-utils";

type VimMode = "insert" | "normal" | "visual" | "visual-line" | "visual-block";
type PendingTextObject = { op: "delete" | "yank"; around: boolean } | null;

function isPrintableTextKey(event: KeyboardEvent): boolean {
  return event.key.length === 1 && !event.metaKey && !event.ctrlKey && !event.altKey;
}

function isLineStartMotionKey(event: KeyboardEvent): boolean {
  return event.key === "0" || event.key === "Home";
}

function isLineEndMotionKey(event: KeyboardEvent): boolean {
  return event.key === "$" || event.key === "End" || (event.shiftKey && event.code === "Digit4");
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

function copyToClipboard(text: string) {
  if (!navigator.clipboard) return;
  void navigator.clipboard.writeText(text).catch((err) => {
    console.error("Vim yank failed:", err);
  });
}

interface VimOptions {
  dateFormat?: string;
  onExitCommand?: () => Promise<void> | void;
}

export function vimModeExtension(options: VimOptions = {}) {
  let mode: VimMode = "insert";
  let pendingDelete = false;
  let pendingGo = false;
  let pendingYank = false;
  let pendingTextObject: PendingTextObject = null;
  let countBuffer = "";
  let visualAnchorPos: number | null = null;
  let visualAnchorLine: number | null = null; // 1-based
  let blockAnchor: { line: number; col: number } | null = null; // line: 1-based

  const clearPending = () => {
    pendingDelete = false;
    pendingGo = false;
    pendingYank = false;
    pendingTextObject = null;
  };

  const resetVisualAnchors = () => {
    visualAnchorPos = null;
    visualAnchorLine = null;
    blockAnchor = null;
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

  const updateModeClasses = (view: EditorView, nextMode: VimMode) => {
    view.dom.classList.toggle("cm-vim-normal", nextMode === "normal");
    view.dom.classList.toggle("cm-vim-insert", nextMode === "insert");
    view.dom.classList.toggle(
      "cm-vim-visual",
      nextMode === "visual" || nextMode === "visual-line" || nextMode === "visual-block",
    );
    view.dom.dataset.vimMode = nextMode;
  };

  const collapseSelection = (view: EditorView) => {
    const head = view.state.selection.main.head;
    view.dispatch({ selection: { anchor: head }, scrollIntoView: true });
  };

  const setMode = (view: EditorView, next: VimMode) => {
    const prev = mode;
    mode = next;
    clearPending();
    countBuffer = "";

    if (next === "normal") {
      resetVisualAnchors();
      if (prev === "visual" || prev === "visual-line" || prev === "visual-block") {
        collapseSelection(view);
      }
    }

    updateModeClasses(view, next);
  };

  const getHeadInfo = (view: EditorView) => {
    const head = view.state.selection.main.head;
    const line = view.state.doc.lineAt(head);
    return {
      head,
      lineNumber: line.number, // 1-based
      col: head - line.from,
      line,
    };
  };

  const applyVisualCharSelection = (view: EditorView) => {
    if (visualAnchorPos === null) return;
    const head = view.state.selection.main.head;
    view.dispatch({
      selection: { anchor: visualAnchorPos, head },
      scrollIntoView: true,
    });
  };

  const applyVisualLineSelection = (view: EditorView) => {
    if (visualAnchorLine === null) return;
    const { lineNumber } = getHeadInfo(view);
    const startLine = Math.min(visualAnchorLine, lineNumber);
    const endLine = Math.max(visualAnchorLine, lineNumber);
    const from = view.state.doc.line(startLine).from;
    const to = view.state.doc.line(endLine).to;
    view.dispatch({
      selection: { anchor: from, head: to },
      scrollIntoView: true,
    });
  };

  const applyVisualBlockSelection = (view: EditorView) => {
    if (!blockAnchor) return;
    const headInfo = getHeadInfo(view);
    const lineLengths: number[] = [];
    for (let n = 1; n <= view.state.doc.lines; n++) {
      lineLengths.push(view.state.doc.line(n).length);
    }

    const spans = computeBlockSpans(
      lineLengths,
      blockAnchor.line - 1,
      blockAnchor.col,
      headInfo.lineNumber - 1,
      headInfo.col,
    );
    if (spans.length === 0) return;

    const ranges = spans.map((span) => {
      const line = view.state.doc.line(span.lineIndex + 1);
      return EditorSelection.range(line.from + span.fromCol, line.from + span.toCol);
    });

    const startLine = Math.min(blockAnchor.line, headInfo.lineNumber) - 1;
    const mainIndex = Math.min(Math.max(headInfo.lineNumber - 1 - startLine, 0), ranges.length - 1);
    view.dispatch({
      selection: EditorSelection.create(ranges, mainIndex),
      scrollIntoView: true,
    });
  };

  const updateVisualSelection = (view: EditorView) => {
    if (mode === "visual") {
      applyVisualCharSelection(view);
      return;
    }
    if (mode === "visual-line") {
      applyVisualLineSelection(view);
      return;
    }
    if (mode === "visual-block") {
      applyVisualBlockSelection(view);
    }
  };

  const startVisualMode = (view: EditorView, next: "visual" | "visual-line" | "visual-block") => {
    const info = getHeadInfo(view);
    mode = next;
    clearPending();
    countBuffer = "";

    if (next === "visual") {
      visualAnchorPos = info.head;
      visualAnchorLine = null;
      blockAnchor = null;
    } else if (next === "visual-line") {
      visualAnchorLine = info.lineNumber;
      visualAnchorPos = null;
      blockAnchor = null;
    } else {
      blockAnchor = { line: info.lineNumber, col: info.col };
      visualAnchorPos = null;
      visualAnchorLine = null;
    }

    updateModeClasses(view, next);
    updateVisualSelection(view);
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

  const selectVariantFor = (command: (view: EditorView) => boolean) => {
    if (command === cursorCharLeft) return selectCharLeft;
    if (command === cursorCharRight) return selectCharRight;
    if (command === cursorLineUp) return selectLineUp;
    if (command === cursorLineDown) return selectLineDown;
    if (command === cursorGroupForward) return selectGroupForward;
    if (command === cursorGroupBackward) return selectGroupBackward;
    if (command === cursorLineStart) return selectLineStart;
    if (command === cursorLineEnd) return selectLineEnd;
    return command;
  };

  const runMove = (
    view: EditorView,
    command: (target: EditorView) => boolean,
    explicitCount?: number,
  ) => {
    const isVisual = mode === "visual" || mode === "visual-line" || mode === "visual-block";
    const cmd = isVisual ? selectVariantFor(command) : command;
    runCounted(view, cmd, explicitCount);
    if (isVisual) {
      updateVisualSelection(view);
    }
    return true;
  };

  const isWordChar = (char: string) => /[A-Za-z0-9_]/.test(char);

  const findWordObjectRange = (
    view: EditorView,
    around: boolean,
  ): { from: number; to: number } | null => {
    const main = view.state.selection.main;
    const line = view.state.doc.lineAt(main.head);
    const text = line.text;
    const len = text.length;
    if (len === 0) return null;

    let rel = Math.max(0, Math.min(main.head - line.from, len));
    if (rel >= len) rel = len - 1;

    if (!isWordChar(text[rel] ?? "")) {
      if (rel > 0 && isWordChar(text[rel - 1] ?? "")) {
        rel -= 1;
      } else {
        while (rel < len && !isWordChar(text[rel] ?? "")) {
          rel += 1;
        }
        if (rel >= len) return null;
      }
    }

    let start = rel;
    while (start > 0 && isWordChar(text[start - 1] ?? "")) {
      start -= 1;
    }
    let end = rel + 1;
    while (end < len && isWordChar(text[end] ?? "")) {
      end += 1;
    }

    if (around) {
      let aroundStart = start;
      let aroundEnd = end;
      while (aroundEnd < len && /\s/.test(text[aroundEnd] ?? "")) {
        aroundEnd += 1;
      }
      if (aroundEnd === end) {
        while (aroundStart > 0 && /\s/.test(text[aroundStart - 1] ?? "")) {
          aroundStart -= 1;
        }
      }
      start = aroundStart;
      end = aroundEnd;
    }

    if (start >= end) return null;
    return { from: line.from + start, to: line.from + end };
  };

  const findPipeObjectRange = (
    view: EditorView,
    around: boolean,
  ): { from: number; to: number } | null => {
    const main = view.state.selection.main;
    const line = view.state.doc.lineAt(main.head);
    const text = line.text;
    if (text.length < 2) return null;

    const pipes: number[] = [];
    for (let i = 0; i < text.length; i++) {
      if (text[i] === "|") {
        pipes.push(i);
      }
    }
    if (pipes.length < 2) return null;

    const rel = Math.max(0, Math.min(main.head - line.from, text.length));
    let pair: [number, number] | null = null;
    for (let i = 0; i < pipes.length - 1; i++) {
      const left = pipes[i]!;
      const right = pipes[i + 1]!;
      if (rel === left || (rel > left && rel <= right)) {
        pair = [left, right];
        break;
      }
    }
    if (!pair) return null;

    const start = around ? pair[0] : pair[0] + 1;
    const end = around ? pair[1] + 1 : pair[1];
    if (start >= end) return null;
    return { from: line.from + start, to: line.from + end };
  };

  const runPendingTextObject = (
    view: EditorView,
    pending: NonNullable<PendingTextObject>,
    objectKey: string,
  ) => {
    const count = consumeCount();
    const chunks: string[] = [];
    for (let i = 0; i < count; i++) {
      const range =
        objectKey === "w"
          ? findWordObjectRange(view, pending.around)
          : objectKey === "|"
            ? findPipeObjectRange(view, pending.around)
            : null;
      if (!range) break;

      const text = view.state.sliceDoc(range.from, range.to);
      if (!text) break;
      chunks.push(text);

      if (pending.op === "delete") {
        view.dispatch({
          changes: { from: range.from, to: range.to, insert: "" },
          selection: { anchor: range.from },
          scrollIntoView: true,
        });
      } else {
        // Advance to make counted text-object yanks progress.
        view.dispatch({
          selection: { anchor: range.to },
          scrollIntoView: true,
        });
      }
    }

    if (chunks.length > 0) {
      copyToClipboard(chunks.join("\n"));
    }
    return true;
  };

  const deleteRange = (view: EditorView, from: number, to: number) => {
    if (to <= from) return true;
    view.dispatch({
      changes: { from, to, insert: "" },
      selection: { anchor: from },
      scrollIntoView: true,
    });
    return true;
  };

  const yankRange = (view: EditorView, from: number, to: number) => {
    if (to <= from) return true;
    copyToClipboard(view.state.sliceDoc(from, to));
    return true;
  };

  const deleteToLineStart = (view: EditorView) => {
    const head = view.state.selection.main.head;
    const line = view.state.doc.lineAt(head);
    return deleteRange(view, line.from, head);
  };

  const deleteToLineEnd = (view: EditorView) => {
    const head = view.state.selection.main.head;
    const line = view.state.doc.lineAt(head);
    return deleteRange(view, head, line.to);
  };

  const yankToLineStart = (view: EditorView) => {
    const head = view.state.selection.main.head;
    const line = view.state.doc.lineAt(head);
    return yankRange(view, line.from, head);
  };

  const yankToLineEnd = (view: EditorView) => {
    const head = view.state.selection.main.head;
    const line = view.state.doc.lineAt(head);
    return yankRange(view, head, line.to);
  };

  const yankVisualSelection = (view: EditorView) => {
    if (mode === "visual-block") {
      const parts = [...view.state.selection.ranges]
        .sort((a, b) => a.from - b.from)
        .map((range) => view.state.sliceDoc(range.from, range.to));
      copyToClipboard(parts.join("\n"));
      setMode(view, "normal");
      return true;
    }

    if (mode === "visual-line") {
      const main = view.state.selection.main;
      const start = view.state.doc.lineAt(main.from).number;
      const end = view.state.doc.lineAt(main.to).number;
      const parts: string[] = [];
      for (let lineNo = start; lineNo <= end; lineNo++) {
        parts.push(view.state.doc.line(lineNo).text);
      }
      copyToClipboard(parts.join("\n"));
      setMode(view, "normal");
      return true;
    }

    const text = view.state.sliceDoc(view.state.selection.main.from, view.state.selection.main.to);
    copyToClipboard(text);
    setMode(view, "normal");
    return true;
  };

  const yankCurrentLines = (view: EditorView) => {
    const count = consumeCount();
    const current = view.state.doc.lineAt(view.state.selection.main.head).number;
    const end = Math.min(view.state.doc.lines, current + count - 1);
    const parts: string[] = [];
    for (let lineNo = current; lineNo <= end; lineNo++) {
      parts.push(view.state.doc.line(lineNo).text);
    }
    copyToClipboard(parts.join("\n"));
    return true;
  };

  return EditorView.domEventHandlers({
    focus: (_event, view) => {
      updateModeClasses(view, mode);
      return false;
    },
    keydown: (event, view) => {
      if (isCommandPickerOpen(view)) {
        return false;
      }

      const plainV = !event.altKey && !event.metaKey && !event.shiftKey && event.key === "v";
      const ctrlV =
        event.ctrlKey && !event.altKey && !event.metaKey && event.key.toLowerCase() === "v";

      if (mode === "insert") {
        if (event.key === "Escape") {
          event.preventDefault();
          setMode(view, "normal");
          return true;
        }
        return false;
      }

      if (event.key === "Escape") {
        event.preventDefault();
        setMode(view, "normal");
        return true;
      }

      if (event.key === ":" && !event.ctrlKey && !event.altKey && !event.metaKey) {
        event.preventDefault();
        setMode(view, "normal");
        openCommandPicker(view, {
          mode: "vim",
          dateFormat: options.dateFormat,
          onExitCommand: options.onExitCommand,
          source: "vim-colon",
        });
        return true;
      }

      // Visual mode toggles
      if (ctrlV) {
        event.preventDefault();
        if (mode === "visual-block") {
          setMode(view, "normal");
        } else {
          startVisualMode(view, "visual-block");
        }
        return true;
      }

      if (event.key === "V" && !event.ctrlKey && !event.altKey && !event.metaKey) {
        event.preventDefault();
        if (mode === "visual-line") {
          setMode(view, "normal");
        } else {
          startVisualMode(view, "visual-line");
        }
        return true;
      }

      if (plainV) {
        event.preventDefault();
        if (mode === "visual") {
          setMode(view, "normal");
        } else {
          startVisualMode(view, "visual");
        }
        return true;
      }

      // Numeric count prefix
      if (
        event.key.length === 1 &&
        event.key >= "0" &&
        event.key <= "9" &&
        !event.shiftKey &&
        !event.ctrlKey &&
        !event.altKey &&
        !event.metaKey
      ) {
        if (event.key === "0" && !hasCount() && !pendingDelete && !pendingGo && !pendingYank) {
          event.preventDefault();
          return runMove(view, cursorLineStart, 1);
        }

        event.preventDefault();
        appendCountDigit(event.key);
        return true;
      }

      // Pending operators
      if (pendingTextObject) {
        event.preventDefault();
        const pending = pendingTextObject;
        clearPending();
        return runPendingTextObject(view, pending, event.key);
      }

      if (pendingDelete) {
        event.preventDefault();
        if (event.key === "i" || event.key === "a") {
          pendingDelete = false;
          pendingTextObject = { op: "delete", around: event.key === "a" };
          return true;
        }
        if (isLineStartMotionKey(event)) {
          clearPending();
          return deleteToLineStart(view);
        }
        if (isLineEndMotionKey(event)) {
          clearPending();
          return deleteToLineEnd(view);
        }
        if (event.key === "d") {
          clearPending();
          return runCounted(view, deleteLine);
        }
        clearPending();
        return true;
      }

      if (pendingYank) {
        event.preventDefault();
        if (event.key === "i" || event.key === "a") {
          pendingYank = false;
          pendingTextObject = { op: "yank", around: event.key === "a" };
          return true;
        }
        if (isLineStartMotionKey(event)) {
          clearPending();
          return yankToLineStart(view);
        }
        if (isLineEndMotionKey(event)) {
          clearPending();
          return yankToLineEnd(view);
        }
        if (event.key === "y") {
          clearPending();
          return yankCurrentLines(view);
        }
        clearPending();
        return true;
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
            if (mode === "visual-block") {
              updateVisualSelection(view);
            }
            return true;
          }
          const ok = moveToDocStart(view);
          if (mode === "visual-block") {
            updateVisualSelection(view);
          }
          return ok;
        }
      }

      if (event.ctrlKey && event.key.toLowerCase() === "r") {
        event.preventDefault();
        return runCounted(view, redo);
      }

      if ((mode === "visual" || mode === "visual-line" || mode === "visual-block") && event.key === "y") {
        event.preventDefault();
        return yankVisualSelection(view);
      }

      if (!event.ctrlKey && !event.altKey && !event.metaKey && isLineEndMotionKey(event)) {
        event.preventDefault();
        return runMove(view, cursorLineEnd);
      }

      if (
        !event.ctrlKey &&
        !event.altKey &&
        !event.metaKey &&
        event.key === "Home" &&
        !pendingDelete &&
        !pendingYank &&
        !pendingGo
      ) {
        event.preventDefault();
        return runMove(view, cursorLineStart);
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
          return runMove(view, cursorCharLeft);
        case "j":
          event.preventDefault();
          return runMove(view, cursorLineDown);
        case "k":
          event.preventDefault();
          return runMove(view, cursorLineUp);
        case "l":
          event.preventDefault();
          return runMove(view, cursorCharRight);
        case "w":
          event.preventDefault();
          return runMove(view, cursorGroupForward);
        case "b":
          event.preventDefault();
          return runMove(view, cursorGroupBackward);
        case "0":
          event.preventDefault();
          return runMove(view, cursorLineStart);
        case "x":
          event.preventDefault();
          return runMove(view, deleteCharForward);
        case "u":
          event.preventDefault();
          return runCounted(view, undo);
        case "d":
          event.preventDefault();
          pendingDelete = true;
          return true;
        case "y":
          event.preventDefault();
          if (mode === "normal") {
            pendingYank = true;
            return true;
          }
          return false;
        case "g":
          event.preventDefault();
          pendingGo = true;
          return true;
        case "G":
          event.preventDefault();
          if (hasCount()) {
            const lineNo = Math.min(consumeCount(), view.state.doc.lines);
            const pos = view.state.doc.line(lineNo).from;
            view.dispatch({ selection: { anchor: pos }, scrollIntoView: true });
            if (mode === "visual-block") {
              updateVisualSelection(view);
            }
            return true;
          }
          const ok = moveToDocEnd(view);
          if (mode === "visual-block") {
            updateVisualSelection(view);
          }
          return ok;
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

      // Intercept arrow keys in visual modes to keep extending the selection
      if (mode === "visual" || mode === "visual-line" || mode === "visual-block") {
        const arrowMap: Record<string, (view: EditorView) => boolean> = {
          ArrowLeft: cursorCharLeft,
          ArrowRight: cursorCharRight,
          ArrowUp: cursorLineUp,
          ArrowDown: cursorLineDown,
        };
        const cmd = arrowMap[event.key];
        if (cmd) {
          event.preventDefault();
          return runMove(view, cmd);
        }
      }

      return false;
    },
  });
}
