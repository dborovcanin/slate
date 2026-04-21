import {
  cursorCharLeft,
  cursorCharRight,
  cursorLineDown,
  cursorLineEnd,
  cursorLineStart,
  cursorLineUp,
  deleteCharForward,
  redo,
  undo,
} from "@codemirror/commands";
import { EditorView } from "@codemirror/view";
import { isCommandPickerOpen, openCommandPicker } from "./command-picker";
import type { NoteModules } from "../api.ts";
import { toggleFoldAtCursor } from "./folding.ts";
import {
  editorSearchHasMatches,
  editorSearchNext,
  editorSearchPrev,
  isEditorSearchOverlayTarget,
  openEditorSearch,
} from "./search";
import {
  VimSession,
  VIM_KEY_KIND,
  type VimAction,
  type VimKeyInput,
  type VimMode,
} from "./wasm.ts";
import { vimAppendInsertPos, vimNormalLineEndPos } from "./vim-utils.ts";

type VimUiMode = "insert" | "normal" | "visual" | "visual-line";

interface VimOptions {
  dateFormat?: string;
  dateTimeFormat?: string;
  onExitCommand?: () => Promise<void> | void;
  onClipWatchStateChange?: (active: boolean) => void;
  onClipWatchPaste?: (text: string) => void;
  getNoteModules?: () => NoteModules | null;
  setNoteModules?: (modules: NoteModules) => Promise<void> | void;
}

type VimRegisterMode = "charwise" | "linewise";

interface VimRegister {
  text: string;
  mode: VimRegisterMode;
}

function isPrintableTextKey(event: KeyboardEvent): boolean {
  return event.key.length === 1 && !event.metaKey && !event.ctrlKey && !event.altKey;
}

function toUiMode(mode: VimMode): VimUiMode {
  return mode === "visual_line" ? "visual-line" : mode;
}

