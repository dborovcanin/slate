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
  undo,
} from "@codemirror/commands";
import { EditorView } from "@codemirror/view";
import { computeBlockSpans } from "./vim-utils";
import { executeExCommand } from "./ex-commands";
import { openDatePicker } from "./date-picker";

type VimMode = "insert" | "normal" | "visual" | "visual-line" | "visual-block";

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

function copyToClipboard(text: string) {
  if (!navigator.clipboard) return;
  void navigator.clipboard.writeText(text).catch((err) => {
    console.error("Vim yank failed:", err);
  });
}

interface VimOptions {
  dateFormat?: string;
}

export function vimModeExtension(options: VimOptions = {}) {
  let mode: VimMode = "insert";
  let pendingDelete = false;
  let pendingGo = false;
  let pendingYank = false;
  let countBuffer = "";
  let visualAnchorPos: number | null = null;
  let visualAnchorLine: number | null = null; // 1-based
  let blockAnchor: { line: number; col: number } | null = null; // line: 1-based
  let commandBarEl: HTMLDivElement | null = null;
  let commandInputEl: HTMLInputElement | null = null;
  let statusTimer: number | null = null;

  const clearPending = () => {
    pendingDelete = false;
    pendingGo = false;
    pendingYank = false;
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

  const showStatus = (view: EditorView, message: string) => {
    const host = view.dom;
    let statusEl = host.querySelector(".vim-command-status") as HTMLDivElement | null;
    if (!statusEl) {
      statusEl = document.createElement("div");
      statusEl.className = "vim-command-status";
      host.appendChild(statusEl);
    }
    statusEl.textContent = message;
    statusEl.classList.add("visible");
    if (statusTimer !== null) window.clearTimeout(statusTimer);
    statusTimer = window.setTimeout(() => {
      statusEl?.classList.remove("visible");
    }, 1800);
  };

  const closeCommandBar = (view: EditorView) => {
    if (commandBarEl) {
      commandBarEl.remove();
      commandBarEl = null;
      commandInputEl = null;
      view.focus();
    }
  };

  const openCommandBar = (view: EditorView) => {
    if (commandBarEl) return;
    const bar = document.createElement("div");
    bar.className = "vim-command-bar";

    const prefix = document.createElement("span");
    prefix.className = "vim-command-prefix";
    prefix.textContent = ":";

    const input = document.createElement("input");
    input.className = "vim-command-input";
    input.type = "text";
    input.spellcheck = false;
    input.autocapitalize = "off";
    input.autocomplete = "off";
    input.autocorrect = false;

    bar.appendChild(prefix);
    bar.appendChild(input);
    view.dom.appendChild(bar);
    commandBarEl = bar;
    commandInputEl = input;

    input.addEventListener("keydown", (e) => {
      if (e.key === "Escape") {
        e.preventDefault();
        closeCommandBar(view);
        return;
      }

      if (e.key === "Enter") {
        e.preventDefault();
        const command = input.value.trim();
        closeCommandBar(view);
        if (!command) return;

        if (command.toLowerCase().replace(/^:/, "") === "date") {
          void openDatePicker(options.dateFormat ?? "%Y-%m-%d")
            .then((value) => {
              if (!value) {
                showStatus(view, "date cancelled");
                return;
              }
              insertAtSelection(view, value);
              showStatus(view, `inserted ${value}`);
            })
            .catch((err) => {
              console.error("Date command failed:", err);
              showStatus(view, "date command failed");
            });
          return;
        }

        void executeExCommand(view, command)
          .then((message) => {
            if (message) showStatus(view, message);
          })
          .catch((err) => {
            console.error("Vim command failed:", err);
            showStatus(view, "command failed");
          });
      }
    });

    window.setTimeout(() => input.focus(), 0);
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

  const runMove = (
    view: EditorView,
    command: (target: EditorView) => boolean,
    explicitCount?: number,
  ) => {
    runCounted(view, command, explicitCount);
    if (mode === "visual" || mode === "visual-line" || mode === "visual-block") {
      updateVisualSelection(view);
    }
    return true;
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

  const insertAtSelection = (view: EditorView, text: string) => {
    const main = view.state.selection.main;
    view.dispatch({
      changes: { from: main.from, to: main.to, insert: text },
      selection: { anchor: main.from + text.length },
      scrollIntoView: true,
    });
  };

  return EditorView.domEventHandlers({
    focus: (_event, view) => {
      updateModeClasses(view, mode);
      return false;
    },
    keydown: (event, view) => {
      if (view.dom.querySelector(".command-picker-bar")) {
        return false;
      }
      if (commandBarEl) {
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
        openCommandBar(view);
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
      if (pendingDelete) {
        clearPending();
        if (event.key === "d") {
          event.preventDefault();
          return runCounted(view, deleteLine);
        }
      }

      if (pendingYank) {
        clearPending();
        if (event.key === "y") {
          event.preventDefault();
          return yankCurrentLines(view);
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
            if (mode === "visual" || mode === "visual-line" || mode === "visual-block") {
              updateVisualSelection(view);
            }
            return true;
          }
          const ok = moveToDocStart(view);
          if (mode === "visual" || mode === "visual-line" || mode === "visual-block") {
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
        case "$":
          event.preventDefault();
          return runMove(view, cursorLineEnd);
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
            if (mode === "visual" || mode === "visual-line" || mode === "visual-block") {
              updateVisualSelection(view);
            }
            return true;
          }
          const ok = moveToDocEnd(view);
          if (mode === "visual" || mode === "visual-line" || mode === "visual-block") {
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

      return false;
    },
  });
}
