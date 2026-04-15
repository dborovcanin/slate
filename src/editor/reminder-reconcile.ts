import type { ChangeDesc, Text } from "@codemirror/state";
import type { NoteReminder } from "../api.ts";

export type MoveReminderLineFn = (
  noteId: string,
  fromLineNumber: number,
  toLineNumber: number,
  lineText: string,
) => Promise<boolean>;

function findNearestAvailableLine(
  candidates: number[],
  preferredLine: number,
  usedLines: Set<number>,
): number | null {
  let best: number | null = null;
  let bestDistance = Number.POSITIVE_INFINITY;
  for (const lineNumber of candidates) {
    if (usedLines.has(lineNumber)) continue;
    const distance = Math.abs(lineNumber - preferredLine);
    if (distance < bestDistance) {
      best = lineNumber;
      bestDistance = distance;
      continue;
    }
    if (distance === bestDistance && best !== null && lineNumber < best) {
      best = lineNumber;
    }
  }
  return best;
}

function clampPos(pos: number, max: number): number {
  return Math.min(Math.max(0, pos), max);
}

function clampLineNumber(lineNumber: number, lineCount: number): number {
  if (lineCount <= 0) return 1;
  return Math.min(Math.max(1, lineNumber), lineCount);
}

function nearestFreeLine(preferredLine: number, lineCount: number, usedLines: Set<number>): number {
  const clamped = clampLineNumber(preferredLine, lineCount);
  if (!usedLines.has(clamped)) return clamped;
  for (let distance = 1; distance <= lineCount; distance += 1) {
    const down = clamped + distance;
    if (down <= lineCount && !usedLines.has(down)) return down;
    const up = clamped - distance;
    if (up >= 1 && !usedLines.has(up)) return up;
  }
  return clamped;
}

export function remindersEqual(left: NoteReminder[], right: NoteReminder[]): boolean {
  if (left.length !== right.length) return false;
  for (let i = 0; i < left.length; i += 1) {
    const a = left[i];
    const b = right[i];
    if (!a || !b) return false;
    if (
      a.note_id !== b.note_id ||
      a.line_number !== b.line_number ||
      a.remind_at_ms !== b.remind_at_ms ||
      a.display_at !== b.display_at ||
      a.line_text !== b.line_text ||
      (a.notified_at_ms ?? null) !== (b.notified_at_ms ?? null)
    ) {
      return false;
    }
  }
  return true;
}

export function remapReminderLinesForDocChange(
  remindersByLine: ReadonlyMap<number, NoteReminder>,
  startDoc: Text,
  changes: ChangeDesc,
  nextDoc: Text,
): Map<number, NoteReminder> {
  if (remindersByLine.size === 0) return new Map();
  if (nextDoc.lines <= 0) return new Map();

  const ordered = [...remindersByLine.values()].sort((a, b) => a.line_number - b.line_number);
  const usedLines = new Set<number>();
  const remapped = new Map<number, NoteReminder>();

  for (const reminder of ordered) {
    let mappedLine = reminder.line_number;
    if (mappedLine >= 1 && mappedLine <= startDoc.lines) {
      const oldLine = startDoc.line(mappedLine);
      const anchor = oldLine.from + (oldLine.length > 0 ? 1 : 0);
      const mappedPos = clampPos(changes.mapPos(anchor, 1), nextDoc.length);
      mappedLine = nextDoc.lineAt(mappedPos).number;
    } else {
      mappedLine = clampLineNumber(mappedLine, nextDoc.lines);
    }

    const targetLine = nearestFreeLine(mappedLine, nextDoc.lines, usedLines);
    usedLines.add(targetLine);
    remapped.set(targetLine, {
      ...reminder,
      line_number: targetLine,
    });
  }

  return remapped;
}

export async function reconcileReminderLinesOnOpen(
  noteId: string,
  reminders: NoteReminder[],
  doc: Text,
  moveReminderLine: MoveReminderLineFn,
): Promise<NoteReminder[]> {
  if (reminders.length === 0 || doc.lines <= 0) {
    return reminders;
  }

  const textToLineNumbers = new Map<string, number[]>();
  for (let lineNumber = 1; lineNumber <= doc.lines; lineNumber += 1) {
    const text = doc.line(lineNumber).text;
    const existing = textToLineNumbers.get(text);
    if (existing) {
      existing.push(lineNumber);
    } else {
      textToLineNumbers.set(text, [lineNumber]);
    }
  }

  const usedLines = new Set<number>();
  const out: NoteReminder[] = [];
  const sorted = [...reminders].sort((a, b) => a.line_number - b.line_number);
  for (const reminder of sorted) {
    const oldLine = reminder.line_number;
    const oldLineInRange = oldLine >= 1 && oldLine <= doc.lines;
    const oldLineText = oldLineInRange ? doc.line(oldLine).text : null;
    if (oldLineText === reminder.line_text && !usedLines.has(oldLine)) {
      usedLines.add(oldLine);
      out.push(reminder);
      continue;
    }

    const candidates = textToLineNumbers.get(reminder.line_text) ?? [];
    const targetLine = findNearestAvailableLine(candidates, oldLine, usedLines);
    if (targetLine === null || targetLine === oldLine) {
      if (oldLineInRange && !usedLines.has(oldLine)) {
        usedLines.add(oldLine);
      }
      out.push(reminder);
      continue;
    }

    const targetText = doc.line(targetLine).text;
    try {
      const moved = await moveReminderLine(noteId, oldLine, targetLine, targetText);
      if (moved) {
        usedLines.add(targetLine);
        out.push({
          ...reminder,
          line_number: targetLine,
          line_text: targetText,
        });
        continue;
      }
    } catch (error) {
      console.error("Reminder line reconcile failed:", error);
    }

    if (oldLineInRange && !usedLines.has(oldLine)) {
      usedLines.add(oldLine);
    }
    out.push(reminder);
  }

  return out.sort((a, b) => a.line_number - b.line_number);
}
