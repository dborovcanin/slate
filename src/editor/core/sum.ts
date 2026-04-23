import { ResolvedContext } from "./context.ts";
import { replaceRange } from "./operations.ts";
import type { EditOperation } from "./types.ts";

export type SumScope = "paragraph" | "list" | "row" | "column" | "doc";
export type SumExpressionEvaluator = (
  expression: string,
) => Promise<string | null> | string | null;

export interface LineRange {
  startLine: number;
  endLine: number;
}

export interface SumExecutionResult {
  message: string;
  operation?: EditOperation;
  clipboardText?: string;
}

export interface SumCommandOptions {
  evaluateExpression?: SumExpressionEvaluator;
}

const listLineRe = /^\s*(?:->|[-*+]|\d+\.|\d+(?:\.\d+)+)\s+/;
const tableLineRe = /^\s*\|.*\|\s*$/;
const tableDelimiterCellRe = /^:?-{3,}:?$/;
const numberRe = /[-+]?(?:\d{1,3}(?:,\d{3})+|\d+)(?:\.\d+)?|[-+]?\.\d+/g;

function formatNumber(value: number): string {
  if (!Number.isFinite(value)) return "0.00";
  return value.toFixed(2);
}

function normalizeEvaluatedValue(value: string | null | undefined): string | null {
  const cleaned = (value ?? "")
    .replace(/\bapproximately\b/gi, "")
    .replace(/\bapprox\.?\b/gi, "")
    .replace(/[≈~]/g, "")
    .trim();
  if (!cleaned) return null;

  const match = cleaned.match(/[-+]?(?:\d{1,3}(?:,\d{3})+|\d+)(?:\.\d+)?|[-+]?\.\d+/);
  if (!match) return null;

  const raw = match[0] ?? "";
  const numeric = Number.parseFloat(raw.replace(/,/g, ""));
  if (!Number.isFinite(numeric)) return null;

  const start = match.index ?? 0;
  const suffix = cleaned.slice(start + raw.length).trim();
  const formatted = formatNumber(numeric);
  return suffix ? `${formatted} ${suffix}` : formatted;
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

function splitTableCells(line: string): string[] {
  const trimmed = line.trim();
  const inner = trimmed.replace(/^\|/, "").replace(/\|$/, "");
  return inner.split("|").map((cell) => cell.trim());
}

function isTableDelimiterRowCells(cells: readonly string[]): boolean {
  return cells.length > 0 && cells.every((cell) => cell.length === 0 || tableDelimiterCellRe.test(cell));
}

async function evaluateCellValue(
  cell: string,
  evaluateExpression?: SumExpressionEvaluator,
): Promise<string | null> {
  const trimmed = cell.trim();
  if (!trimmed) return null;
  if (!/\d/.test(trimmed)) return null;

  if (evaluateExpression) {
    const evaluated = await evaluateExpression(trimmed);
    const normalized = normalizeEvaluatedValue(evaluated);
    if (normalized) return normalized;
  }

  const numeric = parseNumbers(trimmed);
  if (numeric.length === 0) return null;
  return formatNumber(numeric.reduce((acc, value) => acc + value, 0));
}

async function sumTerms(
  terms: readonly string[],
  evaluateExpression?: SumExpressionEvaluator,
): Promise<string | null> {
  if (terms.length === 0) return null;
  if (terms.length === 1) return terms[0] ?? null;

  if (evaluateExpression) {
    let acc = terms[0] ?? "";
    for (const term of terms.slice(1)) {
      const combined = await evaluateExpression(`(${acc}) + (${term})`);
      const normalized = normalizeEvaluatedValue(combined);
      if (!normalized) return null;
      acc = normalized;
    }
    return acc || null;
  }

  const total = terms
    .flatMap((term) => parseNumbers(term))
    .reduce((acc, value) => acc + value, 0);
  return formatNumber(total);
}

async function averageTerms(
  terms: readonly string[],
  evaluateExpression?: SumExpressionEvaluator,
): Promise<string | null> {
  if (terms.length === 0) return null;

  const summed = await sumTerms(terms, evaluateExpression);
  if (!summed) return null;
  if (terms.length === 1) return summed;

  if (evaluateExpression) {
    const averaged = await evaluateExpression(`(${summed}) / ${terms.length}`);
    const normalized = normalizeEvaluatedValue(averaged);
    if (normalized) return normalized;
    return null;
  }

  const numeric = parseNumbers(summed)[0];
  if (numeric === undefined) return null;
  return formatNumber(numeric / terms.length);
}

function cursorTableColumn(ctx: ResolvedContext): number | null {
  const line = ctx.currentLine();
  if (!tableLineRe.test(line.text)) return null;
  const colInLine = Math.min(Math.max(ctx.cursorPos() - line.from, 0), line.text.length);
  const pipesBefore = [...line.text.slice(0, colInLine)].filter((char) => char === "|").length;
  if (pipesBefore === 0) return null;
  return pipesBefore - 1;
}

async function collectRowTerms(
  ctx: ResolvedContext,
  evaluateExpression?: SumExpressionEvaluator,
): Promise<string[]> {
  const cursorCol = cursorTableColumn(ctx);
  if (cursorCol === null) return [];
  const cells = splitTableCells(ctx.currentLine().text);
  const out: string[] = [];
  for (const cell of cells.slice(0, Math.min(cursorCol, cells.length))) {
    const evaluated = await evaluateCellValue(cell, evaluateExpression);
    if (evaluated !== null) out.push(evaluated);
  }
  return out;
}

async function collectColumnTerms(
  ctx: ResolvedContext,
  range: LineRange,
  evaluateExpression?: SumExpressionEvaluator,
): Promise<string[]> {
  const cursorCol = cursorTableColumn(ctx);
  if (cursorCol === null) return [];
  const cursorLine = ctx.currentLine().number;
  const out: string[] = [];
  for (let lineNo = range.startLine; lineNo < cursorLine; lineNo++) {
    const cells = splitTableCells(ctx.lineText(lineNo));
    if (isTableDelimiterRowCells(cells)) continue;
    const evaluated = await evaluateCellValue(cells[cursorCol] ?? "", evaluateExpression);
    if (evaluated !== null) out.push(evaluated);
  }
  return out;
}

function replaceTableCellAtCursor(
  ctx: ResolvedContext,
  value: string,
): EditOperation {
  const selection = ctx.selection();
  const line = ctx.currentLine();
  const cursorInLine = Math.min(Math.max(ctx.cursorPos() - line.from, 0), line.text.length);
  const leftPipe = line.text.slice(0, cursorInLine).lastIndexOf("|");
  const rightPipeRel = line.text.slice(cursorInLine).indexOf("|");
  if (leftPipe < 0 || rightPipeRel < 0) {
    return replaceRange(selection.from, selection.to, value, {
      anchor: selection.from + value.length,
    });
  }
  const cellFrom = line.from + leftPipe + 1;
  const cellTo = line.from + cursorInLine + rightPipeRel;
  const formatted = ` ${value} `;
  return replaceRange(cellFrom, cellTo, formatted, {
    anchor: cellFrom + formatted.length,
  });
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
    case "row":
    case "column":
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
    case "row":
    case "column":
      return ctx.tableRangeAtLine(lineNo, 1);
    case "doc":
      return { startLine: 1, endLine: ctx.lineCount() };
  }
}

