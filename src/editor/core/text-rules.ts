import { ResolvedContext } from "./context.ts";
import { formatTableLines } from "./markdown-table.ts";
import { replaceRange } from "./operations.ts";
import type { EditOperation, EditorContextSnapshot } from "./types.ts";

const orderedMarkerPattern = "(?:\\d+\\.|\\d+(?:\\.\\d+)+)";
const unorderedMarkerPattern = "(?:->|[-*+])";
const listMarkerPattern = `(?:${unorderedMarkerPattern}|${orderedMarkerPattern})`;

const listRe = new RegExp(`^(\\s*)(${listMarkerPattern})\\s+(.*)$`);
const checklistRe = new RegExp(`^(\\s*${listMarkerPattern}\\s+)\\[( |x|X)\\]\\s+(.*)$`);
const checklistToggleSuffixRe = /\/x$/i;

const orderedTopLevelRe = /^\d+\.$/;
const orderedNestedRe = /^\d+(?:\.\d+)+$/;

function clamp(value: number, min: number, max: number): number {
  return Math.min(max, Math.max(min, value));
}

function parseOrderedMarker(marker: string): number[] | null {
  if (orderedTopLevelRe.test(marker)) {
    const value = Number.parseInt(marker.slice(0, -1), 10);
    return Number.isFinite(value) ? [value] : null;
  }
  if (!orderedNestedRe.test(marker)) return null;
  const parts = marker
    .split(".")
    .map((entry) => Number.parseInt(entry, 10))
    .filter((entry) => Number.isFinite(entry));
  return parts.length > 1 ? parts : null;
}

function formatOrderedMarker(parts: number[]): string {
  if (parts.length <= 1) {
    const first = parts[0] ?? 1;
    return `${first}.`;
  }
  return parts.join(".");
}

function incrementOrderedMarker(marker: string): string {
  const parts = parseOrderedMarker(marker);
  if (!parts) return marker;
  const next = [...parts];
  next[next.length - 1] = (next[next.length - 1] ?? 0) + 1;
  return formatOrderedMarker(next);
}

function indentOrderedMarker(marker: string): string {
  const parts = parseOrderedMarker(marker);
  if (!parts) return marker;
  const next = [...parts];
  // When indenting 3. into a child, it becomes 2.1 (child of parent 2)
  if (next[next.length - 1] > 1) {
    next[next.length - 1] = next[next.length - 1] - 1;
  }
  return formatOrderedMarker([...next, 1]);
}

function outdentOrderedMarker(marker: string): string {
  const parts = parseOrderedMarker(marker);
  if (!parts) return marker;
  if (parts.length === 1) return formatOrderedMarker(parts);
  const next = parts.slice(0, -1);
  // Reverse of indent's decrement: 2.1 outdents to 3.
  next[next.length - 1] = next[next.length - 1] + 1;
  return formatOrderedMarker(next);
}

function markerDepth(indent: string): number {
  return Math.floor(indent.length / 2);
}

function unorderedMarkerForDepth(depth: number): string {
  if (depth <= 0) return "-";
  if (depth === 1) return "*";
  return "->";
}

function isUnorderedMarker(marker: string): boolean {
  return marker === "-" || marker === "*" || marker === "+" || marker === "->";
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

  // If only the marker remains with no content (or just a checkbox), clear the line
  const contentAfterCheckbox = content.replace(/^\[[ xX]\]\s*/, "");
  if (contentAfterCheckbox.trim().length === 0 && selection.head === line.to) {
    const markerFrom = line.from + indent.length;
    return replaceRange(markerFrom, line.to, "", { anchor: markerFrom });
  }

  let nextMarker = marker;
  if (parseOrderedMarker(marker)) {
    nextMarker = incrementOrderedMarker(marker);
  }

  // Detect if current line is a checklist item and add unchecked box
  const checkboxPrefix = /^\[[ xX]\]\s+/.test(content) ? "[ ] " : "";

  const insert = `\n${indent}${nextMarker} ${checkboxPrefix}`;
  return replaceRange(selection.head, selection.head, insert, {
    anchor: selection.head + insert.length,
  });
}

export interface TabRuleOptions {
  outdent?: boolean;
}

