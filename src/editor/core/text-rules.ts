import { ResolvedContext } from "./context.ts";
import { formatTableLines } from "./markdown-table.ts";
import { replaceRange } from "./operations.ts";
import type { EditOperation, EditorContextSnapshot } from "./types.ts";

const listRe = /^(\s*)([-*+]|\d+\.)\s+(.*)$/;
const checklistRe = /^(\s*(?:[-*+]|\d+\.)\s+)\[( |x|X)\]\s+(.*)$/;
const checklistToggleSuffixRe = /\/x$/i;

function clamp(value: number, min: number, max: number): number {
  return Math.min(max, Math.max(min, value));
}

function stripChecklistToggleSuffix(content: string): string | null {
  const trimmedEnd = content.replace(/\s+$/g, "");
  if (!checklistToggleSuffixRe.test(trimmedEnd)) return null;
  const slashPos = trimmedEnd.length - 2;
  if (slashPos > 0 && !/\s/.test(trimmedEnd[slashPos - 1] ?? "")) return null;
  return trimmedEnd.slice(0, slashPos).replace(/\s+$/g, "");
}

export function rewriteLineWithChecklistToggleSuffix(lineText: string): string | null {
  const checklistMatch = lineText.match(checklistRe);
  if (checklistMatch) {
    const prefix = checklistMatch[1];
    const marker = checklistMatch[2];
    const content = checklistMatch[3];
    const nextContent = stripChecklistToggleSuffix(content);
    if (nextContent === null) return null;
    const nextMarker = marker.toLowerCase() === "x" ? " " : "x";
    return `${prefix}[${nextMarker}] ${nextContent}`;
  }

  const listMatch = lineText.match(listRe);
  if (!listMatch) return null;
  const indent = listMatch[1];
  const marker = listMatch[2];
  const content = listMatch[3];
  const nextContent = stripChecklistToggleSuffix(content);
  if (nextContent === null) return null;
  return `${indent}${marker} [x] ${nextContent}`;
}

function checklistToggleRule(ctx: ResolvedContext): EditOperation | null {
  const selection = ctx.selection();
  if (!selection.empty) return null;

  const line = ctx.currentLine();
  if (selection.head !== line.to) return null;

  const replacement = rewriteLineWithChecklistToggleSuffix(line.text);
  if (!replacement || replacement === line.text) return null;

  return replaceRange(line.from, line.to, replacement, {
    anchor: line.from + replacement.length,
  });
}

function tableAutoformatRule(ctx: ResolvedContext): EditOperation | null {
  const line = ctx.currentLine();
  const block = ctx.tableRangeAtLine(line.number, 2);
  if (!block) return null;

  const lines: string[] = [];
  for (let n = block.startLine; n <= block.endLine; n++) {
    lines.push(ctx.lineText(n));
  }

  const formatted = formatTableLines(lines);
  if (formatted.every((entry, idx) => entry === lines[idx])) {
    return null;
  }

  const head = ctx.selection().head;
  const headLine = ctx.lineAt(head).number;
  const headCol = head - ctx.line(headLine).from;
  const relativeLine = clamp(headLine - block.startLine, 0, formatted.length - 1);

  const startLine = ctx.line(block.startLine);
  const endLine = ctx.line(block.endLine);

  let newHead = startLine.from;
  for (let i = 0; i < relativeLine; i++) {
    newHead += formatted[i].length + 1;
  }
  newHead += Math.min(headCol, formatted[relativeLine].length);

  return replaceRange(startLine.from, endLine.to, formatted.join("\n"), {
    anchor: newHead,
  });
}

function listContinuationRule(ctx: ResolvedContext): EditOperation | null {
  const selection = ctx.selection();
  if (!selection.empty) return null;

  const line = ctx.currentLine();
  const before = line.text.slice(0, selection.head - line.from);
  const match = before.match(listRe);
  if (!match) return null;

  const indent = match[1];
  const marker = match[2];
  const content = match[3];

  if (content.trim().length === 0 && selection.head === line.to) {
    const markerFrom = line.from + indent.length;
    return replaceRange(markerFrom, line.to, "", { anchor: markerFrom });
  }

  let nextMarker = marker;
  if (/^\d+\.$/.test(marker)) {
    const value = Number.parseInt(marker.slice(0, -1), 10);
    if (Number.isFinite(value)) nextMarker = `${value + 1}.`;
  }

  const insert = `\n${indent}${nextMarker} `;
  return replaceRange(selection.head, selection.head, insert, {
    anchor: selection.head + insert.length,
  });
}

export interface TextRuleOptions {
  markdownAutoformat?: boolean;
}

export function runDocChangeRules(
  snapshot: EditorContextSnapshot,
  options: TextRuleOptions = {},
): EditOperation | null {
  const ctx = new ResolvedContext(snapshot);

  const checklistOp = checklistToggleRule(ctx);
  if (checklistOp) return checklistOp;

  if (!(options.markdownAutoformat ?? true)) return null;
  return tableAutoformatRule(ctx);
}

export function runEnterRules(
  snapshot: EditorContextSnapshot,
  options: TextRuleOptions = {},
): EditOperation | null {
  if (!(options.markdownAutoformat ?? true)) return null;
  const ctx = new ResolvedContext(snapshot);
  return listContinuationRule(ctx);
}
