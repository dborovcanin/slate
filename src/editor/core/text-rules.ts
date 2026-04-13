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
const tableLineRe = /^\s*\|.*\|\s*$/;
const tableSeparatorRe = /^\s*\|(?=.*-)[\s:|-]+\|\s*$/;

function clamp(value: number, min: number, max: number): number {
  return Math.min(max, Math.max(min, value));
}

function tablePipePositions(line: string): number[] {
  const pipes: number[] = [];
  for (let i = 0; i < line.length; i++) {
    if (line[i] === "|") pipes.push(i);
  }
  return pipes;
}

function tableCellIndexForColumn(pipes: number[], col: number): number {
  if (pipes.length < 2) return -1;
  for (let i = 0; i < pipes.length - 1; i++) {
    if (col <= pipes[i + 1]) return i;
  }
  return pipes.length - 2;
}

function firstNonSpaceOffset(text: string): number {
  for (let i = 0; i < text.length; i++) {
    if (text[i] !== " ") return i;
  }
  return text.length;
}

function lastNonSpaceEndOffset(text: string): number {
  for (let i = text.length - 1; i >= 0; i--) {
    if (text[i] !== " ") return i + 1;
  }
  return 0;
}

function mapTableCursorColumn(sourceLine: string, targetLine: string, sourceCol: number): number {
  const sourcePipes = tablePipePositions(sourceLine);
  const targetPipes = tablePipePositions(targetLine);
  if (sourcePipes.length < 2 || targetPipes.length < 2) {
    return Math.min(sourceCol, targetLine.length);
  }

  const sourceCellIndex = tableCellIndexForColumn(sourcePipes, sourceCol);
  if (sourceCellIndex < 0) return Math.min(sourceCol, targetLine.length);
  const targetCellIndex = clamp(sourceCellIndex, 0, targetPipes.length - 2);

  const sourceLeft = sourcePipes[sourceCellIndex] + 1;
  const sourceRight = sourcePipes[sourceCellIndex + 1];
  const sourceRaw = sourceLine.slice(sourceLeft, sourceRight);
  const sourceTrimStart = firstNonSpaceOffset(sourceRaw);
  const sourceTrimEnd = lastNonSpaceEndOffset(sourceRaw);
  const sourceContentLen = Math.max(0, sourceTrimEnd - sourceTrimStart);
  const sourceInCell = clamp(sourceCol - sourceLeft, 0, sourceRaw.length);

  let semanticOffset = 0;
  if (sourceContentLen > 0) {
    if (sourceInCell <= sourceTrimStart) {
      semanticOffset = 0;
    } else if (sourceInCell >= sourceTrimEnd) {
      semanticOffset = sourceContentLen;
    } else {
      semanticOffset = sourceInCell - sourceTrimStart;
    }
  }

  const targetLeft = targetPipes[targetCellIndex] + 1;
  const targetRight = targetPipes[targetCellIndex + 1];
  const targetRaw = targetLine.slice(targetLeft, targetRight);
  const targetTrimStart = firstNonSpaceOffset(targetRaw);
  const targetTrimEnd = lastNonSpaceEndOffset(targetRaw);
  const targetContentLen = Math.max(0, targetTrimEnd - targetTrimStart);

  if (targetContentLen === 0) {
    return Math.min(targetLeft + 1, targetRight);
  }

  const mappedInTarget = targetTrimStart + Math.min(semanticOffset, targetContentLen);
  return clamp(targetLeft + mappedInTarget, 0, targetLine.length);
}

function ensureCellAnchorKeepsLeadingSpace(
  lineText: string,
  pipes: number[],
  leftPipeIndex: number,
  anchorInLine: number,
): number {
  const leftPipe = pipes[leftPipeIndex];
  const rightPipe = pipes[leftPipeIndex + 1];
  if (leftPipe === undefined || rightPipe === undefined) return anchorInLine;
  const cellStart = leftPipe + 1;
  if (anchorInLine !== cellStart) return anchorInLine;
  if (rightPipe <= cellStart) return anchorInLine;
  if (lineText[cellStart] === " ") return Math.min(cellStart + 1, rightPipe);
  return anchorInLine;
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
  return formatOrderedMarker([...parts, 1]);
}

