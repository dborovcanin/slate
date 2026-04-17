import { Prec } from "@codemirror/state";
import { EditorView, keymap, ViewPlugin, type KeyBinding } from "@codemirror/view";
import type { ViewUpdate } from "@codemirror/view";
import { getCalcResultAtCursor } from "./calc-decoration.ts";
import { applyEditOperation, snapshotFromUpdate, snapshotFromView } from "./core/codemirror-adapter.ts";
import {
  markdownClassifyLine,
  runDocChangeRules,
  runEnterRules,
  runTableBoundaryEditRules,
  runTableCellNavigationRules,
  runTableHeaderDeleteColumnRule,
  runTablePipeInsertColumnRule,
  runTabRules,
  rewriteLineWithChecklistToggleSuffix,
} from "./wasm.ts";

export { formatTableLines } from "./core/markdown-table.ts";
export { rewriteLineWithChecklistToggleSuffix };

interface TableCellInfo {
  index: number;
  cellCount: number;
  leftPipe: number;
  rightPipe: number;
  trimStart: number;
  trimEnd: number;
}

function isMarkdownTableLine(text: string): boolean {
  const trimmed = text.trim();
  return trimmed.startsWith("|") && trimmed.endsWith("|");
}

function firstNonSpaceOffset(text: string): number {
  for (let i = 0; i < text.length; i += 1) {
    if (text[i] !== " ") return i;
  }
  return text.length;
}

function lastNonSpaceEndOffset(text: string): number {
  for (let i = text.length - 1; i >= 0; i -= 1) {
    if (text[i] !== " ") return i + 1;
  }
  return 0;
}

function tableCellAtColumn(lineText: string, col: number): TableCellInfo | null {
  if (!isMarkdownTableLine(lineText)) return null;
  const colInLine = Math.max(0, Math.min(col, lineText.length));
  let prevPipe = -1;
  let cellIndex = 0;
  let selected: TableCellInfo | null = null;
  let fallback: TableCellInfo | null = null;

  for (let i = 0; i < lineText.length; i += 1) {
    if (lineText[i] !== "|") continue;
    if (prevPipe >= 0) {
      const raw = lineText.slice(prevPipe + 1, i);
      const cell: TableCellInfo = {
        index: cellIndex,
        cellCount: 0,
        leftPipe: prevPipe,
        rightPipe: i,
        trimStart: firstNonSpaceOffset(raw),
        trimEnd: lastNonSpaceEndOffset(raw),
      };
      if (selected === null && colInLine <= i) {
        selected = cell;
      }
      fallback = cell;
      cellIndex += 1;
    }
    prevPipe = i;
  }

  const picked = selected ?? fallback;
  if (!picked) return null;
  picked.cellCount = cellIndex;
  return picked;
}

function tableCellNavigationAnchorInLine(cell: TableCellInfo): number {
  const cellStart = cell.leftPipe + 1;
  if (cell.trimEnd <= cell.trimStart) {
    return Math.min(cellStart + 1, cell.rightPipe);
  }
  return Math.min(cellStart + cell.trimEnd, cell.rightPipe);
}

function isSingleSpaceInsertion(update: ViewUpdate): boolean {
  let inserted = "";
  let changeCount = 0;
  let hasDeletion = false;
  for (const tr of update.transactions) {
    tr.changes.iterChanges((fromA, toA, _fromB, _toB, text) => {
      changeCount += 1;
      if (fromA !== toA) hasDeletion = true;
      inserted += text.toString();
    });
  }
  return !hasDeletion && changeCount === 1 && inserted === " ";
}

function shouldDeferTableAutoformatForSpace(update: ViewUpdate): boolean {
  if (!update.docChanged) return false;
  if (!isSingleSpaceInsertion(update)) return false;
  const main = update.state.selection.main;
  if (!main.empty) return false;
  const line = update.state.doc.lineAt(main.head);
  return isMarkdownTableLine(line.text);
}

