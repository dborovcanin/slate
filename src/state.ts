import type { Note } from "./api";

export type EventType = "note-changed" | "notes-updated";
type Listener = (event: EventType) => void;

const TITLE_PREVIEW_LIMIT = 60;

function normalizeTitle(line: string): string {
  const trimmed = line.trim();
  if (trimmed.length === 0) return "Untitled";
  return trimmed.length > TITLE_PREVIEW_LIMIT
    ? trimmed.slice(0, TITLE_PREVIEW_LIMIT) + "..."
    : trimmed;
}

export function deriveTitle(body: string): string {
  let lineStart = 0;
  while (lineStart <= body.length) {
    const newline = body.indexOf("\n", lineStart);
    const lineEnd = newline >= 0 ? newline : body.length;
    const title = normalizeTitle(body.slice(lineStart, lineEnd));
    if (title !== "Untitled") return title;
    if (newline < 0) break;
    lineStart = newline + 1;
  }
  return "Untitled";
}

export interface NoteEntry {
  id: string;
  title: string;
  updatedAt: string;
}

export class AppState {
  private _activeNote: Note | null = null;
  private _notes: NoteEntry[] = [];
  private listeners: Listener[] = [];

  get activeNote(): Note | null {
    return this._activeNote;
  }

  get notes(): NoteEntry[] {
    return this._notes;
  }

  setActiveNote(note: Note) {
    this._activeNote = note;
    this.emit("note-changed");
  }

  updateDraftTitle(title: string) {
    const active = this._activeNote;
    if (!active) return;

    const idx = this._notes.findIndex((n) => n.id === active.id);
    if (idx < 0) return;

    const current = this._notes[idx];
    if (!current) return;
    const nextTitle = normalizeTitle(title);
    if (idx === 0 && current.title === nextTitle) return;

    const updated: NoteEntry = { ...current, title: nextTitle };
    this._notes.splice(idx, 1);
    this._notes.unshift(updated);
    this.emit("note-changed");
  }

  updateBody(body: string) {
    const active = this._activeNote;
    if (!active) return;

    const updatedAt = new Date().toISOString();
    const updated = { ...active, body, updated_at: updatedAt };
    this._activeNote = updated;
    const idx = this._notes.findIndex((n) => n.id === updated.id);

    if (idx >= 0) {
      const entry: NoteEntry = {
        ...this._notes[idx],
        title: deriveTitle(body),
        updatedAt,
      };
      this._notes.splice(idx, 1);
      this._notes.unshift(entry);
    }

    this.emit("note-changed");
  }

  setNotes(notes: Note[]) {
    this._notes = notes.map((n) => ({
      id: n.id,
      title: deriveTitle(n.body),
      updatedAt: n.updated_at,
    }));
    this.emit("notes-updated");
  }

  addNote(note: Note) {
    this._notes.unshift({
      id: note.id,
      title: deriveTitle(note.body),
      updatedAt: note.updated_at,
    });
    this.emit("notes-updated");
  }

  removeNote(id: string) {
    this._notes = this._notes.filter((n) => n.id !== id);
    this.emit("notes-updated");
  }

  getAdjacentNoteId(direction: -1 | 1): string | null {
    if (!this._activeNote || this._notes.length < 2) return null;
    const idx = this._notes.findIndex((n) => n.id === this._activeNote!.id);
    if (idx < 0) return null;
    const next = idx + direction;
    if (next < 0 || next >= this._notes.length) return null;
    return this._notes[next].id;
  }

  on(fn: Listener): () => void {
    this.listeners.push(fn);
    return () => {
      this.listeners = this.listeners.filter((l) => l !== fn);
    };
  }

  private emit(event: EventType) {
    for (const fn of this.listeners) fn(event);
  }
}

export const state = new AppState();
