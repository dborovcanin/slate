import { Prec, type Extension } from "@codemirror/state";
import { EditorView, keymap, ViewPlugin, type KeyBinding } from "@codemirror/view";
import type { ViewUpdate } from "@codemirror/view";
import { getCalcResultAtCursor } from "./calc-decoration.ts";
import {
  applyEditOperation,
  changedRangeFromChanges,
  offsetEditOperation,
  snapshotFromViewLines,
  snapshotFromViewTableBlock,
} from "./core/codemirror-adapter.ts";
import {
  getTableCursorCellInfo,
  markdownClassifyLine,
  runTableMultilineBreakRule,
  runMarkdownTransactions,
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
  logicalRowIndex?: number;
  logicalRowCount?: number;
  isContinuationRow?: boolean;
}

function isMarkdownTableLine(text: string): boolean {
  const trimmed = text.trim();
  return trimmed.startsWith("|") && trimmed.endsWith("|");
}

function isTableDelimiterLine(text: string): boolean {
  return isMarkdownTableLine(text) && /^[\s|:-]+$/.test(text);
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

function tableCellAtStatePosition(
  state: EditorView["state"],
  pos: number,
): TableCellInfo | null {
  const line = state.doc.lineAt(pos);
  if (!isMarkdownTableLine(line.text)) return null;
  const colInLine = Math.max(0, Math.min(pos - line.from, line.text.length));

  let startLine = line.number;
  while (startLine > 1 && isMarkdownTableLine(state.doc.line(startLine - 1).text)) {
    startLine -= 1;
  }
  let endLine = line.number;
  while (endLine < state.doc.lines && isMarkdownTableLine(state.doc.line(endLine + 1).text)) {
    endLine += 1;
  }

  const blockLines: string[] = [];
  for (let lineNo = startLine; lineNo <= endLine; lineNo += 1) {
    blockLines.push(state.doc.line(lineNo).text);
  }

  const info = getTableCursorCellInfo(blockLines, line.number - startLine, colInLine);
  if (!info) {
    return tableCellAtColumn(line.text, colInLine);
  }
  return {
    index: info.columnIndex,
    cellCount: info.columnCount,
    leftPipe: info.leftPipe,
    rightPipe: info.rightPipe,
    trimStart: info.trimStart,
    trimEnd: info.trimEnd,
    logicalRowIndex: info.logicalRowIndex ?? undefined,
    logicalRowCount: info.logicalRowCount,
    isContinuationRow: info.isContinuationRow,
  };
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

function lineMightTriggerDocChangeRules(line: string): boolean {
  const trimmed = line.trimStart();
  if (trimmed.length === 0) return false;
  const mightBeList = trimmed.startsWith("-")
    || trimmed.startsWith("*")
    || trimmed.startsWith("+")
    || trimmed.startsWith("->")
    || /^[0-9]/.test(trimmed);
  const mightBeTable = trimmed.startsWith("|") && line.trimEnd().endsWith("|");
  return mightBeList || mightBeTable;
}

function insertedTextMightTriggerDocRules(update: ViewUpdate): boolean {
  const markerRe = /(?:^|\n)\s*(?:\||->|[-*+]|\d)/;
  for (const tr of update.transactions) {
    let matched = false;
    tr.changes.iterChanges((_fromA, _toA, _fromB, _toB, text) => {
      if (matched) return;
      const inserted = text.toString();
      if (!inserted) return;
      if (inserted.includes("|") || markerRe.test(inserted)) {
        matched = true;
      }
    });
    if (matched) return true;
  }
  return false;
}

function rangeMightTriggerDocRules(
  doc: EditorView["state"]["doc"],
  from: number,
  to: number,
): boolean {
  const startLine = doc.lineAt(from).number;
  const endLine = doc.lineAt(Math.max(from, to)).number;
  // Large edits are likely structural; avoid over-filtering and run rules.
  if (endLine - startLine > 32) return true;
  for (let lineNo = startLine; lineNo <= endLine; lineNo += 1) {
    if (lineMightTriggerDocChangeRules(doc.line(lineNo).text)) {
      return true;
    }
  }
  return false;
}

function updateMightTriggerDocChangeRules(update: ViewUpdate): boolean {
  if (insertedTextMightTriggerDocRules(update)) return true;
  let shouldRun = false;
  update.changes.iterChangedRanges((fromA, toA, fromB, toB) => {
    if (shouldRun) return;
    if (
      rangeMightTriggerDocRules(update.startState.doc, fromA, toA)
      || rangeMightTriggerDocRules(update.state.doc, fromB, toB)
    ) {
      shouldRun = true;
    }
  });
  if (shouldRun) return true;
  const main = update.state.selection.main;
  return lineMightTriggerDocChangeRules(update.state.doc.lineAt(main.head).text);
}

function clampTableCursorToContent(state: EditorView["state"], pos: number): number | null {
  const line = state.doc.lineAt(pos);
  if (!isMarkdownTableLine(line.text)) return null;
  const cell = tableCellAtStatePosition(state, pos);
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

// Enter rules only inspect the current line — 2-line margin is enough context.
const ENTER_WINDOW_LINES = 2;
// Tab/indent rules walk the list block or table block around the cursor.
const TAB_WINDOW_LINES = 50;
// Doc-change rules scan the checklist/list block outward from the changed region.
const DOC_CHANGE_WINDOW_LINES = 200;
// Defer very large table autoformat to keep typing responsive.
const TABLE_AUTOFORMAT_DEFER_LINE_THRESHOLD = 120;
const TABLE_AUTOFORMAT_DEFER_MS = 24;

function continueListOnEnter(
  view: EditorView,
  autoformat: boolean,
  tableEnabled: boolean,
): boolean {
  const scoped = snapshotFromViewLines(view, ENTER_WINDOW_LINES);
  const result = runMarkdownTransactions(scoped.snapshot, [
    {
      kind: "enter",
      markdownAutoformat: autoformat,
      tableEnabled,
    },
  ]);
  if (!result) return false;
  applyEditOperation(view, offsetEditOperation(result.operation, scoped.offset));
  return true;
}

function indentListOnTab(
  view: EditorView,
  autoformat: boolean,
  tableEnabled: boolean,
  outdent = false,
): boolean {
  if (!autoformat && !tableEnabled) return false;

  // Prefer calc Tab-apply behavior when a ghost result is available.
  if (!outdent && getCalcResultAtCursor(view) !== null) return false;

  const scoped = snapshotFromViewLines(view, TAB_WINDOW_LINES);
  const result = runMarkdownTransactions(scoped.snapshot, [
    {
      kind: "tab",
      markdownAutoformat: autoformat,
      outdent,
      tableEnabled,
    },
  ]);
  if (!result) return false;
  applyEditOperation(view, offsetEditOperation(result.operation, scoped.offset));
  return true;
}

function tableArrowMovePrev(
  view: EditorView,
  line: ReturnType<EditorView["state"]["doc"]["lineAt"]>,
  cell: TableCellInfo,
): boolean {
  const state = view.state;
  if (cell.index > 0) {
    const prevCell = tableCellAtColumn(line.text, cell.leftPipe - 1);
    if (prevCell && prevCell.index !== cell.index) {
      view.dispatch({
        selection: { anchor: line.from + tableCellNavigationAnchorInLine(prevCell) },
        scrollIntoView: true,
      });
      return true;
    }
  }
  const bounds = tableBoundsForLineNo(state, line.number);
  for (let ln = line.number - 1; ln >= (bounds?.startLine ?? 1); ln -= 1) {
    const prevLine = state.doc.line(ln);
    if (isTableDelimiterLine(prevLine.text)) continue;
    const prevCell = tableCellAtColumn(prevLine.text, prevLine.text.length);
    if (prevCell) {
      view.dispatch({
        selection: { anchor: prevLine.from + tableCellNavigationAnchorInLine(prevCell) },
        scrollIntoView: true,
      });
      return true;
    }
  }
  if (!bounds || bounds.startLine <= 1) {
    view.dispatch({ selection: { anchor: 0 }, scrollIntoView: true });
  } else {
    const exitLine = state.doc.line(bounds.startLine - 1);
    view.dispatch({ selection: { anchor: exitLine.from }, scrollIntoView: true });
  }
  return true;
}

function tableArrowMoveNext(
  view: EditorView,
  line: ReturnType<EditorView["state"]["doc"]["lineAt"]>,
  cell: TableCellInfo,
): boolean {
  const state = view.state;
  const nextCell = tableCellAtColumn(line.text, cell.rightPipe + 1);
  if (nextCell && nextCell.index !== cell.index) {
    view.dispatch({
      selection: { anchor: line.from + Math.min(nextCell.leftPipe + 2, nextCell.rightPipe) },
      scrollIntoView: true,
    });
    return true;
  }
  const bounds = tableBoundsForLineNo(state, line.number);
  for (let ln = line.number + 1; ln <= (bounds?.endLine ?? state.doc.lines); ln += 1) {
    const nextLine = state.doc.line(ln);
    if (isTableDelimiterLine(nextLine.text)) continue;
    const firstCell = tableCellAtColumn(nextLine.text, 1);
    if (firstCell) {
      view.dispatch({
        selection: { anchor: nextLine.from + Math.min(firstCell.leftPipe + 2, firstCell.rightPipe) },
        scrollIntoView: true,
      });
      return true;
    }
  }
  if (!bounds || bounds.endLine >= state.doc.lines) {
    view.dispatch({ selection: { anchor: state.doc.length }, scrollIntoView: true });
  } else {
    const exitLine = state.doc.line(bounds.endLine + 1);
    view.dispatch({ selection: { anchor: exitLine.from }, scrollIntoView: true });
  }
  return true;
}

function tableArrowMove(view: EditorView, direction: -1 | 1): boolean {
  const main = view.state.selection.main;
  if (!main.empty) return false;
  const line = view.state.doc.lineAt(main.head);
  const cell = tableCellAtStatePosition(view.state, main.head);
  if (!cell) return false;

  const contentStart = line.from + Math.min(cell.leftPipe + 2, cell.rightPipe);
  const contentEnd = line.from + tableCellNavigationAnchorInLine(cell);

  if (cell.trimEnd <= cell.trimStart || main.head > contentEnd) {
    if (main.head !== contentEnd) {
      view.dispatch({ selection: { anchor: contentEnd }, scrollIntoView: true });
    }
    return true;
  }

  if (direction === -1) {
    if (main.head > contentStart) {
      view.dispatch({ selection: { anchor: main.head - 1 }, scrollIntoView: true });
      return true;
    }
    return tableArrowMovePrev(view, line, cell);
  }

  if (main.head < contentEnd) {
    view.dispatch({ selection: { anchor: main.head + 1 }, scrollIntoView: true });
    return true;
  }
  return tableArrowMoveNext(view, line, cell);
}

function tableBoundaryEdit(
  view: EditorView,
  autoformat: boolean,
  tableEnabled: boolean,
  backward: boolean,
  structuralMerge = false,
): boolean {
  const main = view.state.selection.main;
  if (!main.empty) return false;
  const line = view.state.doc.lineAt(main.head);
  if (!isMarkdownTableLine(line.text)) return false;
  const scoped = snapshotFromViewTableBlock(view);
  if (!scoped) return false;
  const result = runMarkdownTransactions(scoped.snapshot, [
    {
      kind: "table_boundary_edit",
      markdownAutoformat: autoformat,
      backward,
      structuralMerge,
      tableEnabled,
    },
  ]);
  if (!result) return false;
  applyEditOperation(view, offsetEditOperation(result.operation, scoped.offset));
  return true;
}

function tablePipeInputHandler() {
  return EditorView.inputHandler.of((view, from, to, insert) => {
    if (insert !== "|") return false;
    const main = view.state.selection.main;
    if (!main.empty || main.from !== from || main.to !== to) return false;
    const line = view.state.doc.lineAt(from);
    if (!isMarkdownTableLine(line.text)) return false;
    view.dispatch({
      changes: { from, to, insert: "\\|" },
      selection: { anchor: from + 2 },
      scrollIntoView: false,
    });
    return true;
  });
}

function tableHeaderDeleteColumn(view: EditorView): boolean {
  const main = view.state.selection.main;
  if (!main.empty) return false;
  const line = view.state.doc.lineAt(main.head);
  if (!isMarkdownTableLine(line.text)) return false;
  const scoped = snapshotFromViewTableBlock(view);
  if (!scoped) return false;
  const result = runMarkdownTransactions(scoped.snapshot, [
    { kind: "table_header_delete_column" },
  ]);
  if (!result) return false;
  applyEditOperation(view, offsetEditOperation(result.operation, scoped.offset));
  return true;
}

function tableHeaderDeleteOrBoundaryEdit(
  view: EditorView,
  autoformat: boolean,
  tableEnabled: boolean,
  backward: boolean,
): boolean {
  const main = view.state.selection.main;
  if (!main.empty) return false;
  const line = view.state.doc.lineAt(main.head);
  if (!isMarkdownTableLine(line.text)) return false;
  const scoped = snapshotFromViewTableBlock(view);
  if (!scoped) return false;
  const result = runMarkdownTransactions(scoped.snapshot, [
    { kind: "table_header_delete_column" },
    {
      kind: "table_boundary_edit",
      markdownAutoformat: autoformat,
      backward,
      structuralMerge: true,
      tableEnabled,
    },
  ]);
  if (!result) return false;
  applyEditOperation(view, offsetEditOperation(result.operation, scoped.offset));
  return true;
}

export function runTableHeaderDeleteColumnCommand(view: EditorView): boolean {
  return tableHeaderDeleteColumn(view);
}

interface TableCellNavigationCommandOptions {
  markdownAutoformat?: boolean;
  outdent?: boolean;
  tableEnabled?: boolean;
}

export function runTableCellNavigationCommand(
  view: EditorView,
  options: TableCellNavigationCommandOptions = {},
): boolean {
  const main = view.state.selection.main;
  if (!main.empty) return false;
  const line = view.state.doc.lineAt(main.head);
  if (!isMarkdownTableLine(line.text)) return false;
  const scoped = snapshotFromViewTableBlock(view);
  if (!scoped) return false;
  const result = runMarkdownTransactions(scoped.snapshot, [
    {
      kind: "table_cell_navigation",
      markdownAutoformat: options.markdownAutoformat ?? true,
      outdent: options.outdent ?? false,
      tableEnabled: options.tableEnabled ?? true,
    },
  ]);
  if (!result) return false;
  applyEditOperation(view, offsetEditOperation(result.operation, scoped.offset));
  return true;
}

function tableCellJump(
  view: EditorView,
  autoformat: boolean,
  tableEnabled: boolean,
  outdent: boolean,
): boolean {
  return runTableCellNavigationCommand(view, {
    markdownAutoformat: autoformat,
    outdent,
    tableEnabled,
  });
}

function tableMultilineBreak(view: EditorView, tableEnabled: boolean): boolean {
  const main = view.state.selection.main;
  if (!main.empty) return false;
  const line = view.state.doc.lineAt(main.head);
  if (!isMarkdownTableLine(line.text)) return false;
  const scoped = snapshotFromViewTableBlock(view);
  if (!scoped) return false;
  const op = runTableMultilineBreakRule(scoped.snapshot, tableEnabled);
  if (!op) return false;
  applyEditOperation(view, offsetEditOperation(op, scoped.offset));
  return true;
}

interface TableLineBounds {
  startLine: number;
  endLine: number;
}

function tableBoundsForLineNo(state: EditorView["state"], lineNo: number): TableLineBounds | null {
  if (lineNo < 1 || lineNo > state.doc.lines) return null;
  if (!isMarkdownTableLine(state.doc.line(lineNo).text)) return null;
  let startLine = lineNo;
  let endLine = lineNo;
  while (startLine > 1 && isMarkdownTableLine(state.doc.line(startLine - 1).text)) startLine -= 1;
  while (endLine < state.doc.lines && isMarkdownTableLine(state.doc.line(endLine + 1).text)) endLine += 1;
  return { startLine, endLine };
}

function markdownShortcutKeymap(autoformat: boolean, tableEnabled: boolean): KeyBinding[] {
  const keys: KeyBinding[] = [
    { key: "Mod-b", preventDefault: true, run: (view) => toggleWrap(view, "**") },
    { key: "Mod-i", preventDefault: true, run: (view) => toggleWrap(view, "*") },
    { key: "Mod-Shift-x", preventDefault: true, run: (view) => toggleWrap(view, "~~") },
    { key: "Mod-k", preventDefault: true, run: wrapLink },
  ];

  if (autoformat || tableEnabled) {
    keys.push({
      key: "Enter",
      run: (view) => continueListOnEnter(view, autoformat, tableEnabled),
      preventDefault: true,
    });
  }

  return keys;
}

function markdownTabKeymap(autoformat: boolean, tableEnabled: boolean): KeyBinding[] {
  return [
    {
      key: "Tab",
      preventDefault: true,
      run: (view) => indentListOnTab(view, autoformat, tableEnabled, false),
    },
    {
      key: "Shift-Tab",
      preventDefault: true,
      run: (view) => indentListOnTab(view, autoformat, tableEnabled, true),
    },
  ];
}

function tableCursorKeymap(
  autoformat: boolean,
  tableEnabled: boolean,
): KeyBinding[] {
  return [
    {
      key: "Backspace",
      run: (view) => tableBoundaryEdit(view, autoformat, tableEnabled, true),
    },
    {
      key: "Delete",
      run: (view) => tableBoundaryEdit(view, autoformat, tableEnabled, false),
    },
    {
      key: "Ctrl-Backspace",
      preventDefault: true,
      run: (view) => tableHeaderDeleteOrBoundaryEdit(view, autoformat, tableEnabled, true),
    },
    {
      key: "Ctrl-Delete",
      preventDefault: true,
      run: (view) => tableHeaderDeleteOrBoundaryEdit(view, autoformat, tableEnabled, false),
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
      run: (view) => tableCellJump(view, autoformat, tableEnabled, true),
    },
    {
      key: "Ctrl-ArrowRight",
      preventDefault: true,
      run: (view) => tableCellJump(view, autoformat, tableEnabled, false),
    },
    {
      key: "Shift-Enter",
      preventDefault: true,
      run: (view) => tableMultilineBreak(view, tableEnabled),
    },
  ];
}

function textRulesPlugin(
  autoformat: boolean,
  checklistAutoReorder: boolean,
  tableEnabled: boolean,
) {
  return ViewPlugin.define(() => {
    let applying = false;
    let deferredTimer: ReturnType<typeof setTimeout> | null = null;

    const runRules = (view: EditorView, changedRange?: { from: number; to: number }) => {
      const inTable = isMarkdownTableLine(view.state.doc.lineAt(view.state.selection.main.head).text);
      const tableScoped = inTable ? snapshotFromViewTableBlock(view) : null;
      const scoped = tableScoped ?? snapshotFromViewLines(view, DOC_CHANGE_WINDOW_LINES, changedRange);
      const result = runMarkdownTransactions(scoped.snapshot, [
        {
          kind: "doc_change",
          markdownAutoformat: autoformat,
          checklistAutoReorder,
          tableEnabled,
        },
      ]);
      if (!result) return;
      applyEditOperation(view, offsetEditOperation(result.operation, scoped.offset));
    };

    return {
      update(update: ViewUpdate) {
        if (applying || !update.docChanged) return;
        if (shouldDeferTableAutoformatForSpace(update)) return;
        if (update.transactions.some((tr) => tr.isUserEvent("table.cell.edit"))) return;
        if (!updateMightTriggerDocChangeRules(update)) return;
        const changedRange = changedRangeFromChanges(update.changes);
        const tableScoped = snapshotFromViewTableBlock(update.view);
        const tableLineCount = tableScoped ? tableScoped.snapshot.text.split("\n").length : 0;
        const shouldDeferLargeTable = tableLineCount >= TABLE_AUTOFORMAT_DEFER_LINE_THRESHOLD;

        if (shouldDeferLargeTable) {
          if (deferredTimer !== null) clearTimeout(deferredTimer);
          deferredTimer = setTimeout(() => {
            applying = true;
            try {
              runRules(update.view, changedRange);
            } catch (error) {
              console.error("Deferred markdown table rule dispatch failed:", error);
            } finally {
              applying = false;
            }
          }, TABLE_AUTOFORMAT_DEFER_MS);
          return;
        }

        applying = true;
        Promise.resolve().then(() => {
          applying = true;
          try {
            runRules(update.view, changedRange);
          } catch (error) {
            console.error("Markdown text rule dispatch failed:", error);
          } finally {
            applying = false;
          }
        });
        applying = false;
      },
      destroy() {
        if (deferredTimer !== null) clearTimeout(deferredTimer);
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

function tableCursorPaddingGuard() {
  return ViewPlugin.define(() => ({
    update(update: ViewUpdate) {
      if (!update.selectionSet) return;
      const main = update.state.selection.main;
      if (!main.empty) return;
      const clamped = clampTableCursorToContent(update.state, main.head);
      if (clamped === null || clamped === main.head) return;
      update.view.dispatch({
        selection: { anchor: clamped },
        scrollIntoView: false,
      });
    },
  }));
}

interface MarkdownEditingOptions {
  autoformat?: boolean;
  checklistAutoReorder?: boolean;
  tableEnabled?: boolean;
}

export function markdownEditingExtensions(options: MarkdownEditingOptions = {}) {
  const autoformat = options.autoformat ?? true;
  const checklistAutoReorder = options.checklistAutoReorder ?? true;
  const tableEnabled = options.tableEnabled ?? true;
  const tableExtensions = tableEnabled
    ? [
      Prec.high(tablePipeInputHandler()),
      Prec.high(keymap.of(tableCursorKeymap(autoformat, tableEnabled))),
      tableCursorPaddingGuard(),
    ]
    : [];
  return [
    ...tableExtensions,
    Prec.high(keymap.of(markdownShortcutKeymap(autoformat, tableEnabled))),
    Prec.low(keymap.of(markdownTabKeymap(autoformat, tableEnabled))),
    checklistClickHandlers(),
    textRulesPlugin(autoformat, checklistAutoReorder, tableEnabled),
  ];
}
