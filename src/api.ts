import { invoke } from "@tauri-apps/api/core";

export interface Note {
  id: string;
  body: string;
  created_at: string;
  updated_at: string;
}

export function getOrCreateNote(): Promise<Note> {
  return invoke<Note>("get_or_create_note");
}

export function saveNote(id: string, body: string): Promise<Note> {
  return invoke<Note>("save_note", { id, body });
}

export function createNote(): Promise<Note> {
  return invoke<Note>("create_note");
}

export function listNotes(): Promise<Note[]> {
  return invoke<Note[]>("list_notes");
}

export function deleteNote(id: string): Promise<boolean> {
  return invoke<boolean>("delete_note", { id });
}

export function evaluateLines(lines: string[]): Promise<(string | null)[]> {
  return invoke<(string | null)[]>("evaluate_lines", { lines });
}
