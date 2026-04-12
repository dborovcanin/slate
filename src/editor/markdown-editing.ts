import { EditorState, Prec } from "@codemirror/state";
import { keymap, ViewPlugin, type KeyBinding } from "@codemirror/view";
import type { EditorView, ViewUpdate } from "@codemirror/view";

type Align = "left" | "center" | "right" | "none";

const listRe = /^(\s*)([-*+]|\d+\.)\s+(.*)$/;
const tableRowRe = /^\s*\|.*\|\s*$/;
const delimiterCellRe = /^:?-{3,}:?$/;

function repeat(char: string, count: number): string {
  return new Array(Math.max(0, count) + 1).join(char);
}

function clamp(value: number, min: number, max: number) {
  return Math.min(max, Math.max(min, value));
}

function isTableRow(text: string): boolean {
  return tableRowRe.test(text);
}

function splitTableCells(line: string): string[] {
  const trimmed = line.trim();
  const inner = trimmed.replace(/^\|/, "").replace(/\|$/, "");
  return inner.split("|").map((cell) => cell.trim());
}

function parseAlign(cell: string): Align {
  if (/^:-+:$/.test(cell)) return "center";
  if (/^:-+$/.test(cell)) return "left";
  if (/^-+:$/.test(cell)) return "right";
  return "none";
}

function delimiterForWidth(width: number, align: Align): string {
  const w = Math.max(3, width);
  if (align === "left") return `:${repeat("-", Math.max(3, w - 1))}`;
  if (align === "right") return `${repeat("-", Math.max(3, w - 1))}:`;
  if (align === "center") return `:${repeat("-", Math.max(3, w - 2))}:`;
  return repeat("-", w);
}

export function formatTableLines(lines: string[]): string[] {
  if (lines.length === 0) return lines;

  const rows = lines.map(splitTableCells);
  const columnCount = rows.reduce((max, row) => Math.max(max, row.length), 0);
  const normalizedRows = rows.map((row) => {
    const cells = [...row];
    while (cells.length < columnCount) cells.push("");
    return cells;
  });

  const align: Align[] = new Array(columnCount).fill("none");
  for (const row of normalizedRows) {
    const isDelimiter = row.every((cell) => delimiterCellRe.test(cell) || cell.length === 0);
    if (!isDelimiter) continue;
    for (let i = 0; i < columnCount; i++) {
      if (delimiterCellRe.test(row[i])) {
        align[i] = parseAlign(row[i]);
      }
    }
    break;
  }

  const widths = new Array(columnCount).fill(3);
  for (const row of normalizedRows) {
    const isDelimiter = row.every((cell) => delimiterCellRe.test(cell) || cell.length === 0);
    if (isDelimiter) continue;
    for (let i = 0; i < columnCount; i++) {
      widths[i] = Math.max(widths[i], row[i].length);
    }
  }

  return normalizedRows.map((row) => {
    const isDelimiter = row.every((cell) => delimiterCellRe.test(cell) || cell.length === 0);
    const parts = row.map((cell, i) => {
      if (isDelimiter) {
        return delimiterForWidth(widths[i], align[i]);
      }
      return cell.padEnd(widths[i], " ");
    });
    return `| ${parts.join(" | ")} |`;
  });
}

interface TableBlock {
  startLine: number;
  endLine: number;
}

function findTableBlock(doc: EditorState["doc"], lineNo: number): TableBlock | null {
  if (!isTableRow(doc.line(lineNo).text)) return null;

  let start = lineNo;
  let end = lineNo;

  while (start > 1 && isTableRow(doc.line(start - 1).text)) start--;
  while (end < doc.lines && isTableRow(doc.line(end + 1).text)) end++;

  if (end - start + 1 < 2) return null;
  return { startLine: start, endLine: end };
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

function continueListOnEnter(view: EditorView): boolean {
  const main = view.state.selection.main;
  if (!main.empty) return false;

  const line = view.state.doc.lineAt(main.head);
  const before = line.text.slice(0, main.head - line.from);
  const match = before.match(listRe);
  if (!match) return false;

  const indent = match[1];
  const marker = match[2];
  const content = match[3];

  if (content.trim().length === 0 && main.head === line.to) {
    const markerFrom = line.from + indent.length;
    view.dispatch({
      changes: { from: markerFrom, to: line.to, insert: "" },
      selection: { anchor: markerFrom },
      scrollIntoView: true,
    });
    return true;
  }

  let nextMarker = marker;
  if (/^\d+\.$/.test(marker)) {
    const value = Number.parseInt(marker.slice(0, -1), 10);
    if (Number.isFinite(value)) nextMarker = `${value + 1}.`;
  }

  const insert = `\n${indent}${nextMarker} `;
  view.dispatch({
    changes: { from: main.head, to: main.head, insert },
    selection: { anchor: main.head + insert.length },
    scrollIntoView: true,
  });
  return true;
}

function maybeFormatTable(view: EditorView): boolean {
  const head = view.state.selection.main.head;
  const lineNo = view.state.doc.lineAt(head).number;
  const block = findTableBlock(view.state.doc, lineNo);
  if (!block) return false;

  const lines: string[] = [];
  for (let n = block.startLine; n <= block.endLine; n++) {
    lines.push(view.state.doc.line(n).text);
  }
  const formatted = formatTableLines(lines);

  if (formatted.every((line, i) => line === lines[i])) {
    return false;
  }

  const from = view.state.doc.line(block.startLine).from;
  const to = view.state.doc.line(block.endLine).to;
  const headLine = view.state.doc.lineAt(head).number;
  const headCol = head - view.state.doc.line(headLine).from;
  const relativeLine = clamp(headLine - block.startLine, 0, formatted.length - 1);

  let newHead = from;
  for (let i = 0; i < relativeLine; i++) {
    newHead += formatted[i].length + 1;
  }
  newHead += Math.min(headCol, formatted[relativeLine].length);

  view.dispatch({
    changes: { from, to, insert: formatted.join("\n") },
    selection: { anchor: newHead },
    scrollIntoView: true,
  });
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
      run: continueListOnEnter,
      preventDefault: true,
    });
  }

  return keys;
}

function tableAutoformatPlugin(enabled: boolean) {
  return ViewPlugin.define(() => {
    let applying = false;
    return {
      update(update: ViewUpdate) {
        if (!enabled || applying || !update.docChanged) return;
        applying = true;
        try {
          maybeFormatTable(update.view);
        } finally {
          applying = false;
        }
      },
    };
  });
}

interface MarkdownEditingOptions {
  autoformat?: boolean;
}

export function markdownEditingExtensions(options: MarkdownEditingOptions = {}) {
  const autoformat = options.autoformat ?? true;
  return [
    Prec.high(keymap.of(markdownShortcutKeymap(autoformat))),
    tableAutoformatPlugin(autoformat),
  ];
}
