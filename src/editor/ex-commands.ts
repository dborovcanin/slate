import { EditorView } from "@codemirror/view";

export type SumScope = "paragraph" | "list" | "table" | "doc";
export interface LineRange {
  startLine: number;
  endLine: number;
}
export const EX_COMMAND_CANDIDATES = [
  "sum",
  "sum list",
  "sum table",
  "sum doc",
  "sum_all",
  "date",
] as const;
export type ExCommandCandidate = (typeof EX_COMMAND_CANDIDATES)[number];

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

function textForRange(lines: string[], range: LineRange): string {
  const out: string[] = [];
  for (let n = range.startLine; n <= range.endLine; n++) out.push(lines[n - 1] ?? "");
  return out.join("\n");
}

function linesFromView(view: EditorView): string[] {
  const out: string[] = [];
  for (let n = 1; n <= view.state.doc.lines; n++) out.push(lineText(view, n));
  return out;
}

function insertValueAfterRange(view: EditorView, range: LineRange, value: string) {
  const line = view.state.doc.line(range.endLine);
  const from = line.to;
  const prefix = view.state.doc.length === 0 ? "" : "\n";
  const insert = `${prefix}${value}`;

  view.dispatch({
    changes: { from, to: from, insert },
    selection: { anchor: from + insert.length },
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

export function suggestExCommands(rawInput: string): ExCommandCandidate[] {
  const query = rawInput.trim().toLowerCase().replace(/^:/, "");
  if (!query) return [...EX_COMMAND_CANDIDATES];

  return [...EX_COMMAND_CANDIDATES]
    .map((command) => {
      const c = command.toLowerCase();
      const starts = c.startsWith(query);
      const includes = c.includes(query);
      const score = starts ? 0 : includes ? 1 : 2;
      return { command, score };
    })
    .filter((entry) => entry.score < 2)
    .sort((a, b) => a.score - b.score || a.command.localeCompare(b.command))
    .map((entry) => entry.command);
}

async function copyText(text: string) {
  if (!navigator.clipboard) return;
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
    const lines = linesFromView(view);
    const range = resolveScopeRange(lines, currentLineNo(view), scope);
    const text = range ? textForRange(lines, range) : "";
    const numbers = parseNumbers(text);
    if (numbers.length === 0) return `sum(${scope}): no numbers`;
    const sum = numbers.reduce((acc, n) => acc + n, 0);
    const formatted = formatNumber(sum);
    insertValueAfterRange(view, range ?? docRange(lines), formatted);
    await copyText(formatted);
    return `sum(${scope}) = ${formatted} (${numbers.length} values, inserted value + copied)`;
  }

  return `unknown command: ${trimmed}`;
}
