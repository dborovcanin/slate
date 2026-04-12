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
  markdown_autoformat: boolean;
  terminal_mode: boolean;
  vim_mode: boolean;
  date_format: string;
}

export const DEFAULT_THEME_CONFIG: ThemeConfig = {
  color_scheme: "catppuccin-mocha",
  background: "plain",
  font: "jetbrains-mono",
  font_size: 14,
  markdown_autoformat: true,
  terminal_mode: false,
  vim_mode: false,
  date_format: "%Y-%m-%d",
};

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

export async function getThemeConfigOrDefault(): Promise<ThemeConfig> {
  try {
    return await getThemeConfig();
  } catch {
    return { ...DEFAULT_THEME_CONFIG };
  }
}
