import { invoke } from "@tauri-apps/api/core";

export interface Note {
  id: string;
  body: string;
  created_at: string;
  updated_at: string;
}

export interface ThemeConfig {
  color_scheme: string;
  background: string;
  font: string;
  font_size: number;
  vim_mode: boolean;
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

export function exportToFile(path: string, content: string): Promise<void> {
  return invoke<void>("export_to_file", { path, content });
}

export function getThemeConfig(): Promise<ThemeConfig> {
  return invoke<ThemeConfig>("get_theme_config");
}