function updateModeClasses(view: EditorView, mode: VimUiMode) {
  view.dom.classList.toggle("cm-vim-normal", mode === "normal");
  view.dom.classList.toggle("cm-vim-insert", mode === "insert");
  view.dom.classList.toggle("cm-vim-visual", mode === "visual" || mode === "visual-line");
  view.dom.dataset.vimMode = mode;
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

function moveToLineEndNormalLike(view: EditorView): boolean {
  const head = view.state.selection.main.head;
  const line = view.state.doc.lineAt(head);
  const anchor = vimNormalLineEndPos(line.from, line.to);
  view.dispatch({ selection: { anchor }, scrollIntoView: true });
  return true;
}

function appendInsertWithinLine(view: EditorView): boolean {
  const head = view.state.selection.main.head;
  const line = view.state.doc.lineAt(head);
  const anchor = vimAppendInsertPos(head, line.to);
  view.dispatch({ selection: { anchor }, scrollIntoView: true });
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

function getCharClass(char: string): number {
  if (!char || /^\s$/.test(char)) return 0;
  if (/[A-Za-z0-9_]/.test(char)) return 1;
  return 2;
}

function vimMoveWordForward(view: EditorView): boolean {
  const head = view.state.selection.main.head;
  const line = view.state.doc.lineAt(head);
  const text = line.text;
  const len = text.length;
  let col = head - line.from;

  if (col >= len) {
    if (line.number < view.state.doc.lines) {
      view.dispatch({ selection: { anchor: view.state.doc.line(line.number + 1).from }, scrollIntoView: true });
    }
    return true;
  }

  const startClass = getCharClass(text[col]);
  while (col < len && getCharClass(text[col]) === startClass) {
    col++;
  }
  if (startClass !== 0) {
    while (col < len && getCharClass(text[col]) === 0) {
      col++;
    }
  }

  view.dispatch({ selection: { anchor: line.from + col }, scrollIntoView: true });
  return true;
}

function vimMoveWordBackward(view: EditorView): boolean {
  const head = view.state.selection.main.head;
  const line = view.state.doc.lineAt(head);
  const text = line.text;
  let col = head - line.from;

  if (col === 0) {
    if (line.number > 1) {
      const prevLine = view.state.doc.line(line.number - 1);
      view.dispatch({ selection: { anchor: prevLine.to }, scrollIntoView: true });
    }
    return true;
  }

  col -= 1;
  while (col > 0 && getCharClass(text[col]) === 0) {
    col -= 1;
  }

  const targetClass = getCharClass(text[col]);
  while (col > 0 && getCharClass(text[col - 1]) === targetClass) {
    col -= 1;
  }

  view.dispatch({ selection: { anchor: line.from + col }, scrollIntoView: true });
  return true;
}

function firstCodePoint(value: string): number | null {
  if (!value) return null;
  const codePoint = value.codePointAt(0);
  return typeof codePoint === "number" ? codePoint : null;
}

function toVimKeyInput(event: KeyboardEvent): VimKeyInput | null {
  const key = event.key;
  const code = event.code;

  if (key === "Escape" || key === "Esc" || code === "Escape") {
    return { kind: VIM_KEY_KIND.ESC };
  }
  if (event.key === "Enter") return { kind: VIM_KEY_KIND.ENTER };
  if (event.key === "Tab") return { kind: VIM_KEY_KIND.TAB };
  if (event.key === "Backspace") return { kind: VIM_KEY_KIND.BACKSPACE };
  if (key === "Delete" || key === "Del") return { kind: VIM_KEY_KIND.DELETE };
  if (key === "ArrowUp" || key === "Up" || code === "ArrowUp") {
    return { kind: VIM_KEY_KIND.ARROW_UP };
  }
  if (key === "ArrowDown" || key === "Down" || code === "ArrowDown") {
    return { kind: VIM_KEY_KIND.ARROW_DOWN };
  }
  if (key === "ArrowLeft" || key === "Left" || code === "ArrowLeft") {
    return { kind: VIM_KEY_KIND.ARROW_LEFT };
  }
  if (key === "ArrowRight" || key === "Right" || code === "ArrowRight") {
    return { kind: VIM_KEY_KIND.ARROW_RIGHT };
  }

  const isPlain = !event.ctrlKey && !event.altKey && !event.metaKey;
  if (isPlain && (key === "Home" || code === "Home")) {
    return { kind: VIM_KEY_KIND.CHAR, charCode: "0".charCodeAt(0) };
  }
  if (isPlain && (key === "End" || code === "End")) {
    return { kind: VIM_KEY_KIND.CHAR, charCode: "$".charCodeAt(0) };
  }

  if (event.ctrlKey && !event.altKey && !event.metaKey) {
    const codePoint = firstCodePoint(event.key.toLowerCase());
    if (codePoint !== null) {
      return { kind: VIM_KEY_KIND.CTRL, charCode: codePoint };
    }
    return null;
  }

  if (isPlain && key.length === 1) {
    const codePoint = firstCodePoint(key);
    if (codePoint !== null) {
      return { kind: VIM_KEY_KIND.CHAR, charCode: codePoint };
    }
  }

  return null;
}

function shouldSwallowInNormalLikeMode(event: KeyboardEvent): boolean {
  if (event.ctrlKey && !event.altKey && !event.metaKey && event.key.toLowerCase() === "v") {
    return true;
  }
  if (isPrintableTextKey(event)) return true;
  return (
    event.key === "Backspace" ||
    event.key === "Delete" ||
    event.key === "Enter" ||
    event.key === "Tab"
  );
}

function findWordObjectRange(
  view: EditorView,
  around: boolean,
): { from: number; to: number } | null {
  const main = view.state.selection.main;
  const line = view.state.doc.lineAt(main.head);
  const text = line.text;
  const len = text.length;
  if (len === 0) return null;

  const isWordChar = (char: string) => /[A-Za-z0-9_]/.test(char);

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
}

function findPipeObjectRange(
  view: EditorView,
  around: boolean,
): { from: number; to: number } | null {
  const main = view.state.selection.main;
  const line = view.state.doc.lineAt(main.head);
  const text = line.text;
  if (text.length < 2) return null;

  const pipes: number[] = [];
  for (let i = 0; i < text.length; i++) {
    if (text[i] === "|") pipes.push(i);
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
}

export function vimModeExtension(options: VimOptions = {}) {
  const session = new VimSession("insert");
  let currentMode: VimUiMode = "insert";
  let unnamedRegister: VimRegister | null = null;
  let swallowVisualDdUntilMs = 0;
  let pendingFoldPrefixUntilMs = 0;

  let visualAnchorPos: number | null = null;
  let visualAnchorLine: number | null = null; // 1-based
  let visualHeadLine: number | null = null; // 1-based (visual-line only)

  const mode = (): VimUiMode => currentMode;

  const setRegister = (text: string, registerMode: VimRegisterMode = "charwise") => {
    if (registerMode === "charwise" && !text) return;
    unnamedRegister = { text, mode: registerMode };
    copyToClipboard(text);
  };

  const insertAfterCursorOnce = (view: EditorView, text: string) => {
    if (!text) return;
    const head = view.state.selection.main.head;
    const line = view.state.doc.lineAt(head);
    const insertAt = head < line.to ? head + 1 : line.to;
    view.dispatch({
      changes: { from: insertAt, to: insertAt, insert: text },
      selection: { anchor: insertAt + text.length },
      scrollIntoView: true,
    });
  };

  const pasteLinewiseBelow = (view: EditorView, text: string, repeats: number) => {
    const normalized = text.endsWith("\n") ? text.slice(0, -1) : text;
    const lines = normalized.length > 0 ? normalized.split("\n") : [""];
    if (lines.length === 0) return;
    const repeated: string[] = [];
    for (let i = 0; i < repeats; i++) {
      repeated.push(...lines);
    }
    const head = view.state.selection.main.head;
    const line = view.state.doc.lineAt(head);
    const insertAt = line.to;
    const insert = `\n${repeated.join("\n")}`;
    view.dispatch({
      changes: { from: insertAt, to: insertAt, insert },
      selection: { anchor: insertAt + 1 },
      scrollIntoView: true,
    });
  };

  const pasteAfter = (view: EditorView, count: number) => {
    const repeats = Math.max(1, count);
    const pasteRegister = (register: VimRegister) => {
      if (register.mode === "charwise" && !register.text) return;
      if (register.mode === "linewise") {
        pasteLinewiseBelow(view, register.text, repeats);
        return;
      }
      for (let i = 0; i < repeats; i++) {
        insertAfterCursorOnce(view, register.text);
      }
    };

    if (unnamedRegister) {
      pasteRegister(unnamedRegister);
      return true;
    }

    if (navigator.clipboard?.readText) {
      void navigator.clipboard
        .readText()
        .then((text) => {
          if (!text) return;
          unnamedRegister = { text, mode: "charwise" };
          pasteRegister(unnamedRegister);
        })
        .catch((err) => {
          console.error("Vim paste failed:", err);
        });
    }
    return true;
  };

  const syncModeClasses = (view: EditorView) => {
    updateModeClasses(view, currentMode);
  };

  const refreshFocusedCursor = (view: EditorView) => {
    syncModeClasses(view);
    const current = mode();
    if (current === "insert") return;
    if (current === "visual" || current === "visual-line") {
      updateVisualSelection(view);
      return;
    }
    // Force a cursor layer refresh when regaining focus so normal-mode
    // block cursor is restored immediately without waiting for movement keys.
    const head = view.state.selection.main.head;
    view.dispatch({ selection: { anchor: head } });
  };

  const collapseSelection = (view: EditorView) => {
    const head = view.state.selection.main.head;
    view.dispatch({ selection: { anchor: head }, scrollIntoView: true });
  };

  const resetVisualAnchors = () => {
    visualAnchorPos = null;
    visualAnchorLine = null;
    visualHeadLine = null;
  };

  const setModeLocally = (view: EditorView, next: VimUiMode) => {
    const prev = currentMode;
    currentMode = next;

    if (next === "normal") {
      resetVisualAnchors();
      if (prev === "visual" || prev === "visual-line") {
        collapseSelection(view);
      }
    }

    syncModeClasses(view);
  };

  const getHeadInfo = (view: EditorView) => {
    const head = view.state.selection.main.head;
    const line = view.state.doc.lineAt(head);
    return {
      head,
      lineNumber: line.number,
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
    const lineNumber =
      visualHeadLine ??
      Math.min(Math.max(getHeadInfo(view).lineNumber, 1), view.state.doc.lines);
    const anchorLine = view.state.doc.line(visualAnchorLine);
    const headLine = view.state.doc.line(lineNumber);
    const anchor =
      lineNumber >= visualAnchorLine ? anchorLine.from : anchorLine.to;
    const head = lineNumber >= visualAnchorLine ? headLine.to : headLine.from;
    view.dispatch({
      selection: { anchor, head },
      scrollIntoView: true,
    });
  };

  const updateVisualSelection = (view: EditorView) => {
    if (mode() === "visual") {
      applyVisualCharSelection(view);
      return;
    }
    if (mode() === "visual-line") {
      applyVisualLineSelection(view);
    }
  };

  const startVisualMode = (view: EditorView, next: "visual" | "visual-line") => {
    const info = getHeadInfo(view);

    if (next === "visual") {
      visualAnchorPos = info.head;
      visualAnchorLine = null;
      visualHeadLine = null;
    } else {
      visualAnchorLine = info.lineNumber;
      visualHeadLine = info.lineNumber;
      visualAnchorPos = null;
    }

    syncModeClasses(view);
    updateVisualSelection(view);
  };

  const runCounted = (
    view: EditorView,
    command: (target: EditorView) => boolean,
    explicitCount: number,
  ) => {
    const count = Number.isFinite(explicitCount) && explicitCount > 0 ? explicitCount : 1;
    for (let i = 0; i < count; i++) {
      if (!command(view)) break;
    }
    return true;
  };

  const runMove = (
    view: EditorView,
    command: (target: EditorView) => boolean,
    explicitCount: number,
  ) => {
    const currentMode = mode();
    const isVisual = currentMode === "visual" || currentMode === "visual-line";
    const count = Number.isFinite(explicitCount) && explicitCount > 0 ? explicitCount : 1;

    if (currentMode === "visual-line" && (command === cursorLineUp || command === cursorLineDown)) {
      let headLine = visualHeadLine ?? getHeadInfo(view).lineNumber;
      for (let i = 0; i < count; i++) {
        headLine += command === cursorLineUp ? -1 : 1;
        headLine = Math.min(Math.max(headLine, 1), view.state.doc.lines);
      }
      visualHeadLine = headLine;
      applyVisualLineSelection(view);
      return true;
    }

    if (isVisual) {
      const main = view.state.selection.main;
      if (!main.empty) {
        let caret = main.head;
        if (currentMode === "visual-line") {
          const line = view.state.doc.lineAt(caret);
          // Keep caret inside the visual-line head line to avoid boundary
          // ambiguity when reversing movement direction.
          if (caret === line.to && line.to > line.from) {
            caret = line.to - 1;
          }
        }
        // Keep a stable caret head for movement commands while preserving
        // Vim visual anchor semantics.
        view.dispatch({ selection: { anchor: caret } });
      }
    }
    runCounted(view, command, count);
    if (isVisual) {
      if (currentMode === "visual-line") {
        let caret = view.state.selection.main.head;
        const line = view.state.doc.lineAt(caret);
        if (caret === line.to && line.to > line.from) {
          caret = line.to - 1;
        }
        visualHeadLine = view.state.doc.lineAt(caret).number;
      }
      updateVisualSelection(view);
    }
    return true;
  };

  const applyTextObject = (
    view: EditorView,
    object: "word" | "pipe",
    around: boolean,
    shouldDelete: boolean,
    count: number,
  ): number => {
    const steps = count > 0 ? count : 1;
    const chunks: string[] = [];

    for (let i = 0; i < steps; i++) {
      const range =
        object === "word"
          ? findWordObjectRange(view, around)
          : findPipeObjectRange(view, around);
      if (!range) break;

      const text = view.state.sliceDoc(range.from, range.to);
      if (!text) break;
      chunks.push(text);

      if (shouldDelete) {
        view.dispatch({
          changes: { from: range.from, to: range.to, insert: "" },
          selection: { anchor: range.from },
          scrollIntoView: true,
        });
      } else {
        view.dispatch({
          selection: { anchor: range.to },
          scrollIntoView: true,
        });
      }
    }

    if (chunks.length > 0) {
      setRegister(chunks.join("\n"));
    }

    return chunks.length;
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

  const deleteToLineStart = (view: EditorView, count: number) => {
    let changed = false;
    for (let i = 0; i < count; i++) {
      const head = view.state.selection.main.head;
      const line = view.state.doc.lineAt(head);
      if (head <= line.from) break;
      deleteRange(view, line.from, head);
      changed = true;
    }
    return changed;
  };

  const deleteToLineEnd = (view: EditorView, count: number) => {
    let changed = false;
    for (let i = 0; i < count; i++) {
      const head = view.state.selection.main.head;
      const line = view.state.doc.lineAt(head);
      if (head >= line.to) break;
      deleteRange(view, head, line.to);
      changed = true;
    }
    return changed;
  };

  const yankToLineStart = (view: EditorView, count: number) => {
    const chunks: string[] = [];
    for (let i = 0; i < count; i++) {
      const head = view.state.selection.main.head;
      const line = view.state.doc.lineAt(head);
      if (head <= line.from) break;
      chunks.push(view.state.sliceDoc(line.from, head));
    }
    if (chunks.length > 0) {
      setRegister(chunks.join("\n"));
      return true;
    }
    return false;
  };

  const yankToLineEnd = (view: EditorView, count: number) => {
    const chunks: string[] = [];
    for (let i = 0; i < count; i++) {
      const head = view.state.selection.main.head;
      const line = view.state.doc.lineAt(head);
      if (head >= line.to) break;
      chunks.push(view.state.sliceDoc(head, line.to));
    }
    if (chunks.length > 0) {
      setRegister(chunks.join("\n"));
      return true;
    }
    return false;
  };

  const deleteWordForward = (view: EditorView, count: number) => {
    let changed = false;
    for (let i = 0; i < count; i++) {
      const from = view.state.selection.main.head;
      vimMoveWordForward(view);
      const to = view.state.selection.main.head;
      if (from === to) break;
      const start = Math.min(from, to);
      const end = Math.max(from, to);
      setRegister(view.state.sliceDoc(start, end));
      deleteRange(view, start, end);
      changed = true;
    }
    return changed;
  };

  const deleteWordBackward = (view: EditorView, count: number) => {
    let changed = false;
    for (let i = 0; i < count; i++) {
      const from = view.state.selection.main.head;
      vimMoveWordBackward(view);
      const to = view.state.selection.main.head;
      if (from === to) break;
      const start = Math.min(from, to);
      const end = Math.max(from, to);
      setRegister(view.state.sliceDoc(start, end));
      deleteRange(view, start, end);
      changed = true;
    }
    return changed;
  };

  const yankWordForward = (view: EditorView, count: number) => {
    const chunks: string[] = [];
    const origHead = view.state.selection.main.head;
    for (let i = 0; i < count; i++) {
      const from = view.state.selection.main.head;
      vimMoveWordForward(view);
      const to = view.state.selection.main.head;
      if (from === to) break;
      chunks.push(view.state.sliceDoc(Math.min(from, to), Math.max(from, to)));
    }
    view.dispatch({ selection: { anchor: origHead }, scrollIntoView: true });
    if (chunks.length > 0) {
      setRegister(chunks.join(""));
      return true;
    }
    return false;
  };

  const yankWordBackward = (view: EditorView, count: number) => {
    const chunks: string[] = [];
    const origHead = view.state.selection.main.head;
    for (let i = 0; i < count; i++) {
      const from = view.state.selection.main.head;
      vimMoveWordBackward(view);
      const to = view.state.selection.main.head;
      if (from === to) break;
      chunks.push(view.state.sliceDoc(Math.min(from, to), Math.max(from, to)));
    }
    view.dispatch({ selection: { anchor: origHead }, scrollIntoView: true });
    if (chunks.length > 0) {
      chunks.reverse();
      setRegister(chunks.join(""));
      return true;
    }
    return false;
  };

  const yankVisualSelection = (view: EditorView) => {
    if (mode() === "visual-line") {
      const main = view.state.selection.main;
      const start = view.state.doc.lineAt(main.from).number;
      const end = view.state.doc.lineAt(main.to).number;
      const parts: string[] = [];
      for (let lineNo = start; lineNo <= end; lineNo++) {
        parts.push(view.state.doc.line(lineNo).text);
      }
      setRegister(parts.join("\n"), "linewise");
      return true;
    }

    const text = view.state.sliceDoc(view.state.selection.main.from, view.state.selection.main.to);
    setRegister(text);
    return true;
  };

  const deleteVisualSelection = (view: EditorView) => {
    if (mode() === "visual-line") {
      const main = view.state.selection.main;
      let startLine = visualAnchorLine ?? view.state.doc.lineAt(main.from).number;
      let endLine = visualHeadLine ?? view.state.doc.lineAt(main.to).number;
      if (endLine < startLine) {
        [startLine, endLine] = [endLine, startLine];
      }

      const parts: string[] = [];
      for (let lineNo = startLine; lineNo <= endLine; lineNo++) {
        parts.push(view.state.doc.line(lineNo).text);
      }
      setRegister(parts.join("\n"), "linewise");

      let from = view.state.doc.line(startLine).from;
      let to = view.state.doc.line(endLine).to;
      if (endLine < view.state.doc.lines) {
        // Remove trailing line break to delete whole selected lines cleanly.
        to += 1;
      } else if (startLine > 1) {
        // Last line selection: remove the preceding line break.
        from = view.state.doc.line(startLine - 1).to;
      }

      view.dispatch({
        changes: { from, to, insert: "" },
        selection: { anchor: from },
        scrollIntoView: true,
      });
      return true;
    }

    const main = view.state.selection.main;
    let from = main.from;
    let to = main.to;
    if (from === to) {
      const line = view.state.doc.lineAt(main.head);
      from = Math.min(main.head, line.to);
      to = Math.min(from + 1, line.to);
      if (to <= from) return false;
    }

    setRegister(view.state.sliceDoc(from, to));
    view.dispatch({
      changes: { from, to, insert: "" },
      selection: { anchor: from },
      scrollIntoView: true,
    });
    return true;
  };

  const yankCurrentLines = (view: EditorView, count: number) => {
    const current = view.state.doc.lineAt(view.state.selection.main.head).number;
    const end = Math.min(view.state.doc.lines, current + count - 1);
    const parts: string[] = [];
    for (let lineNo = current; lineNo <= end; lineNo++) {
      parts.push(view.state.doc.line(lineNo).text);
    }
    setRegister(parts.join("\n"), "linewise");
    return true;
  };

  const deleteCurrentLines = (view: EditorView, count: number) => {
    const current = view.state.doc.lineAt(view.state.selection.main.head).number;
    const end = Math.min(view.state.doc.lines, current + count - 1);
    const parts: string[] = [];
    for (let lineNo = current; lineNo <= end; lineNo++) {
      parts.push(view.state.doc.line(lineNo).text);
    }
    if (parts.length === 0) return false;
    setRegister(parts.join("\n"), "linewise");

    let from = view.state.doc.line(current).from;
    let to = view.state.doc.line(end).to;
    if (end < view.state.doc.lines) {
      to += 1;
    } else if (current > 1) {
      from = view.state.doc.line(current - 1).to;
    }

    view.dispatch({
      changes: { from, to, insert: "" },
      selection: { anchor: from },
      scrollIntoView: true,
    });
    return true;
  };

  const applyAction = (view: EditorView, action: VimAction) => {
    const count = action.count > 0 ? action.count : 1;

    switch (action.intent) {
      case 0: // move_left
        return runMove(view, cursorCharLeft, count);
      case 1: // move_right
        return runMove(view, cursorCharRight, count);
      case 2: // move_up
        return runMove(view, cursorLineUp, count);
      case 3: // move_down
        return runMove(view, cursorLineDown, count);
      case 4: // move_word_forward
        return runMove(view, vimMoveWordForward, count);
      case 5: // move_word_backward
        return runMove(view, vimMoveWordBackward, count);
      case 6: // move_line_start
        return runMove(view, cursorLineStart, count);
      case 7: // move_line_end
        return runMove(view, moveToLineEndNormalLike, count);
      case 8: // move_doc_start
        moveToDocStart(view);
        if (mode() === "visual" || mode() === "visual-line") {
          updateVisualSelection(view);
        }
        return true;
      case 9: // move_doc_end
        moveToDocEnd(view);
        if (mode() === "visual" || mode() === "visual-line") {
          updateVisualSelection(view);
        }
        return true;
      case 10: { // move_to_line
        const lineNo = Math.min(Math.max(count, 1), view.state.doc.lines);
        const pos = view.state.doc.line(lineNo).from;
        view.dispatch({ selection: { anchor: pos }, scrollIntoView: true });
        if (mode() === "visual" || mode() === "visual-line") {
          updateVisualSelection(view);
        }
        return true;
      }
      case 11: // enter_insert
        return true;
      case 12: // append_insert
        return appendInsertWithinLine(view);
      case 13: // insert_line_start
        return cursorLineStart(view);
      case 14: // append_line_end
        return cursorLineEnd(view);
      case 15: // open_line_below
        return insertLineBelow(view);
      case 16: // open_line_above
        return insertLineAbove(view);
      case 17: // enter_visual
        startVisualMode(view, "visual");
        return true;
      case 18: // enter_visual_line
        startVisualMode(view, "visual-line");
        return true;
      case 19: // exit_visual
        resetVisualAnchors();
        collapseSelection(view);
        return true;
      case 20: // delete_line
        return deleteCurrentLines(view, count);
      case 21: // yank_line
        return yankCurrentLines(view, count);
      case 22: // delete_to_line_start
        return deleteToLineStart(view, count);
      case 23: // delete_to_line_end
        return deleteToLineEnd(view, count);
      case 24: // yank_to_line_start
        return yankToLineStart(view, count);
      case 25: // yank_to_line_end
        return yankToLineEnd(view, count);
      case 26: // delete_char
        return runCounted(view, deleteCharForward, count);
      case 27: // paste_after
        return pasteAfter(view, count);
      case 28: // undo
        return runCounted(view, undo, count);
      case 29: // redo
        return runCounted(view, redo, count);
      case 30: // open_command_bar
        // Preserve visual selection for command execution: command-bar focus
        // can collapse the CM selection when mode is normal.
        const preservedSelection =
          view.state.selection.main.empty ? undefined : {
            anchor: view.state.selection.main.anchor,
            head: view.state.selection.main.head,
          };
        openCommandPicker(view, {
          mode: "vim",
          dateFormat: options.dateFormat,
          dateTimeFormat: options.dateTimeFormat,
          onExitCommand: options.onExitCommand,
          onClipWatchStateChange: options.onClipWatchStateChange,
          onClipWatchPaste: options.onClipWatchPaste,
          getNoteModules: options.getNoteModules,
          setNoteModules: options.setNoteModules,
          source: "vim-colon",
          selectionOverride: preservedSelection,
        });
        return true;
      case 31: // open_search
        return openEditorSearch(view);
      case 32: // search_next
        return editorSearchNext(view);
      case 33: // search_prev
        return editorSearchPrev(view);
      case 34: // delete_inside_word
        return applyTextObject(view, "word", false, true, count) > 0;
      case 35: // delete_around_word
        return applyTextObject(view, "word", true, true, count) > 0;
      case 36: // yank_inside_word
        return applyTextObject(view, "word", false, false, count) > 0;
      case 37: // yank_around_word
        return applyTextObject(view, "word", true, false, count) > 0;
      case 38: // delete_inside_pipe
        return applyTextObject(view, "pipe", false, true, count) > 0;
      case 39: // delete_around_pipe
        return applyTextObject(view, "pipe", true, true, count) > 0;
      case 40: // yank_inside_pipe
        return applyTextObject(view, "pipe", false, false, count) > 0;
      case 41: // yank_around_pipe
        return applyTextObject(view, "pipe", true, false, count) > 0;
      case 42: // swallow
        return true;
      case 43: // delete_word_forward
        return deleteWordForward(view, count);
      case 44: // delete_word_backward
        return deleteWordBackward(view, count);
      case 45: // yank_word_forward
        return yankWordForward(view, count);
      case 46: // yank_word_backward
        return yankWordBackward(view, count);
      default:
        return true;
    }
  };

  const focusSync = EditorView.updateListener.of((update) => {
    if (!update.focusChanged || !update.view.hasFocus) return;
    refreshFocusedCursor(update.view);
  });

  const handlers = EditorView.domEventHandlers({
    focus: (_event, view) => {
      refreshFocusedCursor(view);
      return false;
    },
    blur: (_event, view) => {
      syncModeClasses(view);
      return false;
    },
    keydown: (event, view) => {
      if (isCommandPickerOpen(view)) {
        return false;
      }
      if (isEditorSearchOverlayTarget(view, event.target)) {
        return false;
      }

      if (
        (event.ctrlKey || event.metaKey) &&
        !event.altKey &&
        !event.shiftKey &&
        event.key.toLowerCase() === "f"
      ) {
        event.preventDefault();
        return openEditorSearch(view);
      }

      const activeMode = mode();
      const now = Date.now();
      if (pendingFoldPrefixUntilMs > 0 && now > pendingFoldPrefixUntilMs) {
        pendingFoldPrefixUntilMs = 0;
      }

      if (activeMode !== "normal") {
        pendingFoldPrefixUntilMs = 0;
      }

      const plain = !event.ctrlKey && !event.altKey && !event.metaKey;
      if (activeMode === "normal" && plain) {
        const plainKey = event.key.toLowerCase();
        if (pendingFoldPrefixUntilMs > 0) {
          pendingFoldPrefixUntilMs = 0;
          if (plainKey === "a") {
            event.preventDefault();
            return toggleFoldAtCursor(view);
          }
        } else if (plainKey === "z") {
          event.preventDefault();
          pendingFoldPrefixUntilMs = now + 900;
          return true;
        }
      }

      if (
        activeMode === "normal"
        && now <= swallowVisualDdUntilMs
        && !event.ctrlKey
        && !event.altKey
        && !event.metaKey
        && event.key === "d"
      ) {
        swallowVisualDdUntilMs = 0;
        event.preventDefault();
        return true;
      }

      // Hard guarantee for visual behavior: Esc exits and x/d delete the
      // current selection in a single keypress. Navigation remains in the
      // shared Vim state machine so counts/motions work consistently.
      if (activeMode === "visual" || activeMode === "visual-line") {
        if (event.key === "Escape" || event.key === "Esc" || event.code === "Escape") {
          event.preventDefault();
          setModeLocally(view, "normal");
          session.step({ kind: VIM_KEY_KIND.ESC }, { line_count: view.state.doc.lines });
          return true;
        }

        const plain = !event.ctrlKey && !event.altKey && !event.metaKey;
        if (plain) {
          if (event.key === "x" || event.key === "d") {
            event.preventDefault();
            const deleted = deleteVisualSelection(view);
            if (event.key === "d") {
              // Let `dd` in visual modes behave like single delete by swallowing
              // the immediate follow-up `d` from that key sequence.
              swallowVisualDdUntilMs = now + 180;
            }
            if (deleted) {
              setModeLocally(view, "normal");
              session.step({ kind: VIM_KEY_KIND.ESC }, { line_count: view.state.doc.lines });
            }
            return true;
          }
        }
      }

      // Insert-mode fast path: avoid wasm roundtrip for regular insert editing.
      if (activeMode === "insert" && event.key !== "Escape") {
        return false;
      }

      const keyInput = toVimKeyInput(event);
      if (!keyInput) {
        if (activeMode !== "insert" && shouldSwallowInNormalLikeMode(event)) {
          event.preventDefault();
          return true;
        }
        return false;
      }

      const step = session.step(keyInput, {
        has_search_matches: editorSearchHasMatches(view),
        line_count: view.state.doc.lines,
      });

      if (!step) {
        if (activeMode !== "insert" && shouldSwallowInNormalLikeMode(event)) {
          event.preventDefault();
          return true;
        }
        return false;
      }

      currentMode = toUiMode(step.mode);
      syncModeClasses(view);

      if (!step.handled) {
        const isVisualYank =
          (mode() === "visual" || mode() === "visual-line") &&
          !event.ctrlKey &&
          !event.altKey &&
          !event.metaKey &&
          event.key === "y";
        if (isVisualYank) {
          event.preventDefault();
          yankVisualSelection(view);
          setModeLocally(view, "normal");
          session.step({ kind: VIM_KEY_KIND.ESC }, { line_count: view.state.doc.lines });
          return true;
        }
        if (mode() !== "insert" && shouldSwallowInNormalLikeMode(event)) {
          event.preventDefault();
          return true;
        }
        return false;
      }

      event.preventDefault();
      for (const action of step.actions) {
        applyAction(view, action);
      }

      if (mode() === "visual" || mode() === "visual-line") {
        updateVisualSelection(view);
      } else {
        resetVisualAnchors();
      }

      return true;
    },
  });

  return [handlers, focusSync];
}
