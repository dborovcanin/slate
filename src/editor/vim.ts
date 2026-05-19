import {
  cursorCharLeft,
  cursorCharRight,
  cursorGroupLeft,
  cursorGroupRight,
  cursorLineDown,
  cursorLineEnd,
  cursorLineStart,
  cursorLineUp,
  deleteCharBackward,
  deleteCharForward,
  deleteGroupBackward,
  deleteGroupForward,
  redoDepth,
  redo,
  undoDepth,
  undo,
} from "@codemirror/commands";
import { EditorView, ViewPlugin } from "@codemirror/view";
import { isCommandPickerOpen, openCommandPicker } from "./command-picker";
import type { NoteModules } from "../api.ts";
import { toggleFoldAtCursor } from "./folding.ts";
import {
  applyEditOperations,
  offsetEditOperation,
  snapshotFromViewLines,
} from "./core/codemirror-adapter.ts";
import { Transaction } from "@codemirror/state";
import {
  editorSearchHasMatches,
  editorSearchNext,
  editorSearchPrev,
  isEditorSearchOverlayTarget,
  openEditorSearch,
} from "./search";
import {
  VIM_INTENT,
  VimSession,
  markdownWikiLinkAtCursor,
  executeVimActionFromWasm,
  type VimAction,
  type VimIntent,
  type VimMode,
  type VimRegisterValue,
} from "./wasm.ts";
import { resolveWikiLink } from "../api.ts";
import { runUiVimPipeline } from "./vim-adapter.ts";
import { vimAppendInsertPos, vimNormalLineEndPos } from "./vim-utils.ts";

type VimUiMode = "insert" | "normal" | "visual" | "visual-line";

interface VimOptions {
  dateFormat?: string;
  dateTimeFormat?: string;
  onWriteCommand?: (options?: { force?: boolean }) => Promise<void> | void;
  onExitCommand?: () => Promise<void> | void;
  onExportCommand?: (options: {
    format: "pdf" | "md" | "txt";
    path: string | null;
  }) => Promise<string | void> | string | void;
  onBackupCommand?: (options: {
    path: string | null;
  }) => Promise<string | void> | string | void;
  onClipWatchStateChange?: (active: boolean) => void;
  onClipWatchPaste?: (text: string) => void;
  getNoteModules?: () => NoteModules | null;
  setNoteModules?: (modules: NoteModules) => Promise<void> | void;
  onNavigateToNote?: (noteId: string, heading?: string) => void;
  onMacroRecordingChange?: (register: string | null) => void;
  onVimStatusMessage?: (message: string) => void;
}

type VimRegisterMode = "charwise" | "linewise";

interface VimRegister {
  text: string;
  mode: VimRegisterMode;
}

interface MacroInsertEvent {
  key: string;
  code: string;
  ctrlKey: boolean;
  altKey: boolean;
  metaKey: boolean;
}

type MacroStep =
  | { kind: "action"; action: VimAction }
  | { kind: "insert_event"; event: MacroInsertEvent };

const SHARED_VIM_FULL_DOC_MAX_BYTES = 200_000;
const VIM_MACRO_REPLAY_STEP_BUDGET = 10_000;
const VIM_MACRO_PENDING_RECORD = 1;
const VIM_MACRO_PENDING_PLAY = 2;

function isMacroRegisterChar(char: string | undefined): boolean {
  if (!char || char.length === 0) return false;
  return /^[A-Za-z0-9]$/.test(char.slice(0, 1));
}

function macroRegisterSummary(registers: Map<string, MacroStep[]>): string {
  if (registers.size === 0) return "no recorded macros";
  const entries = [...registers.entries()]
    .map(([register, steps]) => [register, steps.length] as const)
    .filter(([, steps]) => steps > 0)
    .sort((a, b) => a[0].localeCompare(b[0]));
  if (entries.length === 0) return "no recorded macros";
  const preview = entries.slice(0, 6).map(([register, steps]) => `@${register}:${steps}`);
  const remainder = entries.length > 6 ? ` +${entries.length - 6}` : "";
  return `macros ${preview.join(" ")}${remainder}`;
}