function clampTableCursorToContent(state: EditorView["state"], pos: number): number | null {
  const line = state.doc.lineAt(pos);
  if (!isMarkdownTableLine(line.text)) return null;
  const cell = tableCellAtColumn(line.text, pos - line.from);
  if (!cell) return null;

  const cellStart = cell.leftPipe + 1;
  const contentStart = Math.min(cellStart + 1, cell.rightPipe);
  const contentEnd = cellStart + cell.trimEnd;
  const anchor = line.from + tableCellNavigationAnchorInLine(cell);
  const colInLine = pos - line.from;

  if (cell.trimEnd <= cell.trimStart) {
    return anchor;
  }
  if (colInLine < contentStart || colInLine > contentEnd) {
    return anchor;
  }
  return null;
}

function toggleWrap(view: EditorView, left: string, right = left): boolean {
  const main = view.state.selection.main;
  const from = main.from;
  const to = main.to;

  if (main.empty) {
    const insert = left + right;
    view.dispatch({
      changes: { from, to, insert },
      selection: { anchor: from + left.length },
      scrollIntoView: true,
    });
    return true;
  }

  const before = from >= left.length ? view.state.sliceDoc(from - left.length, from) : "";
  const after =
    to + right.length <= view.state.doc.length ? view.state.sliceDoc(to, to + right.length) : "";
  const isWrapped = before === left && after === right;

  if (isWrapped) {
    view.dispatch({
      changes: [
        { from: to, to: to + right.length, insert: "" },
        { from: from - left.length, to: from, insert: "" },
      ],
      selection: { anchor: from - left.length, head: to - left.length },
      scrollIntoView: true,
    });
    return true;
  }

  view.dispatch({
    changes: [
      { from, to: from, insert: left },
      { from: to, to, insert: right },
    ],
    selection: { anchor: from + left.length, head: to + left.length },
    scrollIntoView: true,
  });
  return true;
}

function wrapLink(view: EditorView): boolean {
  const main = view.state.selection.main;
  const selected = view.state.sliceDoc(main.from, main.to);
  if (main.empty) {
    const text = "[text](url)";
    view.dispatch({
      changes: { from: main.from, to: main.to, insert: text },
      selection: { anchor: main.from + 1, head: main.from + 5 },
      scrollIntoView: true,
    });
    return true;
  }

  const link = `[${selected}](url)`;
  view.dispatch({
    changes: { from: main.from, to: main.to, insert: link },
    selection: { anchor: main.from + selected.length + 3, head: main.from + selected.length + 6 },
    scrollIntoView: true,
  });
  return true;
}

function continueListOnEnter(view: EditorView, autoformat: boolean): boolean {
  const operation = runEnterRules(snapshotFromView(view), {
    markdownAutoformat: autoformat,
  });
  if (!operation) return false;
  applyEditOperation(view, operation);
  return true;
}

function indentListOnTab(view: EditorView, autoformat: boolean, outdent = false): boolean {
  if (!autoformat) return false;

  // Prefer calc Tab-apply behavior when a ghost result is available.
  if (!outdent && getCalcResultAtCursor(view) !== null) return false;

  const operation = runTabRules(snapshotFromView(view), {
    markdownAutoformat: autoformat,
    outdent,
  });
  if (!operation) return false;
  applyEditOperation(view, operation);
  return true;
}

function tableArrowMove(view: EditorView, direction: -1 | 1): boolean {
  const main = view.state.selection.main;
  if (!main.empty) return false;
  const line = view.state.doc.lineAt(main.head);
  const cell = tableCellAtColumn(line.text, main.head - line.from);
  if (!cell) return false;

  const cellStart = cell.leftPipe + 1;
  const contentStart = line.from + Math.min(cellStart + 1, cell.rightPipe);
  const contentEnd = line.from + tableCellNavigationAnchorInLine(cell);
  let target = main.head;

  if (cell.trimEnd <= cell.trimStart) {
    target = contentEnd;
  } else if (main.head > contentEnd) {
    target = contentEnd;
  } else if (direction === -1) {
    if (main.head > contentStart) {
      target = main.head - 1;
    } else {
      target = contentStart;
    }
  } else if (main.head < contentEnd) {
    target = main.head + 1;
  } else {
    target = contentEnd;
  }

  if (target !== main.head) {
    view.dispatch({
      selection: { anchor: target },
      scrollIntoView: true,
    });
  }
  return true;
}

