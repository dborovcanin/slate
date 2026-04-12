import { EditorView } from "@codemirror/view";

export type SumScope = "paragraph" | "list" | "table" | "doc";
export interface LineRange {
  startLine: number;
  endLine: number;
}

const listLineRe = /^\s*(?:[-*+]|\d+\.)\s+/;
const tableLineRe = /^\s*\|.*\|\s*$/;
const numberRe = /[-+]?(?:\d{1,3}(?:,\d{3})+|\d+)(?:\.\d+)?|[-+]?\.\d+/g;

function formatNumber(value: number): string {
  if (!Number.isFinite(value)) return "0";
  if (Number.isInteger(value)) return `${value}`;
  return value.toFixed(10).replace(/\.?0+$/, "");
}

export function parseNumbers(text: string): number[] {
  const matches = text.match(numberRe) ?? [];
  const values: number[] = [];
  for (const token of matches) {
    const normalized = token.replace(/,/g, "");
    const value = Number.parseFloat(normalized);
    if (Number.isFinite(value)) values.push(value);
  }
  return values;
}

function currentLineNo(view: EditorView): number {
  return view.state.doc.lineAt(view.state.selection.main.head).number;
}

function lineText(view: EditorView, lineNo: number): string {
  return view.state.doc.line(lineNo).text;
}

function lineCount(view: EditorView): number {
  return view.state.doc.lines;
}

function paragraphRange(lines: string[], cursor: number): LineRange {
  let start = cursor;
  let end = cursor;
  while (start > 1 && lines[start - 2].trim().length > 0) start--;
  while (end < lines.length && lines[end].trim().length > 0) end++;
  return { startLine: start, endLine: end };
}

function listRange(lines: string[], cursor: number): LineRange | null {
  if (!listLineRe.test(lines[cursor - 1] ?? "")) return null;

  let start = cursor;
  let end = cursor;
  while (start > 1 && listLineRe.test(lines[start - 2] ?? "")) start--;
  while (end < lines.length && listLineRe.test(lines[end] ?? "")) end++;
  return { startLine: start, endLine: end };
}

function tableRange(lines: string[], cursor: number): LineRange | null {
  if (!tableLineRe.test(lines[cursor - 1] ?? "")) return null;

  let start = cursor;
  let end = cursor;
  while (start > 1 && tableLineRe.test(lines[start - 2] ?? "")) start--;
  while (end < lines.length && tableLineRe.test(lines[end] ?? "")) end++;
  return { startLine: start, endLine: end };
}

function docRange(lines: string[]): LineRange {
  return { startLine: 1, endLine: Math.max(1, lines.length) };
}

export function resolveScopeRange(
  lines: string[],
  cursorLine: number,
  scope: SumScope,
): LineRange | null {
  const cursor = Math.min(Math.max(1, cursorLine), Math.max(1, lines.length));
  switch (scope) {
    case "paragraph":
      return paragraphRange(lines, cursor);
    case "list":
      return listRange(lines, cursor);
    case "table":
      return tableRange(lines, cursor);
    case "doc":
      return docRange(lines);
  }
}

export function resolveScopeRangeInView(view: EditorView, scope: SumScope): LineRange | null {
  const lines = lineCount(view);
  const cursor = Math.min(Math.max(1, currentLineNo(view)), Math.max(1, lines));

  if (scope === "doc") {
    return { startLine: 1, endLine: lines };
  }

  if (scope === "paragraph") {
    let start = cursor;
    let end = cursor;
    while (start > 1 && lineText(view, start - 1).trim().length > 0) start--;
    while (end < lines && lineText(view, end + 1).trim().length > 0) end++;
    return { startLine: start, endLine: end };
  }

  if (scope === "list") {
    if (!listLineRe.test(lineText(view, cursor))) return null;
    let start = cursor;
    let end = cursor;
    while (start > 1 && listLineRe.test(lineText(view, start - 1))) start--;
    while (end < lines && listLineRe.test(lineText(view, end + 1))) end++;
    return { startLine: start, endLine: end };
  }

  if (!tableLineRe.test(lineText(view, cursor))) return null;
  let start = cursor;
  let end = cursor;
  while (start > 1 && tableLineRe.test(lineText(view, start - 1))) start--;
  while (end < lines && tableLineRe.test(lineText(view, end + 1))) end++;
  return { startLine: start, endLine: end };
}

function textForRange(lines: string[], range: LineRange): string {
  const out: string[] = [];
  for (let n = range.startLine; n <= range.endLine; n++) out.push(lines[n - 1] ?? "");
  return out.join("\n");
}

function textForRangeInView(view: EditorView, range: LineRange): string {
  const out: string[] = [];
  for (let n = range.startLine; n <= range.endLine; n++) out.push(lineText(view, n));
  return out.join("\n");
}

function insertValueAtCursor(view: EditorView, value: string) {
  const main = view.state.selection.main;
  view.dispatch({
    changes: { from: main.from, to: main.to, insert: value },
    selection: { anchor: main.from + value.length },
    scrollIntoView: true,
  });
}

export function parseScope(raw: string | undefined): SumScope {
  const arg = (raw ?? "").trim().toLowerCase();
  if (arg === "all" || arg === "doc" || arg === "file") return "doc";
  if (arg === "list") return "list";
  if (arg === "table") return "table";
  return "paragraph";
}

async function copyText(text: string) {
  if (typeof navigator === "undefined" || !navigator.clipboard) return;
  try {
    await navigator.clipboard.writeText(text);
  } catch (err) {
    console.error("Clipboard write failed:", err);
  }
}

export async function executeExCommand(
  view: EditorView,
  rawCommand: string,
): Promise<string> {
  const trimmed = rawCommand.trim().replace(/^:/, "");
  if (!trimmed) return "";

  const scope: SumScope | null =
    trimmed === "sum_all" ? "doc" : trimmed.startsWith("sum") ? parseScope(trimmed.slice(3)) : null;

  if (scope) {
    const range = resolveScopeRangeInView(view, scope);
    const text = range ? textForRangeInView(view, range) : "";
    const numbers = parseNumbers(text);
    if (numbers.length === 0) return `sum(${scope}): no numbers`;
    const sum = numbers.reduce((acc, n) => acc + n, 0);
    const formatted = formatNumber(sum);
    insertValueAtCursor(view, formatted);
    await copyText(formatted);
    return `sum(${scope}) = ${formatted} (${numbers.length} values, inserted at cursor + copied)`;
  }

  return `unknown command: ${trimmed}`;
}