function shouldExecuteSharedVimAction(intent: VimIntent): boolean {
  switch (intent) {
    case VIM_INTENT.DELETE_LINE:
    case VIM_INTENT.YANK_LINE:
    case VIM_INTENT.DELETE_TO_LINE_START:
    case VIM_INTENT.DELETE_TO_LINE_END:
    case VIM_INTENT.YANK_TO_LINE_START:
    case VIM_INTENT.YANK_TO_LINE_END:
    case VIM_INTENT.DELETE_CHAR:
    case VIM_INTENT.PASTE_AFTER:
    case VIM_INTENT.DELETE_WORD_FORWARD:
    case VIM_INTENT.DELETE_WORD_BACKWARD:
    case VIM_INTENT.DELETE_WORD_END:
    case VIM_INTENT.DELETE_TILL_CHAR:
    case VIM_INTENT.YANK_WORD_FORWARD:
    case VIM_INTENT.YANK_WORD_BACKWARD:
    case VIM_INTENT.DELETE_INSIDE_WORD:
    case VIM_INTENT.DELETE_AROUND_WORD:
    case VIM_INTENT.YANK_INSIDE_WORD:
    case VIM_INTENT.YANK_AROUND_WORD:
    case VIM_INTENT.DELETE_INSIDE_PIPE:
    case VIM_INTENT.DELETE_AROUND_PIPE:
    case VIM_INTENT.YANK_INSIDE_PIPE:
    case VIM_INTENT.YANK_AROUND_PIPE:
    case VIM_INTENT.DELETE_INSIDE_PAREN:
    case VIM_INTENT.DELETE_INSIDE_BRACKET:
    case VIM_INTENT.DELETE_INSIDE_BRACE:
    case VIM_INTENT.DELETE_INSIDE_DOUBLE_QUOTE:
    case VIM_INTENT.DELETE_INSIDE_BACKTICK:
      return true;
    default:
      return false;
  }
}

function isRecordableMacroIntent(intent: VimIntent): boolean {
  switch (intent) {
    case VIM_INTENT.START_MACRO_RECORD:
    case VIM_INTENT.STOP_MACRO_RECORD:
    case VIM_INTENT.PLAY_MACRO:
    case VIM_INTENT.OPEN_COMMAND_BAR:
    case VIM_INTENT.OPEN_SEARCH:
      return false;
    default:
      return true;
  }
}

function scopedSharedVimMarginLines(intent: VimIntent, count: number): number {
  const repeats = Math.max(1, count);
  switch (intent) {
    case VIM_INTENT.DELETE_LINE:
    case VIM_INTENT.YANK_LINE:
      return repeats + 2;
    case VIM_INTENT.DELETE_CHAR:
      return repeats + 2;
    case VIM_INTENT.PASTE_AFTER:
      return 2;
    case VIM_INTENT.DELETE_WORD_FORWARD:
    case VIM_INTENT.DELETE_WORD_BACKWARD:
    case VIM_INTENT.DELETE_WORD_END:
    case VIM_INTENT.DELETE_TILL_CHAR:
    case VIM_INTENT.YANK_WORD_FORWARD:
    case VIM_INTENT.YANK_WORD_BACKWARD:
      return repeats + 8;
    default:
      return 2;
  }
}

