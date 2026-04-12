import type { Note } from "./api";

export type EventType = "note-changed" | "notes-updated";
type Listener = (event: EventType) => void;

export function deriveTitle(body: string): string {
  const line = body.split("\n").find((l) => l.trim().length > 0);
  if (!line) return "Untitled";
  const trimmed = line.trim();
  return trimmed.length > 60 ? trimmed.slice(0, 60) + "..." : trimmed;
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
