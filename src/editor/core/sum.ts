import { ResolvedContext } from "./context";
import { replaceRange } from "./operations";
import type { EditOperation } from "./types";

export type SumScope = "paragraph" | "list" | "table" | "doc";

export interface LineRange {
  startLine: number;
  endLine: number;
}

export interface SumExecutionResult {
  message: string;
  operation?: EditOperation;
  clipboardText?: string;
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

export function resolveScopeRangeInContext(
  ctx: ResolvedContext,
  scope: SumScope,
): LineRange | null {
  const lineNo = ctx.currentLine().number;
  switch (scope) {
    case "paragraph":
      return ctx.paragraphRangeAtLine(lineNo);
    case "list":
      return ctx.listRangeAtLine(lineNo);
    case "table":
      return ctx.tableRangeAtLine(lineNo);
    case "doc":
      return { startLine: 1, endLine: ctx.lineCount() };
  }
}

export function parseScope(raw: string | undefined): SumScope {
  const arg = (raw ?? "").trim().toLowerCase();
  if (arg === "all" || arg === "doc" || arg === "file") return "doc";
  if (arg === "list") return "list";
  if (arg === "table") return "table";
  return "paragraph";
}

export function parseSumScopeFromCommand(rawCommand: string): SumScope | null {
  const trimmed = rawCommand.trim().replace(/^:/, "");
  if (!trimmed) return null;
  if (trimmed === "sum_all") return "doc";
  if (trimmed.startsWith("sum")) return parseScope(trimmed.slice(3));
  return null;
}

export function executeSumCommand(rawCommand: string, ctx: ResolvedContext): SumExecutionResult {
  const trimmed = rawCommand.trim().replace(/^:/, "");
  const scope = parseSumScopeFromCommand(trimmed);
  if (!scope) {
    return { message: `unknown command: ${trimmed}` };
  }

  const range = resolveScopeRangeInContext(ctx, scope);
  const text = range ? ctx.textForLineRange(range) : "";
  const numbers = parseNumbers(text);
  if (numbers.length === 0) {
    return { message: `sum(${scope}): no numbers` };
  }

  const sum = numbers.reduce((acc, n) => acc + n, 0);
  const formatted = formatNumber(sum);
  const selection = ctx.selection();
  const operation = replaceRange(selection.from, selection.to, formatted, {
    anchor: selection.from + formatted.length,
  });

  return {
    message: `sum(${scope}) = ${formatted} (${numbers.length} values, inserted at cursor + copied)`,
    operation,
    clipboardText: formatted,
  };
}