function executeSharedVimAction(
  view: EditorView,
  action: VimAction,
  count: number,
  register: VimRegisterValue | null,
) {
  // Keep the exact full-snapshot path for smaller docs. Large docs use the
  // same shared WASM executor, but with a cursor-local window to avoid
  // serializing the whole rope for common Vim edits.
  if (view.state.doc.length <= SHARED_VIM_FULL_DOC_MAX_BYTES) {
    return executeVimActionFromWasm(
      {
        text: view.state.doc.toString(),
        selection: {
          anchor: view.state.selection.main.anchor,
          head: view.state.selection.main.head,
        },
      },
      action.intent,
      count,
      register,
      action.targetChar ?? null,
    );
  }

  const scoped = snapshotFromViewLines(
    view,
    scopedSharedVimMarginLines(action.intent, count),
  );
  const result = executeVimActionFromWasm(
    scoped.snapshot,
    action.intent,
    count,
    register,
    action.targetChar ?? null,
  );
  if (!result) return null;
  return {
    ...result,
    operations: result.operations.map((operation) =>
      offsetEditOperation(operation, scoped.offset),
    ),
  };
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

function runEndByClass(text: string, start: number, klass: number): number {
  let cursor = Math.max(0, Math.min(start, text.length));
  while (cursor < text.length) {
    const next = cursor + 1;
    if (next >= text.length || getCharClass(text[next] ?? "") !== klass) {
      break;
    }
    cursor = next;
  }
  return cursor;
}

function findNextNonWhitespace(text: string, start: number): number | null {
  let cursor = Math.max(0, Math.min(start, text.length));
  while (cursor < text.length) {
    if (getCharClass(text[cursor] ?? "") !== 0) {
      return cursor;
    }
    cursor += 1;
  }
  return null;
}

function moveWordEndPos(text: string, offset: number): number | null {
  if (text.length === 0) return null;
  const cursor = Math.max(0, Math.min(offset, text.length - 1));
  const klass = getCharClass(text[cursor] ?? "");
  if (klass !== 0) {
    const next = cursor + 1;
    if (next < text.length && getCharClass(text[next] ?? "") === klass) {
      return runEndByClass(text, cursor, klass);
    }
    const nextWord = findNextNonWhitespace(text, next);
    if (nextWord !== null) {
      return runEndByClass(text, nextWord, getCharClass(text[nextWord] ?? ""));
    }
    return cursor;
  }
  const nextWord = findNextNonWhitespace(text, cursor);
  if (nextWord !== null) {
    return runEndByClass(text, nextWord, getCharClass(text[nextWord] ?? ""));
  }
  return runEndByClass(text, cursor, 0);
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

function toRecordableInsertMacroEvent(event: KeyboardEvent): MacroInsertEvent | null {
  if (event.metaKey) return null;
  const key = event.key;
  const code = event.code;
  const isNavigation =
    key === "ArrowUp" ||
    key === "ArrowDown" ||
    key === "ArrowLeft" ||
    key === "ArrowRight" ||
    key === "Home" ||
    key === "End";
  const isEditingKey =
    key === "Escape" ||
    key === "Enter" ||
    key === "Tab" ||
    key === "Backspace" ||
    key === "Delete";
  const isWordNavigation =
    (key === "ArrowLeft" || key === "ArrowRight") && (event.ctrlKey || event.altKey);
  const isWordDelete =
    (key === "Backspace" || key === "Delete") && (event.ctrlKey || event.altKey);
  const isChar = key.length === 1;
  if (!isNavigation && !isEditingKey && !isChar && !isWordNavigation && !isWordDelete) return null;
  if (event.ctrlKey || event.altKey) {
    if (!isWordNavigation && !isWordDelete) {
      return null;
    }
  }
  return {
    key,
    code,
    ctrlKey: !!event.ctrlKey,
    altKey: !!event.altKey,
    metaKey: false,
  };
}

function applyInsertMacroEvent(view: EditorView, event: MacroInsertEvent): boolean {
  const key = event.key;
  const wordMotion = event.ctrlKey || event.altKey;
  if (key.length === 1 && !event.ctrlKey && !event.altKey && !event.metaKey) {
    const sel = view.state.selection.main;
    view.dispatch({
      changes: { from: sel.from, to: sel.to, insert: key },
      selection: { anchor: sel.from + key.length },
      scrollIntoView: true,
    });
    return true;
  }
  switch (key) {
    case "Home":
      return cursorLineStart(view);
    case "End":
      return cursorLineEnd(view);
    case "Enter": {
      const sel = view.state.selection.main;
      view.dispatch({
        changes: { from: sel.from, to: sel.to, insert: "\n" },
        selection: { anchor: sel.from + 1 },
        scrollIntoView: true,
      });
      return true;
    }
    case "Tab": {
      const sel = view.state.selection.main;
      view.dispatch({
        changes: { from: sel.from, to: sel.to, insert: "\t" },
        selection: { anchor: sel.from + 1 },
        scrollIntoView: true,
      });
      return true;
    }
    case "Backspace":
      if (wordMotion) return deleteGroupBackward(view);
      return deleteCharBackward(view);
    case "Delete":
      if (wordMotion) return deleteGroupForward(view);
      return deleteCharForward(view);
    case "ArrowUp":
      return cursorLineUp(view);
    case "ArrowDown":
      return cursorLineDown(view);
    case "ArrowLeft":
      if (wordMotion) return cursorGroupLeft(view);
      return cursorCharLeft(view);
    case "ArrowRight":
      if (wordMotion) return cursorGroupRight(view);
      return cursorCharRight(view);
    default:
      return true;
  }
}

interface VimLineColPosition {
  line: number;
  col: number;
}

function lineColFromPos(view: EditorView, pos: number): VimLineColPosition {
  const clamped = Math.max(0, Math.min(pos, view.state.doc.length));
  const line = view.state.doc.lineAt(clamped);
  return { line: line.number, col: clamped - line.from };
}

function posFromLineCol(view: EditorView, position: VimLineColPosition): number {
  const lineNo = Math.max(1, Math.min(position.line, view.state.doc.lines));
  const line = view.state.doc.line(lineNo);
  return Math.min(line.to, line.from + Math.max(0, position.col));
}

function restoreCursorWithoutHistory(view: EditorView, position: VimLineColPosition) {
  const anchor = posFromLineCol(view, position);
  view.dispatch({
    selection: { anchor },
    scrollIntoView: true,
    annotations: Transaction.addToHistory.of(false),
  });
}

function runUndoLikeTui(
  view: EditorView,
  count: number,
  emitStatus: (message: string) => void,
): boolean {
  let applied = 0;
  for (let i = 0; i < Math.max(1, count); i++) {
    const depthBefore = undoDepth(view.state);
    if (depthBefore <= 0) break;
    const keepCursorOnExhaust = depthBefore === 1;
    const cursorBefore = lineColFromPos(view, view.state.selection.main.head);
    if (!undo(view)) break;
    if (keepCursorOnExhaust) {
      restoreCursorWithoutHistory(view, cursorBefore);
    }
    applied += 1;
  }

  if (applied === 0) {
    emitStatus("already at oldest change");
    return true;
  }
  emitStatus(`undo (${undoDepth(view.state)} left)`);
  return true;
}

function runRedoLikeTui(
  view: EditorView,
  count: number,
  emitStatus: (message: string) => void,
): boolean {
  let applied = 0;
  for (let i = 0; i < Math.max(1, count); i++) {
    const depthBefore = redoDepth(view.state);
    if (depthBefore <= 0) break;
    if (!redo(view)) break;
    applied += 1;
  }
  if (applied === 0) {
    emitStatus("already at newest change");
    return true;
  }
  emitStatus(`redo (${redoDepth(view.state)} left)`);
  return true;
}

export const __vimUndoRedoInternals = {
  runUndoLikeTui,
  runRedoLikeTui,
};

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
  const session = new VimSession("normal");
  let currentMode: VimUiMode = "normal";
  let unnamedRegister: VimRegister | null = null;
  let macroRecordingRegister: string | null = null;
  const macroRegisters = new Map<string, MacroStep[]>();
  let macroReplaying = false;
  let pendingFoldPrefixUntilMs = 0;
  let pendingGoToLinkUntilMs = 0;

  options.onMacroRecordingChange?.(null);
  const emitVimStatusMessage =
    options.onVimStatusMessage ?? ((_message: string) => {});

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

  const runVerticalMoveCounted = (
    view: EditorView,
    deltaLines: number,
    explicitCount: number,
  ) => {
    const count = Number.isFinite(explicitCount) && explicitCount > 0 ? explicitCount : 1;
    const delta = Math.trunc(deltaLines * count);
    if (delta === 0) return true;

    const head = view.state.selection.main.head;
    const sourceLine = view.state.doc.lineAt(head);
    const goalCol = head - sourceLine.from;
    const targetLineNumber = Math.min(
      Math.max(sourceLine.number + delta, 1),
      view.state.doc.lines,
    );
    const targetLine = view.state.doc.line(targetLineNumber);
    const targetPos = Math.min(targetLine.from + goalCol, targetLine.to);
    view.dispatch({ selection: { anchor: targetPos }, scrollIntoView: true });
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
      const direction = command === cursorLineUp ? -1 : 1;
      const baseLine = visualHeadLine ?? getHeadInfo(view).lineNumber;
      const headLine = Math.min(
        Math.max(baseLine + direction * count, 1),
        view.state.doc.lines,
      );
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
    if (command === cursorLineUp || command === cursorLineDown) {
      const direction = command === cursorLineUp ? -1 : 1;
      runVerticalMoveCounted(view, direction, count);
    } else {
      runCounted(view, command, count);
    }
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

  const navigateWikiLinkAtCursor = (view: EditorView): boolean => {
    if (!options.onNavigateToNote) return false;
    const head = view.state.selection.main.head;
    const line = view.state.doc.lineAt(head);
    const link = markdownWikiLinkAtCursor(line.text, head - line.from);
    if (!link) return false;
    resolveWikiLink(link.shortId)
      .then((result) => {
        if (result) options.onNavigateToNote?.(result.id, link.heading ?? undefined);
      })
      .catch(() => {});
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

  const deleteWordEnd = (view: EditorView, count: number) => {
    const text = view.state.doc.toString();
    const from = view.state.selection.main.head;
    let target = from;
    let resolved = false;
    for (let i = 0; i < count; i++) {
      const next = moveWordEndPos(text, target);
      if (next === null) break;
      target = next;
      resolved = true;
    }
    if (!resolved) return false;
    const to = Math.min(text.length, target + 1);
    if (to <= from) return false;
    setRegister(text.slice(from, to));
    deleteRange(view, from, to);
    return true;
  };

  const deleteTillChar = (
    view: EditorView,
    count: number,
    targetChar: string | undefined,
  ) => {
    const target = targetChar?.slice(0, 1);
    if (!target) return false;
    const head = view.state.selection.main.head;
    const line = view.state.doc.lineAt(head);
    const rel = Math.max(0, Math.min(head - line.from, line.text.length));
    let searchFrom = rel;
    let matchIdx = -1;
    for (let i = 0; i < count; i++) {
      matchIdx = line.text.indexOf(target, searchFrom);
      if (matchIdx < 0) return false;
      searchFrom = matchIdx + target.length;
    }
    if (matchIdx <= rel) return false;
    const from = line.from + rel;
    const to = line.from + matchIdx;
    setRegister(view.state.sliceDoc(from, to));
    deleteRange(view, from, to);
    return true;
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

  const yankVisualSelection = (view: EditorView, forceLinewise = false) => {
    const linewise = forceLinewise || mode() === "visual-line";
    if (linewise) {
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

  const deleteVisualSelection = (view: EditorView, forceLinewise = false) => {
    const linewise = forceLinewise || mode() === "visual-line";
    if (linewise) {
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

  const applyAction = (
    view: EditorView,
    action: VimAction,
    sourceMode: VimUiMode = mode(),
  ) => {
    const count = action.count > 0 ? action.count : 1;
    if (!macroReplaying && macroRecordingRegister && isRecordableMacroIntent(action.intent)) {
      const macroSteps = macroRegisters.get(macroRecordingRegister) ?? [];
      macroSteps.push({
        kind: "action",
        action: {
          intent: action.intent,
          count,
          targetChar: action.targetChar,
        },
      });
      macroRegisters.set(macroRecordingRegister, macroSteps);
    }
    if (shouldExecuteSharedVimAction(action.intent)) {
      const sharedRegister: VimRegisterValue | null = unnamedRegister
        ? { text: unnamedRegister.text, mode: unnamedRegister.mode }
        : null;
      const sharedResult = executeSharedVimAction(
        view,
        action,
        count,
        sharedRegister,
      );
      if (sharedResult) {
        if (sharedResult.register) {
          setRegister(sharedResult.register.text, sharedResult.register.mode);
        }
        if (sharedResult.operations.length > 0) {
          applyEditOperations(view, sharedResult.operations);
        }
        return true;
      }
    }

    switch (action.intent) {
      case VIM_INTENT.START_MACRO_RECORD: {
        const register = (action.targetChar ?? "").toLowerCase();
        if (!register) {
          emitVimStatusMessage("macro register required");
          return true;
        }
        macroRecordingRegister = register;
        macroRegisters.set(register, []);
        options.onMacroRecordingChange?.(macroRecordingRegister);
        emitVimStatusMessage(`recording @${register}`);
        return true;
      }
      case VIM_INTENT.STOP_MACRO_RECORD: {
        if (!macroRecordingRegister) {
          emitVimStatusMessage("no active macro recording");
          return true;
        }
        const register = macroRecordingRegister;
        const steps = macroRegisters.get(register)?.length ?? 0;
        macroRecordingRegister = null;
        options.onMacroRecordingChange?.(null);
        emitVimStatusMessage(`recorded @${register} (${steps} steps)`);
        return true;
      }
      case VIM_INTENT.PLAY_MACRO: {
        if (macroReplaying) {
          emitVimStatusMessage("macro replay ignored while replaying");
          return true;
        }
        const register = (action.targetChar ?? "").toLowerCase();
        if (!register) {
          emitVimStatusMessage("macro register required");
          return true;
        }
        const sequence = macroRegisters.get(register);
        if (!sequence || sequence.length === 0) {
          emitVimStatusMessage(`macro @${register} is empty`);
          return true;
        }
        const totalSteps = count * sequence.length;
        if (!Number.isFinite(totalSteps) || totalSteps > VIM_MACRO_REPLAY_STEP_BUDGET) {
          emitVimStatusMessage(
            `macro @${register} replay aborted: step budget exceeded (>${VIM_MACRO_REPLAY_STEP_BUDGET})`,
          );
          return true;
        }
        macroReplaying = true;
        let executedSteps = 0;
        let aborted = false;
        for (let i = 0; i < count; i++) {
          for (let idx = 0; idx < sequence.length; idx++) {
            if (executedSteps >= VIM_MACRO_REPLAY_STEP_BUDGET) {
              aborted = true;
              break;
            }
            const step = sequence[idx];
            if (!step) {
              aborted = true;
              break;
            }
            if (step.kind === "action") {
              applyAction(view, step.action, mode());
              executedSteps += 1;
              continue;
            }
            const pipeline = runUiVimPipeline(session, step.event, {
              hasSearchMatches: editorSearchHasMatches(view),
              lineCount: view.state.doc.lines,
              macroRecording: macroRecordingRegister !== null,
            });
            if (pipeline.kind === "handled" || pipeline.kind === "unhandled") {
              currentMode = toUiMode(pipeline.step.mode);
              syncModeClasses(view);
              if (pipeline.kind === "handled") {
                for (const action of pipeline.step.actions) {
                  applyAction(view, action, mode());
                }
              }
            }
            if (mode() === "insert") {
              applyInsertMacroEvent(view, step.event);
            }
            executedSteps += 1;
          }
          if (aborted) {
            break;
          }
        }
        macroReplaying = false;
        if (aborted) {
          emitVimStatusMessage(
            `macro @${register} replay aborted: step budget exceeded (>${VIM_MACRO_REPLAY_STEP_BUDGET})`,
          );
        } else {
          emitVimStatusMessage(`replayed @${register} x${count}`);
        }
        return true;
      }
      case VIM_INTENT.MOVE_LEFT:
        return runMove(view, cursorCharLeft, count);
      case VIM_INTENT.MOVE_RIGHT:
        return runMove(view, cursorCharRight, count);
      case VIM_INTENT.MOVE_UP:
        return runMove(view, cursorLineUp, count);
      case VIM_INTENT.MOVE_DOWN:
        return runMove(view, cursorLineDown, count);
      case VIM_INTENT.MOVE_WORD_FORWARD:
        return runMove(view, vimMoveWordForward, count);
      case VIM_INTENT.MOVE_WORD_BACKWARD:
        return runMove(view, vimMoveWordBackward, count);
      case VIM_INTENT.MOVE_LINE_START:
        return runMove(view, cursorLineStart, count);
      case VIM_INTENT.MOVE_LINE_END:
        return runMove(view, moveToLineEndNormalLike, count);
      case VIM_INTENT.MOVE_DOC_START:
        moveToDocStart(view);
        if (mode() === "visual" || mode() === "visual-line") {
          updateVisualSelection(view);
        }
        return true;
      case VIM_INTENT.MOVE_DOC_END:
        moveToDocEnd(view);
        if (mode() === "visual" || mode() === "visual-line") {
          updateVisualSelection(view);
        }
        return true;
      case VIM_INTENT.MOVE_TO_LINE: {
        const lineNo = Math.min(Math.max(count, 1), view.state.doc.lines);
        const pos = view.state.doc.line(lineNo).from;
        view.dispatch({ selection: { anchor: pos }, scrollIntoView: true });
        if (mode() === "visual" || mode() === "visual-line") {
          updateVisualSelection(view);
        }
        return true;
      }
      case VIM_INTENT.ENTER_INSERT:
        return true;
      case VIM_INTENT.APPEND_INSERT:
        return appendInsertWithinLine(view);
      case VIM_INTENT.INSERT_LINE_START:
        return cursorLineStart(view);
      case VIM_INTENT.APPEND_LINE_END:
        return cursorLineEnd(view);
      case VIM_INTENT.OPEN_LINE_BELOW:
        return insertLineBelow(view);
      case VIM_INTENT.OPEN_LINE_ABOVE:
        return insertLineAbove(view);
      case VIM_INTENT.ENTER_VISUAL:
        startVisualMode(view, "visual");
        return true;
      case VIM_INTENT.ENTER_VISUAL_LINE:
        startVisualMode(view, "visual-line");
        return true;
      case VIM_INTENT.EXIT_VISUAL:
        resetVisualAnchors();
        collapseSelection(view);
        return true;
      case VIM_INTENT.DELETE_LINE:
        return deleteCurrentLines(view, count);
      case VIM_INTENT.YANK_LINE:
        return yankCurrentLines(view, count);
      case VIM_INTENT.DELETE_TO_LINE_START:
        return deleteToLineStart(view, count);
      case VIM_INTENT.DELETE_TO_LINE_END:
        return deleteToLineEnd(view, count);
      case VIM_INTENT.YANK_TO_LINE_START:
        return yankToLineStart(view, count);
      case VIM_INTENT.YANK_TO_LINE_END:
        return yankToLineEnd(view, count);
      case VIM_INTENT.DELETE_CHAR:
        return runCounted(view, deleteCharForward, count);
      case VIM_INTENT.PASTE_AFTER:
        return pasteAfter(view, count);
      case VIM_INTENT.UNDO:
        return runUndoLikeTui(view, count, emitVimStatusMessage);
      case VIM_INTENT.REDO:
        return runRedoLikeTui(view, count, emitVimStatusMessage);
      case VIM_INTENT.OPEN_COMMAND_BAR:
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
          onWriteCommand: options.onWriteCommand,
          onExitCommand: options.onExitCommand,
          onExportCommand: options.onExportCommand,
          onBackupCommand: options.onBackupCommand,
          onClipWatchStateChange: options.onClipWatchStateChange,
          onClipWatchPaste: options.onClipWatchPaste,
          getNoteModules: options.getNoteModules,
          setNoteModules: options.setNoteModules,
          source: "vim-colon",
          onCancel: () => {
            currentMode = "normal";
            resetVisualAnchors();
            collapseSelection(view);
            syncModeClasses(view);
          },
          selectionOverride: preservedSelection,
        });
        return true;
      case VIM_INTENT.OPEN_SEARCH:
        return openEditorSearch(view);
      case VIM_INTENT.SEARCH_NEXT:
        return editorSearchNext(view);
      case VIM_INTENT.SEARCH_PREV:
        return editorSearchPrev(view);
      case VIM_INTENT.DELETE_INSIDE_WORD:
        return applyTextObject(view, "word", false, true, count) > 0;
      case VIM_INTENT.DELETE_AROUND_WORD:
        return applyTextObject(view, "word", true, true, count) > 0;
      case VIM_INTENT.YANK_INSIDE_WORD:
        return applyTextObject(view, "word", false, false, count) > 0;
      case VIM_INTENT.YANK_AROUND_WORD:
        return applyTextObject(view, "word", true, false, count) > 0;
      case VIM_INTENT.DELETE_INSIDE_PIPE:
        return applyTextObject(view, "pipe", false, true, count) > 0;
      case VIM_INTENT.DELETE_AROUND_PIPE:
        return applyTextObject(view, "pipe", true, true, count) > 0;
      case VIM_INTENT.YANK_INSIDE_PIPE:
        return applyTextObject(view, "pipe", false, false, count) > 0;
      case VIM_INTENT.YANK_AROUND_PIPE:
        return applyTextObject(view, "pipe", true, false, count) > 0;
      case VIM_INTENT.SWALLOW:
        return true;
      case VIM_INTENT.DELETE_WORD_FORWARD:
        return deleteWordForward(view, count);
      case VIM_INTENT.DELETE_WORD_BACKWARD:
        return deleteWordBackward(view, count);
      case VIM_INTENT.DELETE_WORD_END:
        return deleteWordEnd(view, count);
      case VIM_INTENT.DELETE_TILL_CHAR:
        return deleteTillChar(view, count, action.targetChar);
      case VIM_INTENT.YANK_WORD_FORWARD:
        return yankWordForward(view, count);
      case VIM_INTENT.YANK_WORD_BACKWARD:
        return yankWordBackward(view, count);
      case VIM_INTENT.YANK_VISUAL_SELECTION:
        return yankVisualSelection(view, sourceMode === "visual-line");
      case VIM_INTENT.DELETE_VISUAL_SELECTION:
        return deleteVisualSelection(view, sourceMode === "visual-line");
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
      const macroPendingBefore = session.macroPendingKind();
      const now = Date.now();
      if (pendingFoldPrefixUntilMs > 0 && now > pendingFoldPrefixUntilMs) {
        pendingFoldPrefixUntilMs = 0;
      }
      if (pendingGoToLinkUntilMs > 0 && now > pendingGoToLinkUntilMs) {
        pendingGoToLinkUntilMs = 0;
      }

      if (activeMode !== "normal") {
        pendingFoldPrefixUntilMs = 0;
        pendingGoToLinkUntilMs = 0;
      }

      // Let plain arrow navigation flow through to CodeMirror/markdown table
      // handlers so cursor movement works the same in normal and insert modes.
      if (
        activeMode === "normal"
        && !event.ctrlKey
        && !event.metaKey
        && !event.altKey
        && !event.shiftKey
        && (
          event.key === "ArrowUp"
          || event.key === "ArrowDown"
          || event.key === "ArrowLeft"
          || event.key === "ArrowRight"
          || event.key === "Up"
          || event.key === "Down"
          || event.key === "Left"
          || event.key === "Right"
        )
      ) {
        return false;
      }

      const plain = !event.ctrlKey && !event.altKey && !event.metaKey;
      if (
        activeMode === "normal"
        && plain
        && event.key === "Q"
      ) {
        event.preventDefault();
        emitVimStatusMessage(macroRegisterSummary(macroRegisters));
        return true;
      }
      if (
        activeMode === "normal"
        && plain
        && event.key.toLowerCase() === "q"
        && event.repeat
      ) {
        // Browser key-repeat can accidentally retrigger q and start a new
        // pending recording right after stopping one.
        event.preventDefault();
        return true;
      }
      if (
        activeMode === "normal"
        && plain
        && event.key.toLowerCase() === "q"
        && macroRecordingRegister
      ) {
        // Keep UI behavior stable even if session/pending state drifts.
        event.preventDefault();
        applyAction(
          view,
          { intent: VIM_INTENT.STOP_MACRO_RECORD, count: 1 },
          activeMode,
        );
        return true;
      }
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

        if (plainKey === "g") {
          pendingGoToLinkUntilMs = now + 900;
        } else if (plainKey !== "d") {
          pendingGoToLinkUntilMs = 0;
        }
      }

      // Insert-mode fast path: avoid wasm roundtrip for regular insert editing.
      if (activeMode === "insert" && event.key !== "Escape") {
        if (!macroReplaying && macroRecordingRegister) {
          const insertEvent = toRecordableInsertMacroEvent(event);
          if (insertEvent) {
            const macroSteps = macroRegisters.get(macroRecordingRegister) ?? [];
            macroSteps.push({ kind: "insert_event", event: insertEvent });
            macroRegisters.set(macroRecordingRegister, macroSteps);
          }
        }
        return false;
      }
      if (
        activeMode === "insert" &&
        event.key === "Escape" &&
        !macroReplaying &&
        macroRecordingRegister
      ) {
        const insertEvent = toRecordableInsertMacroEvent(event);
        if (insertEvent) {
          const macroSteps = macroRegisters.get(macroRecordingRegister) ?? [];
          macroSteps.push({ kind: "insert_event", event: insertEvent });
          macroRegisters.set(macroRecordingRegister, macroSteps);
        }
      }

      const pipeline = runUiVimPipeline(session, event, {
        hasSearchMatches: editorSearchHasMatches(view),
        lineCount: view.state.doc.lines,
        macroRecording: macroRecordingRegister !== null,
      });
      if (pipeline.kind === "no_step") {
        if (event.key === "Escape" && macroPendingBefore === VIM_MACRO_PENDING_RECORD) {
          emitVimStatusMessage("macro record canceled");
        } else if (event.key === "Escape" && macroPendingBefore === VIM_MACRO_PENDING_PLAY) {
          emitVimStatusMessage("macro replay canceled");
        } else if (
          activeMode === "normal"
          && macroPendingBefore === VIM_MACRO_PENDING_RECORD
          && plain
          && event.key.length === 1
          && !isMacroRegisterChar(event.key)
        ) {
          emitVimStatusMessage("invalid macro register: use [a-z0-9]");
        } else if (
          activeMode === "normal"
          && macroPendingBefore === VIM_MACRO_PENDING_PLAY
          && plain
          && event.key.length === 1
          && !isMacroRegisterChar(event.key)
        ) {
          emitVimStatusMessage("invalid macro register: use [a-z0-9]");
        }
        if (activeMode !== "insert" || event.key === "Escape") {
          event.preventDefault();
          return true;
        }
        return false;
      }
      if (pipeline.kind === "no_intent") {
        if (event.key === "Escape" && macroPendingBefore === VIM_MACRO_PENDING_RECORD) {
          emitVimStatusMessage("macro record canceled");
        } else if (event.key === "Escape" && macroPendingBefore === VIM_MACRO_PENDING_PLAY) {
          emitVimStatusMessage("macro replay canceled");
        } else if (
          activeMode === "normal"
          && (macroPendingBefore === VIM_MACRO_PENDING_RECORD || macroPendingBefore === VIM_MACRO_PENDING_PLAY)
          && plain
          && event.key.length === 1
          && !isMacroRegisterChar(event.key)
        ) {
          emitVimStatusMessage("invalid macro register: use [a-z0-9]");
        }
        if (activeMode !== "insert" && shouldSwallowInNormalLikeMode(event)) {
          event.preventDefault();
          return true;
        }
        return false;
      }

      const step = pipeline.step;
      if (
        activeMode === "normal"
        && (macroPendingBefore === VIM_MACRO_PENDING_RECORD || macroPendingBefore === VIM_MACRO_PENDING_PLAY)
        && plain
        && event.key.length === 1
        && !isMacroRegisterChar(event.key)
        && step.actions.length > 0
        && step.actions.every((action) => action.intent === VIM_INTENT.SWALLOW)
      ) {
        emitVimStatusMessage("invalid macro register: use [a-z0-9]");
      }
      currentMode = toUiMode(step.mode);
      syncModeClasses(view);

      if (
        activeMode === "normal"
        && plain
        && event.key.toLowerCase() === "d"
        && pendingGoToLinkUntilMs > 0
      ) {
        pendingGoToLinkUntilMs = 0;
        if (navigateWikiLinkAtCursor(view)) {
          event.preventDefault();
          return true;
        }
      }

      if (pipeline.kind === "unhandled") {
        if (mode() !== "insert" && shouldSwallowInNormalLikeMode(event)) {
          event.preventDefault();
          return true;
        }
        return false;
      }

      event.preventDefault();
      const sourceMode = activeMode;
      for (const action of step.actions) {
        applyAction(view, action, sourceMode);
      }

      if (mode() === "visual" || mode() === "visual-line") {
        updateVisualSelection(view);
      } else {
        resetVisualAnchors();
      }

      return true;
    },
  });

  const lifecycle = ViewPlugin.fromClass(
    class {
      destroy() {
        // Reconfiguration or teardown must clear UI recording badges.
        options.onMacroRecordingChange?.(null);
      }
    },
  );

  return [handlers, focusSync, lifecycle];
}
