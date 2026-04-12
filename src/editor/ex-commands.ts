import { EditorView } from "@codemirror/view";

type SumScope = "paragraph" | "list" | "table" | "doc";

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

function scopeParagraph(view: EditorView): string {
  const cursor = currentLineNo(view);
  let start = cursor;
  let end = cursor;
  while (start > 1 && lineText(view, start - 1).trim().length > 0) start--;
  while (end < view.state.doc.lines && lineText(view, end + 1).trim().length > 0) end++;
  const out: string[] = [];
  for (let n = start; n <= end; n++) out.push(lineText(view, n));
  return out.join("\n");
}

function scopeList(view: EditorView): string {
  const cursor = currentLineNo(view);
  if (!listLineRe.test(lineText(view, cursor))) return "";

  let start = cursor;
  let end = cursor;
  while (start > 1 && listLineRe.test(lineText(view, start - 1))) start--;
  while (end < view.state.doc.lines && listLineRe.test(lineText(view, end + 1))) end++;
  const out: string[] = [];
  for (let n = start; n <= end; n++) out.push(lineText(view, n));
  return out.join("\n");
}

function scopeTable(view: EditorView): string {
  const cursor = currentLineNo(view);
  if (!tableLineRe.test(lineText(view, cursor))) return "";

  let start = cursor;
  let end = cursor;
  while (start > 1 && tableLineRe.test(lineText(view, start - 1))) start--;
  while (end < view.state.doc.lines && tableLineRe.test(lineText(view, end + 1))) end++;
  const out: string[] = [];
  for (let n = start; n <= end; n++) out.push(lineText(view, n));
  return out.join("\n");
}

function scopeDoc(view: EditorView): string {
  return view.state.doc.toString();
}

function textForScope(view: EditorView, scope: SumScope): string {
  switch (scope) {
    case "paragraph":
      return scopeParagraph(view);
    case "list":
      return scopeList(view);
    case "table":
      return scopeTable(view);
    case "doc":
      return scopeDoc(view);
  }
}

export function parseScope(raw: string | undefined): SumScope {
  const arg = (raw ?? "").trim().toLowerCase();
  if (arg === "all" || arg === "doc" || arg === "file") return "doc";
  if (arg === "list") return "list";
  if (arg === "table") return "table";
  return "paragraph";
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

  if (trimmed === "sum_all") {
    const text = textForScope(view, "doc");
    const numbers = parseNumbers(text);
    if (numbers.length === 0) return "sum(doc): no numbers";
    const sum = numbers.reduce((acc, n) => acc + n, 0);
    const formatted = formatNumber(sum);
    await copyText(formatted);
    return `sum(doc) = ${formatted} (${numbers.length} values, copied)`;
  }

  if (trimmed.startsWith("sum")) {
    const scope = parseScope(trimmed.slice(3));
    const text = textForScope(view, scope);
    const numbers = parseNumbers(text);
    if (numbers.length === 0) return `sum(${scope}): no numbers`;
    const sum = numbers.reduce((acc, n) => acc + n, 0);
    const formatted = formatNumber(sum);
    await copyText(formatted);
    return `sum(${scope}) = ${formatted} (${numbers.length} values, copied)`;
  }

  return `unknown command: ${trimmed}`;
}