function tableBoundaryEdit(
  view: EditorView,
  autoformat: boolean,
  backward: boolean,
  structuralMerge = false,
): boolean {
  const operation = runTableBoundaryEditRules(snapshotFromView(view), {
    markdownAutoformat: autoformat,
    backward,
    structuralMerge,
  });
  if (!operation) return false;
  applyEditOperation(view, operation);
  return true;
}

function tablePipeInsertColumn(view: EditorView): boolean {
  const operation = runTablePipeInsertColumnRule(snapshotFromView(view));
  if (!operation) return false;
  applyEditOperation(view, operation);
  return true;
}

function tablePipeInputHandler() {
  return EditorView.inputHandler.of((view, _from, _to, insert) => {
    if (insert !== "|") return false;
    return tablePipeInsertColumn(view);
  });
}

function tableHeaderDeleteColumn(view: EditorView): boolean {
  const operation = runTableHeaderDeleteColumnRule(snapshotFromView(view));
  if (!operation) return false;
  applyEditOperation(view, operation);
  return true;
}

export function runTableHeaderDeleteColumnCommand(view: EditorView): boolean {
  return tableHeaderDeleteColumn(view);
}

function tableCellJump(view: EditorView, autoformat: boolean, outdent: boolean): boolean {
  const operation = runTableCellNavigationRules(snapshotFromView(view), {
    markdownAutoformat: autoformat,
    outdent,
  });
  if (!operation) return false;
  applyEditOperation(view, operation);
  return true;
}

function markdownShortcutKeymap(autoformat: boolean): KeyBinding[] {
  const keys: KeyBinding[] = [
    { key: "Mod-b", preventDefault: true, run: (view) => toggleWrap(view, "**") },
    { key: "Mod-i", preventDefault: true, run: (view) => toggleWrap(view, "*") },
    { key: "Mod-Shift-x", preventDefault: true, run: (view) => toggleWrap(view, "~~") },
    { key: "Mod-k", preventDefault: true, run: wrapLink },
  ];

  if (autoformat) {
    keys.push({
      key: "Enter",
      run: (view) => continueListOnEnter(view, autoformat),
      preventDefault: true,
    });
  }

  return keys;
}

function markdownTabKeymap(autoformat: boolean): KeyBinding[] {
  return [
    {
      key: "Tab",
      preventDefault: true,
      run: (view) => indentListOnTab(view, autoformat, false),
    },
    {
      key: "Shift-Tab",
      preventDefault: true,
      run: (view) => indentListOnTab(view, autoformat, true),
    },
  ];
}

function tableCursorKeymap(autoformat: boolean): KeyBinding[] {
  return [
    {
      key: "Backspace",
      run: (view) => tableBoundaryEdit(view, autoformat, true),
    },
    {
      key: "Delete",
      run: (view) => tableBoundaryEdit(view, autoformat, false),
    },
    {
      key: "Ctrl-Backspace",
      preventDefault: true,
      run: (view) => tableHeaderDeleteColumn(view) || tableBoundaryEdit(view, autoformat, true, true),
    },
    {
      key: "Ctrl-Delete",
      preventDefault: true,
      run: (view) => tableHeaderDeleteColumn(view) || tableBoundaryEdit(view, autoformat, false, true),
    },
    {
      key: "ArrowLeft",
      run: (view) => tableArrowMove(view, -1),
    },
    {
      key: "ArrowRight",
      run: (view) => tableArrowMove(view, 1),
    },
    {
      key: "Ctrl-ArrowLeft",
      preventDefault: true,
      run: (view) => tableCellJump(view, autoformat, true),
    },
    {
      key: "Ctrl-ArrowRight",
      preventDefault: true,
      run: (view) => tableCellJump(view, autoformat, false),
    },
  ];
}

function textRulesPlugin(autoformat: boolean, checklistAutoReorder: boolean) {
  return ViewPlugin.define(() => {
    let applying = false;
    return {
      update(update: ViewUpdate) {
        if (applying || !update.docChanged) return;
        if (shouldDeferTableAutoformatForSpace(update)) return;
        applying = true;
        try {
          const operation = runDocChangeRules(snapshotFromUpdate(update), {
            markdownAutoformat: autoformat,
            checklistAutoReorder,
          });
          if (operation) {
            Promise.resolve().then(() => {
              applying = true;
              try {
                applyEditOperation(update.view, operation);
              } catch (error) {
                console.error("Markdown text rule dispatch failed:", error);
              } finally {
                applying = false;
              }
            });
            return;
          }
        } catch (error) {
          console.error("Markdown text rule failed:", error);
        } finally {
          applying = false;
        }
      },
    };
  });
}