export function parseScope(raw: string | undefined): SumScope {
  const arg = (raw ?? "").trim().toLowerCase();
  if (arg === "all" || arg === "doc" || arg === "file") return "doc";
  if (arg === "list") return "list";
  if (arg === "row") return "row";
  if (arg === "column") return "column";
  return "paragraph";
}

export function parseSumScopeFromCommand(rawCommand: string): SumScope | null {
  const trimmed = rawCommand.trim().replace(/^:/, "");
  if (!trimmed) return null;
  if (trimmed === "sum_all") return "doc";
  if (trimmed === "sum_row") return "row";
  if (trimmed === "sum_column") return "column";
  if (!trimmed.startsWith("sum")) return null;

  const rawScope = trimmed.slice(3).trim().toLowerCase();
  if (rawScope.length === 0) return "paragraph";
  if (rawScope === "doc" || rawScope === "all" || rawScope === "file") return "doc";
  if (rawScope === "list") return "list";
  if (rawScope === "row") return "row";
  if (rawScope === "column") return "column";
  return null;
}

export function parseAvgScopeFromCommand(rawCommand: string): SumScope | null {
  const trimmed = rawCommand.trim().replace(/^:/, "");
  if (!trimmed) return null;
  if (trimmed === "avg_all") return "doc";
  if (trimmed === "avg_row") return "row";
  if (trimmed === "avg_column") return "column";
  if (!trimmed.startsWith("avg")) return null;

  const rawScope = trimmed.slice(3).trim().toLowerCase();
  if (rawScope.length === 0) return "paragraph";
  if (rawScope === "doc" || rawScope === "all" || rawScope === "file") return "doc";
  if (rawScope === "list") return "list";
  if (rawScope === "row") return "row";
  if (rawScope === "column") return "column";
  return null;
}

