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

function parseTableDataRows(text: string): string[][] {
  const rows: string[][] = [];
  for (const line of text.split("\n")) {
    if (!tableLineRe.test(line)) continue;
    const cells = splitTableCells(line);
    if (isTableDelimiterRowCells(cells)) continue;
    rows.push(cells);
  }
  return rows;
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

async function sumRows(
  text: string,
  evaluateExpression?: SumExpressionEvaluator,
): Promise<string[]> {
  const rows = parseTableDataRows(text);
  const out: string[] = [];

  for (const row of rows) {
    const evaluated = await Promise.all(row.map((cell) => evaluateCellValue(cell, evaluateExpression)));
    const terms = evaluated.filter((value): value is string => value !== null);
    const summed = await sumTerms(terms, evaluateExpression);
    if (summed) out.push(summed);
  }

  return out;
}

async function sumColumns(
  text: string,
  evaluateExpression?: SumExpressionEvaluator,
): Promise<string[]> {
  const rows = parseTableDataRows(text);
  const columnCount = rows.reduce((max, row) => Math.max(max, row.length), 0);
  const out: string[] = [];

  for (let col = 0; col < columnCount; col++) {
    const evaluated = await Promise.all(
      rows.map((row) => evaluateCellValue(row[col] ?? "", evaluateExpression)),
    );
    const terms = evaluated.filter((value): value is string => value !== null);
    const summed = await sumTerms(terms, evaluateExpression);
    if (summed) out.push(summed);
  }

  return out;
}

async function averageRows(
  text: string,
  evaluateExpression?: SumExpressionEvaluator,
): Promise<string[]> {
  const rows = parseTableDataRows(text);
  const out: string[] = [];

  for (const row of rows) {
    const evaluated = await Promise.all(row.map((cell) => evaluateCellValue(cell, evaluateExpression)));
    const terms = evaluated.filter((value): value is string => value !== null);
    const averaged = await averageTerms(terms, evaluateExpression);
    if (averaged) out.push(averaged);
  }

  return out;
}

async function averageColumns(
  text: string,
  evaluateExpression?: SumExpressionEvaluator,
): Promise<string[]> {
  const rows = parseTableDataRows(text);
  const columnCount = rows.reduce((max, row) => Math.max(max, row.length), 0);
  const out: string[] = [];

  for (let col = 0; col < columnCount; col++) {
    const evaluated = await Promise.all(
      rows.map((row) => evaluateCellValue(row[col] ?? "", evaluateExpression)),
    );
    const terms = evaluated.filter((value): value is string => value !== null);
    const averaged = await averageTerms(terms, evaluateExpression);
    if (averaged) out.push(averaged);
  }

  return out;
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
      return ctx.tableRangeAtLine(lineNo);
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
  const text = range ? ctx.textForLineRange(range) : "";
  const selection = ctx.selection();

  if (scope === "row" || scope === "column") {
    const totals =
      scope === "row"
        ? await sumRows(text, options.evaluateExpression)
        : await sumColumns(text, options.evaluateExpression);
    if (totals.length === 0) {
      return { message: `sum(${scope}): no numbers` };
    }

    const formatted = totals.join("\n");
    const operation = replaceRange(selection.from, selection.to, formatted, {
      anchor: selection.from + formatted.length,
    });
    return {
      message: `sum(${scope}) = [${totals.join(", ")}] (${totals.length} totals, inserted at cursor + copied)`,
      operation,
      clipboardText: formatted,
    };
  }

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
  const text = range ? ctx.textForLineRange(range) : "";
  const selection = ctx.selection();

  if (scope === "row" || scope === "column") {
    const averages =
      scope === "row"
        ? await averageRows(text, options.evaluateExpression)
        : await averageColumns(text, options.evaluateExpression);
    if (averages.length === 0) {
      return { message: `avg(${scope}): no numbers` };
    }

    const formatted = averages.join("\n");
    const operation = replaceRange(selection.from, selection.to, formatted, {
      anchor: selection.from + formatted.length,
    });
    return {
      message: `avg(${scope}) = [${averages.join(", ")}] (${averages.length} averages, inserted at cursor + copied)`,
      operation,
      clipboardText: formatted,
    };
  }

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
