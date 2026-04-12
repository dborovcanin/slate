import type {
  BlockLineRange,
  EditorContextSnapshot,
  LineContext,
  SelectionContext,
  WordContext,
} from "./types.ts";

const listLineRe = /^\s*(?:[-*+]|\d+\.)\s+/;
const tableLineRe = /^\s*\|.*\|\s*$/;

interface ParsedLines {
  starts: number[];
  lines: string[];
}

function clamp(value: number, min: number, max: number): number {
  return Math.min(max, Math.max(min, value));
}

function parseLines(text: string): ParsedLines {
  if (text.length === 0) {
    return { starts: [0], lines: [""] };
  }

  const starts: number[] = [];
  const lines: string[] = [];
  let start = 0;

  for (let i = 0; i < text.length; i++) {
    if (text[i] !== "\n") continue;
    starts.push(start);
    lines.push(text.slice(start, i));
    start = i + 1;
  }

  starts.push(start);
  lines.push(text.slice(start));
  return { starts, lines };
}

export class ResolvedContext {
  private readonly snapshot: EditorContextSnapshot;
  private parsedLinesCache: ParsedLines | null = null;
  private selectionCache: SelectionContext | null = null;

  constructor(snapshot: EditorContextSnapshot) {
    this.snapshot = snapshot;
  }

  text(): string {
    return this.snapshot.text;
  }

  selection(): SelectionContext {
    if (this.selectionCache) return this.selectionCache;

    const max = this.snapshot.text.length;
    const anchor = clamp(this.snapshot.selection.anchor, 0, max);
    const head = clamp(this.snapshot.selection.head, 0, max);
    const from = Math.min(anchor, head);
    const to = Math.max(anchor, head);

    this.selectionCache = {
      anchor,
      head,
      from,
      to,
      empty: from === to,
    };
    return this.selectionCache;
  }

  cursorPos(): number {
    return this.selection().head;
  }

  changedRange() {
    return this.snapshot.changedRange;
  }

  lineCount(): number {
    return this.parsedLines().lines.length;
  }

  line(number: number): LineContext {
    const parsed = this.parsedLines();
    const idx = clamp(number - 1, 0, parsed.lines.length - 1);
    return this.lineFromIndex(idx);
  }

  lineAt(pos: number): LineContext {
    const parsed = this.parsedLines();
    const p = clamp(pos, 0, this.snapshot.text.length);

    let lo = 0;
    let hi = parsed.starts.length - 1;
    while (lo < hi) {
      const mid = Math.floor((lo + hi + 1) / 2);
      if (parsed.starts[mid] <= p) {
        lo = mid;
      } else {
        hi = mid - 1;
      }
    }

    return this.lineFromIndex(lo);
  }

  currentLine(): LineContext {
    return this.lineAt(this.cursorPos());
  }

  currentColumn(): number {
    const line = this.currentLine();
    return this.cursorPos() - line.from;
  }

  lineText(number: number): string {
    return this.line(number).text;
  }

  textForLineRange(range: BlockLineRange): string {
    const lines: string[] = [];
    for (let n = range.startLine; n <= range.endLine; n++) {
      lines.push(this.lineText(n));
    }
    return lines.join("\n");
  }

  paragraphRangeAtLine(lineNumber: number): BlockLineRange {
    const lineCount = this.lineCount();
    const cursor = clamp(lineNumber, 1, lineCount);

    let start = cursor;
    let end = cursor;

    while (start > 1 && this.lineText(start - 1).trim().length > 0) start--;
    while (end < lineCount && this.lineText(end + 1).trim().length > 0) end++;

    return { startLine: start, endLine: end };
  }

  listRangeAtLine(lineNumber: number): BlockLineRange | null {
    const lineCount = this.lineCount();
    const cursor = clamp(lineNumber, 1, lineCount);
    if (!listLineRe.test(this.lineText(cursor))) return null;

    let start = cursor;
    let end = cursor;

    while (start > 1 && listLineRe.test(this.lineText(start - 1))) start--;
    while (end < lineCount && listLineRe.test(this.lineText(end + 1))) end++;

    return { startLine: start, endLine: end };
  }

  tableRangeAtLine(lineNumber: number, minRows = 2): BlockLineRange | null {
    const lineCount = this.lineCount();
    const cursor = clamp(lineNumber, 1, lineCount);
    if (!tableLineRe.test(this.lineText(cursor))) return null;

    let start = cursor;
    let end = cursor;

    while (start > 1 && tableLineRe.test(this.lineText(start - 1))) start--;
    while (end < lineCount && tableLineRe.test(this.lineText(end + 1))) end++;

    if (end - start + 1 < minRows) return null;
    return { startLine: start, endLine: end };
  }

  wordAt(pos = this.cursorPos()): WordContext | null {
    const text = this.snapshot.text;
    if (text.length === 0) return null;

    const p = clamp(pos, 0, text.length);
    const leftChar = p > 0 ? text[p - 1] : "";
    const rightChar = p < text.length ? text[p] : "";

    const isWordChar = (ch: string) => /[A-Za-z0-9_]/.test(ch);
    if (!isWordChar(leftChar) && !isWordChar(rightChar)) return null;

    let from = p;
    let to = p;
    while (from > 0 && isWordChar(text[from - 1] ?? "")) from--;
    while (to < text.length && isWordChar(text[to] ?? "")) to++;

    if (from >= to) return null;
    return { from, to, text: text.slice(from, to) };
  }

  positionForLineColumn(lineNumber: number, column: number): number {
    const line = this.line(lineNumber);
    return line.from + clamp(column, 0, line.text.length);
  }

  private parsedLines(): ParsedLines {
    if (!this.parsedLinesCache) {
      this.parsedLinesCache = parseLines(this.snapshot.text);
    }
    return this.parsedLinesCache;
  }

  private lineFromIndex(idx: number): LineContext {
    const parsed = this.parsedLines();
    const text = parsed.lines[idx] ?? "";
    const from = parsed.starts[idx] ?? 0;
    return {
      number: idx + 1,
      from,
      to: from + text.length,
      text,
    };
  }
}