function tableCursorGuards() {
  return ViewPlugin.define(() => {
    let syncing = false;
    let lastCellKey: string | null = null;
    return {
      update(update: ViewUpdate) {
        if (syncing || update.docChanged || !update.selectionSet) return;
        const main = update.state.selection.main;
        if (!main.empty) {
          lastCellKey = null;
          return;
        }
        const line = update.state.doc.lineAt(main.head);
        const cell = tableCellAtColumn(line.text, main.head - line.from);
        if (!cell) {
          lastCellKey = null;
          return;
        }

        const cellKey = `${line.from}:${cell.index}`;
        const pointerSelection = update.transactions.some((tr) => tr.isUserEvent("select.pointer"));
        const anchor = line.from + tableCellNavigationAnchorInLine(cell);
        const clamped = clampTableCursorToContent(update.state, main.head);
        const shouldSnapToEnd = clamped === null && (lastCellKey !== cellKey || pointerSelection);
        const target = clamped ?? (shouldSnapToEnd ? anchor : null);

        lastCellKey = cellKey;
        if (target === null || target === main.head) return;

        syncing = true;
        try {
          update.view.dispatch({
            selection: { anchor: target },
            scrollIntoView: true,
          });
        } finally {
          syncing = false;
        }
      },
    };
  });
}

export const __tableCursorInternals = {
  tableCellAtColumn,
  tableCellNavigationAnchorInLine,
  clampTableCursorToContent,
};

function toggleChecklistAtPos(view: EditorView, pos: number): boolean {
  const line = view.state.doc.lineAt(pos);
  const info = markdownClassifyLine(line.text);
  if (info.checklistMarkerStart === null || info.checklistMarkerEnd === null) {
    return false;
  }
  const markerFrom = line.from + info.checklistMarkerStart;
  const markerTo = line.from + info.checklistMarkerEnd;

  const markFrom = markerFrom + 1;
  const markTo = markerTo - 1;
  if (markFrom >= markTo || markFrom < line.from || markTo > line.to) {
    return false;
  }

  const prevSelection = view.state.selection;
  view.dispatch({
    changes: { from: markFrom, to: markTo, insert: info.checklistChecked ? " " : "x" },
    selection: prevSelection,
  });
  return true;
}

function checklistClickHandlers() {
  return EditorView.domEventHandlers({
    mousedown(event) {
      const target = event.target;
      if (!(target instanceof Element)) return false;
      if (!target.closest(".md-checklist-mark")) return false;
      event.preventDefault();
      return true;
    },
    click(event, view) {
      const target = event.target;
      if (!(target instanceof Element)) return false;
      const markEl = target.closest(".md-checklist-mark");
      if (!markEl) return false;
      let pos: number;
      try {
        pos = view.posAtDOM(markEl, 0);
      } catch {
        return false;
      }
      const toggled = toggleChecklistAtPos(view, pos);
      if (!toggled) return false;
      event.preventDefault();
      event.stopPropagation();
      return true;
    },
  });
}

interface MarkdownEditingOptions {
  autoformat?: boolean;
  checklistAutoReorder?: boolean;
}

export function markdownEditingExtensions(options: MarkdownEditingOptions = {}) {
  const autoformat = options.autoformat ?? true;
  const checklistAutoReorder = options.checklistAutoReorder ?? true;
  return [
    Prec.high(tablePipeInputHandler()),
    Prec.high(keymap.of(tableCursorKeymap(autoformat))),
    Prec.high(keymap.of(markdownShortcutKeymap(autoformat))),
    Prec.low(keymap.of(markdownTabKeymap(autoformat))),
    checklistClickHandlers(),
    tableCursorGuards(),
    textRulesPlugin(autoformat, checklistAutoReorder),
  ];
}
