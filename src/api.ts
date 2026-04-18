import { invoke } from "@tauri-apps/api/core";

export interface Note {
  id: string;
  body: string;
  created_at: string;
  updated_at: string;
}

export interface NoteSummary {
  id: string;
  body_prefix: string;
  updated_at: string;
}

export interface NoteReminder {
  note_id: string;
  line_number: number;
  remind_at_ms: number;
  display_at: string;
  line_text: string;
  notified_at_ms?: number | null;
  created_at: string;
  updated_at: string;
}

export interface ThemeConfig {
  color_scheme: string;
  background: string;
  font: string;
  font_size: number;
  animation_mode: string;
  animation_style: string;
  markdown_autoformat: boolean;
  checklist_auto_reorder: boolean;
  format_on_save?: boolean;
  terminal_mode: boolean;
  vim_mode: boolean;
  date_format: string;
  date_time_format: string;
  variables_enabled: boolean;
  variables_autocomplete_min_chars: number;
}

export interface RuntimeFlags {
  plain_text_mode: boolean;
  backend_detach: boolean;
  calc_disable: boolean;
  markdown_disable: boolean;
  folding_disable: boolean;
  notify_disable: boolean;
  autocomplete_disable: boolean;
}

export const DEFAULT_THEME_CONFIG: ThemeConfig = {
  color_scheme: "gruvbox-light",
  background: "plain",
  font: "jetbrains-mono",
  font_size: 14,
  animation_mode: "fast",
  animation_style: "pop-up",
  markdown_autoformat: true,
  checklist_auto_reorder: true,
  format_on_save: false,
  terminal_mode: false,
  vim_mode: false,
  date_format: "%Y-%m-%d",
  date_time_format: "%Y-%m-%d %H:%M",
  variables_enabled: true,
  variables_autocomplete_min_chars: 3,
};

export const DEFAULT_RUNTIME_FLAGS: RuntimeFlags = {
  plain_text_mode: false,
  backend_detach: false,
  calc_disable: false,
  markdown_disable: false,
  folding_disable: false,
  notify_disable: false,
  autocomplete_disable: false,
};

export interface VariableIndexEntry {
  name: string;
  normalized: string;
  line: number; // 1-based
}

export interface NoteEvaluationDiagnostic {
  kind: string;
  line: number; // 1-based
  message: string;
}

export interface TableCellEvaluation {
  cell_index: number;
  value: string;
}

export interface NoteEvaluationResult {
  line_results: (string | null)[];
  variables: VariableIndexEntry[];
  diagnostics?: NoteEvaluationDiagnostic[] | null;
  table_cell_results?: TableCellEvaluation[][];
}

export function getOrCreateNote(): Promise<Note> {
  return invoke<Note>("get_or_create_note");
}

export function saveNote(id: string, body: string): Promise<Note> {
  return invoke<Note>("save_note", { id, body });
}

export function getNote(id: string): Promise<Note | null> {
  return invoke<Note | null>("get_note", { id });
}

export function createNote(): Promise<Note> {
  return invoke<Note>("create_note");
}

export function listNotes(): Promise<Note[]> {
  return invoke<Note[]>("list_notes");
}

export function listNotesMeta(): Promise<NoteSummary[]> {
  return invoke<NoteSummary[]>("list_notes_meta");
}

export function getNoteMeta(id: string): Promise<NoteSummary | null> {
  return invoke<NoteSummary | null>("get_note_meta", { id });
}

export function deleteNote(id: string): Promise<boolean> {
  return invoke<boolean>("delete_note", { id });
}

export function listNoteReminders(noteId: string): Promise<NoteReminder[]> {
  return invoke<NoteReminder[]>("list_note_reminders", { noteId });
}

export function upsertNoteReminder(
  noteId: string,
  lineNumber: number,
  remindAtMs: number,
  displayAt: string,
  lineText: string,
): Promise<NoteReminder> {
  return invoke<NoteReminder>("upsert_note_reminder", {
    noteId,
    lineNumber,
    remindAtMs,
    displayAt,
    lineText,
  });
}

export function deleteNoteReminder(noteId: string, lineNumber: number): Promise<boolean> {
  return invoke<boolean>("delete_note_reminder", { noteId, lineNumber });
}

export function moveNoteReminderLine(
  noteId: string,
  fromLineNumber: number,
  toLineNumber: number,
  lineText: string,
): Promise<boolean> {
  return invoke<boolean>("move_note_reminder_line", {
    noteId,
    fromLineNumber,
    toLineNumber,
    lineText,
  });
}

export function markNoteReminderNotified(
  noteId: string,
  lineNumber: number,
  notifiedAtMs?: number,
): Promise<NoteReminder | null> {
  return invoke<NoteReminder | null>("mark_note_reminder_notified", {
    noteId,
    lineNumber,
    notifiedAtMs,
  });
}

export function sendSystemNotification(title: string, body: string): Promise<void> {
  return invoke<void>("send_system_notification", { title, body });
}

export function evaluateLines(lines: string[]): Promise<(string | null)[]> {
  return invoke<(string | null)[]>("evaluate_lines", { lines });
}

export interface NoteEvaluationRange {
  evalFrom: number;
  evalTo: number;
}

export function evaluateNoteContext(
  lines: string[],
  variablesEnabled = true,
  range?: NoteEvaluationRange,
): Promise<NoteEvaluationResult> {
  return invoke<NoteEvaluationResult>("evaluate_note_context", {
    lines,
    variablesEnabled,
    evalFrom: range?.evalFrom,
    evalTo: range?.evalTo,
  });
}

export function syncNoteLines(
  noteId: string,
  from: number,
  to: number,
  changedLines: string[],
): Promise<void> {
  return invoke<void>("sync_note_lines", { noteId, from, to, changedLines });
}

export function evaluateNoteContextDelta(
  noteId: string,
  variablesEnabled = true,
  range?: NoteEvaluationRange,
): Promise<NoteEvaluationResult> {
  return invoke<NoteEvaluationResult>("evaluate_note_context_delta", {
    noteId,
    variablesEnabled,
    evalFrom: range?.evalFrom,
    evalTo: range?.evalTo,
  });
}

export function exportToFile(path: string, content: string): Promise<void> {
  return invoke<void>("export_to_file", { path, content });
}

export async function readSystemClipboardText(): Promise<string | null> {
  try {
    return await invoke<string | null>("read_clipboard_text");
  } catch {
    return null;
  }
}

export function getThemeConfig(): Promise<ThemeConfig> {
  return invoke<ThemeConfig>("get_theme_config");
}

export function getRuntimeFlags(): Promise<RuntimeFlags> {
  return invoke<RuntimeFlags>("get_runtime_flags");
}

export function appendStartupLog(mode: string, line: string): Promise<void> {
  return invoke<void>("append_startup_log", { mode, line });
}

export async function getThemeConfigOrDefault(): Promise<ThemeConfig> {
  try {
    return await getThemeConfig();
  } catch {
    return { ...DEFAULT_THEME_CONFIG };
  }
}

export async function getRuntimeFlagsOrDefault(): Promise<RuntimeFlags> {
  try {
    return await getRuntimeFlags();
  } catch {
    return { ...DEFAULT_RUNTIME_FLAGS };
  }
}