function lineRangeForSelection(ctx: ResolvedContext): { startLine: number; endLine: number } {
  const selection = ctx.selection();
  const startLine = ctx.lineAt(selection.from).number;
  let endLine = ctx.lineAt(selection.to).number;

  if (!selection.empty) {
    const endLineCtx = ctx.lineAt(selection.to);
    if (selection.to === endLineCtx.from && endLineCtx.number > startLine) {
      endLine = endLineCtx.number - 1;
    }
  } else {
    endLine = startLine;
  }

  return { startLine, endLine };
}

function listTabRule(ctx: ResolvedContext, options: TabRuleOptions): EditOperation | null {
  const outdent = options.outdent ?? false;
  const { startLine, endLine } = lineRangeForSelection(ctx);
  const changes: EditOperation["changes"] = [];

  for (let lineNo = startLine; lineNo <= endLine; lineNo++) {
    const line = ctx.line(lineNo);
    const match = line.text.match(listRe);
    if (!match) continue;

    const indent = match[1];
    const marker = match[2];
    const content = match[3];

    const currentDepth = markerDepth(indent);
    const nextDepth = outdent ? Math.max(0, currentDepth - 1) : currentDepth + 1;
    const nextIndent = " ".repeat(nextDepth * 2);

    let nextMarker = marker;
    if (parseOrderedMarker(marker)) {
      nextMarker = outdent ? outdentOrderedMarker(marker) : indentOrderedMarker(marker);
    } else if (isUnorderedMarker(marker)) {
      nextMarker = unorderedMarkerForDepth(nextDepth);
    }

    const replacement = `${nextIndent}${nextMarker} ${content}`;
    if (replacement === line.text) continue;
    changes.push({ from: line.from, to: line.to, insert: replacement });
  }

  if (changes.length === 0) return null;
  return { changes };
}

export interface TextRuleOptions {
  markdownAutoformat?: boolean;
}

function listAutoformatRule(ctx: ResolvedContext): EditOperation | null {
  const line = ctx.currentLine();
  const block = ctx.listRangeAtLine(line.number);
  if (!block) return null;

  const lines: string[] = [];
  for (let n = block.startLine; n <= block.endLine; n++) {
    lines.push(ctx.lineText(n));
  }

  const formatted: string[] = [];
  const expectedHierarchy: number[] = [];

  let initialized = false;

  for (let i = 0; i < lines.length; i++) {
    const text = lines[i];
    const match = text.match(listRe);
    if (!match) {
      formatted.push(text);
      continue;
    }

    const indent = match[1];
    const marker = match[2];
    const content = match[3];

    const depth = markerDepth(indent);
    const parts = parseOrderedMarker(marker);

    if (parts) {
      if (!initialized) {
        initialized = true;
        for (let j = 0; j <= depth; j++) {
          expectedHierarchy[j] = parts[j] ?? 1;
        }
        expectedHierarchy[depth] = Math.max(0, expectedHierarchy[depth] - 1);
      }

      expectedHierarchy[depth] = (expectedHierarchy[depth] || 0) + 1;
      expectedHierarchy.length = depth + 1;

      for (let j = 0; j < depth; j++) {
        if (expectedHierarchy[j] === undefined || expectedHierarchy[j] === 0) {
          expectedHierarchy[j] = 1;
        }
      }

      const nextMarker = formatOrderedMarker(expectedHierarchy);
      formatted.push(`${indent}${nextMarker} ${content}`);
    } else {
      formatted.push(text);
    }
  }

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

export function runDocChangeRules(
  snapshot: EditorContextSnapshot,
  options: TextRuleOptions = {},
): EditOperation | null {
  const ctx = new ResolvedContext(snapshot);

  const checklistOp = checklistToggleRule(ctx);
  if (checklistOp) return checklistOp;

  if (!(options.markdownAutoformat ?? true)) return null;
  
  const tableOp = tableAutoformatRule(ctx);
  if (tableOp) return tableOp;

  return listAutoformatRule(ctx);
}

export function runEnterRules(
  snapshot: EditorContextSnapshot,
  options: TextRuleOptions = {},
): EditOperation | null {
  if (!(options.markdownAutoformat ?? true)) return null;
  const ctx = new ResolvedContext(snapshot);
  return listContinuationRule(ctx);
}

export function runTabRules(
  snapshot: EditorContextSnapshot,
  options: TextRuleOptions & TabRuleOptions = {},
): EditOperation | null {
  if (!(options.markdownAutoformat ?? true)) return null;
  const ctx = new ResolvedContext(snapshot);
  return listTabRule(ctx, options);
}
