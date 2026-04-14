const tableRowRe = /^\s*\|.*\|\s*$/;
const unorderedListRe = /^(\s*)(->|[-*+])\s+/;
const orderedListRe = /^(\s*)(\d+\.|\d+(?:\.\d+)+)\s+/;
const checklistRe = /^(\s*)(?:->|[-*+]|\d+\.|\d+(?:\.\d+)+)\s+\[(?: |x|X)\]\s+/;

interface TableCalcCell {
  expr: string;
  fromCol: number;
  toCol: number;
}

export function isBuiltinFormula(text: string): boolean {
  const trimmed = text.trim();
  if (trimmed.length === 0) return false;
  const withoutEquals = trimmed.startsWith("=") ? trimmed.slice(1).trim() : trimmed;
  const compact = withoutEquals.replace(/\s+/g, "").toLowerCase();
  const token = compact.endsWith("()") ? compact.slice(0, -2) : compact;
  return (
    token === "sum_row" ||
    token === "avg_row" ||
    token === "sum_col" ||
    token === "sum_column" ||
    token === "avg_col" ||
    token === "avg_column"
  );
}

function hasCalcSignal(text: string): boolean {
  if (text.trim().length === 0) return false;
  if (isBuiltinFormula(text)) return true;
  const hasOperator = /[+\-*/^%()]/.test(text);
  if (hasOperator) return true;
  return text.includes(" to ") || text.includes(" in ");
}

function listBodyRange(lineText: string): { fromCol: number; toCol: number } | null {
  let prefixEnd: number | null = null;

  const checklistMatch = lineText.match(checklistRe);
  if (checklistMatch) {
    prefixEnd = checklistMatch[0].length;
  } else {
    const unorderedMatch = lineText.match(unorderedListRe);
    const orderedMatch = lineText.match(orderedListRe);
    if (unorderedMatch) prefixEnd = unorderedMatch[0].length;
    else if (orderedMatch) prefixEnd = orderedMatch[0].length;
  }

  if (prefixEnd === null) return null;
  const raw = lineText.slice(prefixEnd);
  const leadingWs = raw.match(/^\s*/)?.[0].length ?? 0;
  const trailingWs = raw.match(/\s*$/)?.[0].length ?? 0;
  const fromCol = prefixEnd + leadingWs;
  const toCol = lineText.length - trailingWs;
  if (fromCol >= toCol) return null;
  return { fromCol, toCol };
}

export function findSingleCalcTableCell(lineText: string): TableCalcCell | null {
  if (!tableRowRe.test(lineText)) return null;

  const pipeIdx: number[] = [];
  for (let i = 0; i < lineText.length; i++) {
    if (lineText[i] === "|") pipeIdx.push(i);
  }
  if (pipeIdx.length < 2) return null;

  const formulaCandidates: TableCalcCell[] = [];
  const candidates: TableCalcCell[] = [];
  for (let i = 0; i < pipeIdx.length - 1; i++) {
    const start = pipeIdx[i] + 1;
    const end = pipeIdx[i + 1];
    if (start >= end) continue;

    const raw = lineText.slice(start, end);
    const trimmed = raw.trim();
    if (!hasCalcSignal(trimmed)) continue;

    const leadingWs = raw.match(/^\s*/)?.[0].length ?? 0;
    const trailingWs = raw.match(/\s*$/)?.[0].length ?? 0;
    const fromCol = start + leadingWs;
    const toCol = end - trailingWs;
    if (fromCol >= toCol) continue;

    if (isBuiltinFormula(trimmed)) {
      formulaCandidates.push({ expr: trimmed, fromCol, toCol });
    } else {
      candidates.push({ expr: trimmed, fromCol, toCol });
    }
  }

  if (formulaCandidates.length === 1) return formulaCandidates[0] ?? null;
  if (formulaCandidates.length > 1) return null;
  if (candidates.length !== 1) return null;
  return candidates[0] ?? null;
}

export function findListCalcSegment(lineText: string): TableCalcCell | null {
  const body = listBodyRange(lineText);
  if (!body) return null;
  const trimmed = lineText.slice(body.fromCol, body.toCol).trim();
  if (!hasCalcSignal(trimmed)) return null;
  return { expr: trimmed, fromCol: body.fromCol, toCol: body.toCol };
}

export function findCalcSegment(lineText: string): TableCalcCell | null {
  return findSingleCalcTableCell(lineText) ?? findListCalcSegment(lineText);
}

export function lineForCalcEvaluation(lineText: string): string {
  const segment = findCalcSegment(lineText);
  if (segment) return segment.expr;

  if (tableRowRe.test(lineText)) return "";

  const body = listBodyRange(lineText);
  if (body) {
    const value = lineText.slice(body.fromCol, body.toCol).trim();
    return hasCalcSignal(value) ? value : "";
  }

  return lineText;
}

export type { TableCalcCell };