function outdentOrderedMarker(marker: string): string {
  const parts = parseOrderedMarker(marker);
  if (!parts) return marker;
  if (parts.length === 1) return formatOrderedMarker(parts);
  return formatOrderedMarker(parts.slice(0, -1));
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
  const sourceLine = lines[relativeLine] ?? "";
  const targetLine = formatted[relativeLine] ?? "";
  const mappedHeadCol =
    tableLineRe.test(sourceLine) && tableLineRe.test(targetLine)
      ? mapTableCursorColumn(sourceLine, targetLine, headCol)
      : Math.min(headCol, targetLine.length);

  const startLine = ctx.line(block.startLine);
  const endLine = ctx.line(block.endLine);

  let newHead = startLine.from;
  for (let i = 0; i < relativeLine; i++) {
    newHead += formatted[i].length + 1;
  }
  newHead += mappedHeadCol;

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

export function runDocChangeRules(snapshot: EditorContextSnapshot, options: TextRuleOptions = {}): EditOperation | null {
  const ctx = new ResolvedContext(snapshot);

  const checklistOp = checklistToggleRule(ctx);
  if (checklistOp) return checklistOp;

  if (!(options.markdownAutoformat ?? true)) return null;
  
  const tableOp = tableAutoformatRule(ctx);
  if (tableOp) return tableOp;

  return listAutoformatRule(ctx);
}

function tableContinuationRule(ctx: ResolvedContext): EditOperation | null {
  const selection = ctx.selection();
  if (!selection.empty) return null;

  const line = ctx.currentLine();
  if (!tableLineRe.test(line.text)) return null;
  const atLineEnd = selection.head === line.to;

  // Count columns by splitting on |
  const cells = line.text.split("|");
  // First and last are outside the pipes, inner ones are cells
  const columnCount = Math.max(cells.length - 2, 1);

  // Check if this is a separator row — skip continuation
  if (tableSeparatorRe.test(line.text)) return null;

  // Check if the row is "empty" (only pipes and whitespace) — exit table
  const innerContent = cells.slice(1, -1).join("").trim();
  if (innerContent.length === 0) {
    // Remove the empty row and place cursor on next line
    return replaceRange(line.from, line.to, "", { anchor: line.from });
  }
  if (!atLineEnd) return null;

  // Build an empty row with matching column count
  const emptyRow = "|" + " |".repeat(columnCount);
  const insert = `\n${emptyRow}`;
  // Place cursor after first pipe + space in new row
  const anchor = selection.head + 1 + 2; // \n + | + space
  return replaceRange(selection.head, selection.head, insert, { anchor });
}

export function runEnterRules(
  snapshot: EditorContextSnapshot,
  options: TextRuleOptions = {},
): EditOperation | null {
  if (!(options.markdownAutoformat ?? true)) return null;
  const ctx = new ResolvedContext(snapshot);
  const tableOp = tableContinuationRule(ctx);
  if (tableOp) return tableOp;
  return listContinuationRule(ctx);
}

function tableTabRule(ctx: ResolvedContext, options: TabRuleOptions): EditOperation | null {
  const selection = ctx.selection();
  if (!selection.empty) return null;

  let currentLineIdx = ctx.currentLine().number;
  let headCol = selection.head - ctx.line(currentLineIdx).from;
  const outdent = options.outdent ?? false;

  let foundTarget = false;
  let targetAnchor = -1;

  while (currentLineIdx >= 1 && currentLineIdx <= ctx.lineCount()) {
    const line = ctx.line(currentLineIdx);
    if (!tableLineRe.test(line.text)) break;
    
    // Skip separator lines when wrapping
    if (tableSeparatorRe.test(line.text) && currentLineIdx !== ctx.currentLine().number) {
      currentLineIdx += outdent ? -1 : 1;
      continue;
    }

    const pipes: number[] = [];
    for (let i = 0; i < line.text.length; i++) {
      if (line.text[i] === "|") pipes.push(i);
    }
    if (pipes.length < 2) break;

    if (outdent) {
      let leftPipe = -1;
      for (let i = pipes.length - 1; i >= 0; i--) {
        if (pipes[i] < headCol) {
          leftPipe = i;
          break;
        }
      }
      if (leftPipe > 0) {
        const targetPipeIndex = pipes[leftPipe];
        let pos = targetPipeIndex;
        while (pos > pipes[leftPipe - 1] + 1 && line.text[pos - 1] === " ") {
          pos--;
        }
        pos = ensureCellAnchorKeepsLeadingSpace(line.text, pipes, leftPipe - 1, pos);
        targetAnchor = line.from + pos;
        foundTarget = true;
        break;
      } else {
        currentLineIdx--;
        if (currentLineIdx >= 1) {
          const prevLine = ctx.line(currentLineIdx);
          if (tableLineRe.test(prevLine.text)) {
            headCol = prevLine.text.length;
            continue;
          }
        }
        break;
      }
    } else {
      let rightPipe = -1;
      for (let i = 0; i < pipes.length; i++) {
        if (pipes[i] > headCol) {
          rightPipe = i;
          break;
        }
      }
      if (rightPipe !== -1 && rightPipe + 1 < pipes.length) {
        const targetPipeIndex = pipes[rightPipe + 1];
        let pos = targetPipeIndex;
        while (pos > pipes[rightPipe] + 1 && line.text[pos - 1] === " ") {
          pos--;
        }
        pos = ensureCellAnchorKeepsLeadingSpace(line.text, pipes, rightPipe, pos);
        targetAnchor = line.from + pos;
        foundTarget = true;
        break;
      } else {
        currentLineIdx++;
        if (currentLineIdx <= ctx.lineCount()) {
          const nextLine = ctx.line(currentLineIdx);
          if (tableLineRe.test(nextLine.text)) {
            headCol = 0;
            continue;
          }
        }
        break;
      }
    }
  }

  if (foundTarget) {
    return { changes: [], selection: { anchor: targetAnchor } };
  }
  return null;
}

export function runTabRules(
  snapshot: EditorContextSnapshot,
  options: TextRuleOptions & TabRuleOptions = {},
): EditOperation | null {
  if (!(options.markdownAutoformat ?? true)) return null;
  const ctx = new ResolvedContext(snapshot);
  const tableOp = tableTabRule(ctx, options);
  if (tableOp) return tableOp;
  return listTabRule(ctx, options);
}

export function runTableCellNavigationRules(
  snapshot: EditorContextSnapshot,
  options: TextRuleOptions & TabRuleOptions = {},
): EditOperation | null {
  if (!(options.markdownAutoformat ?? true)) return null;
  const ctx = new ResolvedContext(snapshot);
  return tableTabRule(ctx, options);
}
