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
import { isCommandPickerOpen, openCommandPicker } from "./command-picker";
import {
  VimSession,
  VIM_KEY_KIND,
  type VimAction,
  type VimKeyInput,
  type VimMode,
} from "./wasm.ts";

type VimUiMode = "insert" | "normal" | "visual" | "visual-line";

interface VimOptions {
  dateFormat?: string;
  onExitCommand?: () => Promise<void> | void;
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

  let visualAnchorPos: number | null = null;
  let visualAnchorLine: number | null = null; // 1-based

  const mode = (): VimUiMode => currentMode;

  const syncModeClasses = (view: EditorView) => {
    updateModeClasses(view, currentMode);
  };

  const collapseSelection = (view: EditorView) => {
    const head = view.state.selection.main.head;
    view.dispatch({ selection: { anchor: head }, scrollIntoView: true });
  };

  const resetVisualAnchors = () => {
    visualAnchorPos = null;
    visualAnchorLine = null;
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
    const { lineNumber } = getHeadInfo(view);
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
    } else {
      visualAnchorLine = info.lineNumber;
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
    runCounted(view, command, explicitCount);
    if (isVisual) {
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
      copyToClipboard(chunks.join("\n"));
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
      copyToClipboard(chunks.join("\n"));
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
      copyToClipboard(chunks.join("\n"));
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
      copyToClipboard(parts.join("\n"));
      return true;
    }

    const text = view.state.sliceDoc(view.state.selection.main.from, view.state.selection.main.to);
    copyToClipboard(text);
    return true;
  };

  const yankCurrentLines = (view: EditorView, count: number) => {
    const current = view.state.doc.lineAt(view.state.selection.main.head).number;
    const end = Math.min(view.state.doc.lines, current + count - 1);
    const parts: string[] = [];
    for (let lineNo = current; lineNo <= end; lineNo++) {
      parts.push(view.state.doc.line(lineNo).text);
    }
    copyToClipboard(parts.join("\n"));
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
        return runMove(view, cursorGroupForward, count);
      case 5: // move_word_backward
        return runMove(view, cursorGroupBackward, count);
      case 6: // move_line_start
        return runMove(view, cursorLineStart, count);
      case 7: // move_line_end
        return runMove(view, cursorLineEnd, count);
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
        return cursorCharRight(view);
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
        return runCounted(view, deleteLine, count);
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
        return true;
      case 28: // undo
        return runCounted(view, undo, count);
      case 29: // redo
        return runCounted(view, redo, count);
      case 30: // open_command_bar
        openCommandPicker(view, {
          mode: "vim",
          dateFormat: options.dateFormat,
          onExitCommand: options.onExitCommand,
          source: "vim-colon",
        });
        return true;
      case 31: // open_search
      case 32: // search_next
      case 33: // search_prev
        return true;
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
      default:
        return true;
    }
  };

  return EditorView.domEventHandlers({
    focus: (_event, view) => {
      syncModeClasses(view);
      return false;
    },
    keydown: (event, view) => {
      if (isCommandPickerOpen(view)) {
        return false;
      }

      const activeMode = mode();

      // Hard guarantee for visual behavior: both hjkl and arrow keys move the
      // selection, and Esc exits to normal in a single press.
      if (activeMode === "visual" || activeMode === "visual-line") {
        if (event.key === "Escape" || event.key === "Esc" || event.code === "Escape") {
          event.preventDefault();
          setModeLocally(view, "normal");
          session.step({ kind: VIM_KEY_KIND.ESC }, { line_count: view.state.doc.lines });
          return true;
        }

        const plain = !event.ctrlKey && !event.altKey && !event.metaKey;
        if (plain) {
          if (event.key === "h" || event.key === "ArrowLeft" || event.key === "Left") {
            event.preventDefault();
            return runMove(view, cursorCharLeft, 1);
          }
          if (event.key === "l" || event.key === "ArrowRight" || event.key === "Right") {
            event.preventDefault();
            return runMove(view, cursorCharRight, 1);
          }
          if (event.key === "k" || event.key === "ArrowUp" || event.key === "Up") {
            event.preventDefault();
            return runMove(view, cursorLineUp, 1);
          }
          if (event.key === "j" || event.key === "ArrowDown" || event.key === "Down") {
            event.preventDefault();
            return runMove(view, cursorLineDown, 1);
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
        has_search_matches: false,
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
}