export async function executeSumCommand(
  rawCommand: string,
  ctx: ResolvedContext,
  options: SumCommandOptions = {},
): Promise<SumExecutionResult> {
  const trimmed = rawCommand.trim().replace(/^:/, "");
  const scope = parseSumScopeFromCommand(trimmed);
  if (!scope) {
    return { message: `unknown command: ${trimmed}` };
  }

  const range = resolveScopeRangeInContext(ctx, scope);

  if (scope === "row" || scope === "column") {
    if (!range) {
      return { message: `sum(${scope}): no block at cursor` };
    }
    const terms = scope === "row"
      ? await collectRowTerms(ctx, options.evaluateExpression)
      : await collectColumnTerms(ctx, range, options.evaluateExpression);
    if (terms.length === 0) {
      return { message: `sum(${scope}): no numbers` };
    }
    const total = await sumTerms(terms, options.evaluateExpression);
    if (!total) {
      return { message: `sum(${scope}): incompatible units` };
    }
    const operation = replaceTableCellAtCursor(ctx, total);
    return {
      message: `sum(${scope}) = ${total}`,
      operation,
      clipboardText: total,
    };
  }

  const text = range ? ctx.textForLineRange(range) : "";
  const selection = ctx.selection();
  const numbers = parseNumbers(text);
  if (numbers.length === 0) {
    return { message: `sum(${scope}): no numbers` };
  }

  const sum = numbers.reduce((acc, n) => acc + n, 0);
  const formatted = formatNumber(sum);
  const operation = replaceRange(selection.from, selection.to, formatted, {
    anchor: selection.from + formatted.length,
  });

  return {
    message: `sum(${scope}) = ${formatted} (${numbers.length} values, inserted at cursor + copied)`,
    operation,
    clipboardText: formatted,
  };
}

export async function executeAvgCommand(
  rawCommand: string,
  ctx: ResolvedContext,
  options: SumCommandOptions = {},
): Promise<SumExecutionResult> {
  const trimmed = rawCommand.trim().replace(/^:/, "");
  const scope = parseAvgScopeFromCommand(trimmed);
  if (!scope) {
    return { message: `unknown command: ${trimmed}` };
  }

  const range = resolveScopeRangeInContext(ctx, scope);

  if (scope === "row" || scope === "column") {
    if (!range) {
      return { message: `avg(${scope}): no block at cursor` };
    }
    const terms = scope === "row"
      ? await collectRowTerms(ctx, options.evaluateExpression)
      : await collectColumnTerms(ctx, range, options.evaluateExpression);
    if (terms.length === 0) {
      return { message: `avg(${scope}): no numbers` };
    }
    const avg = await averageTerms(terms, options.evaluateExpression);
    if (!avg) {
      return { message: `avg(${scope}): incompatible units` };
    }
    const operation = replaceTableCellAtCursor(ctx, avg);
    return {
      message: `avg(${scope}) = ${avg}`,
      operation,
      clipboardText: avg,
    };
  }

  const text = range ? ctx.textForLineRange(range) : "";
  const selection = ctx.selection();
  const numbers = parseNumbers(text);
  if (numbers.length === 0) {
    return { message: `avg(${scope}): no numbers` };
  }

  const average = numbers.reduce((acc, n) => acc + n, 0) / numbers.length;
  const formatted = formatNumber(average);
  const operation = replaceRange(selection.from, selection.to, formatted, {
    anchor: selection.from + formatted.length,
  });

  return {
    message: `avg(${scope}) = ${formatted} (${numbers.length} values, inserted at cursor + copied)`,
    operation,
    clipboardText: formatted,
  };
}
