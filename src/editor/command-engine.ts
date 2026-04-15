import type { EditorView } from "@codemirror/view";
import { deleteNoteReminder, evaluateLines, upsertNoteReminder } from "../api.ts";
import {
  executeCommand as executeCoreCommand,
  listCommandSuggestions as listCoreCommandSuggestions,
  type CommandMode,
  type CommandSuggestion,
} from "./core/commands.ts";
import { applyEditOperations, snapshotFromView } from "./core/codemirror-adapter.ts";
import { openDatePicker, openDateTimePicker } from "./date-picker.ts";
import { formatMarkdownTextAsync } from "./markdown-format.ts";
import { state } from "../state.ts";
import { applyReminderDelete, applyReminderUpsert } from "./notify-decoration.ts";

export type { CommandMode, CommandSuggestion };

export interface CommandExecutionOptions {
  mode: CommandMode;
  dateFormat?: string;
  dateTimeFormat?: string;
  onExitCommand?: () => Promise<void> | void;
  selectionOverride?: {
    anchor: number;
    head: number;
  };
}

async function copyText(text: string) {
  if (typeof navigator === "undefined" || !navigator.clipboard) return;
  try {
    await navigator.clipboard.writeText(text);
  } catch (err) {
    console.error("Clipboard write failed:", err);
  }
}

export function listCommandSuggestions(mode: CommandMode, rawInput: string): CommandSuggestion[] {
  return listCoreCommandSuggestions(mode, rawInput);
}

export async function executeCommand(
  view: EditorView,
  rawInput: string,
  options: CommandExecutionOptions,
): Promise<string> {
  const snapshot = snapshotFromView(view);
  if (options.selectionOverride) {
    snapshot.selection = {
      anchor: options.selectionOverride.anchor,
      head: options.selectionOverride.head,
    };
  }
  const result = await executeCoreCommand(snapshot, rawInput, {
    mode: options.mode,
    dateFormat: options.dateFormat,
    dateTimeFormat: options.dateTimeFormat,
    pickDate: (dateFormat) => openDatePicker(dateFormat),
    pickDateTime: (pickerOptions) => openDateTimePicker(pickerOptions),
    activeNoteId: state.activeNote?.id ?? null,
    upsertReminder: async (reminder) => {
      const stored = await upsertNoteReminder(
        reminder.noteId,
        reminder.lineNumber,
        reminder.remindAtMs,
        reminder.displayAt,
        reminder.lineText,
      );
      applyReminderUpsert(view, reminder.noteId, stored);
    },
    deleteReminder: async (reminder) => {
      const deleted = await deleteNoteReminder(reminder.noteId, reminder.lineNumber);
      if (deleted) {
        applyReminderDelete(view, reminder.noteId, reminder.lineNumber);
      }
      return deleted;
    },
    evaluateExpression: async (expression) => {
      const [result] = await evaluateLines([expression]);
      return result ?? null;
    },
    copyText,
    onQuit: options.onExitCommand,
    formatMarkdown: formatMarkdownTextAsync,
  });

  if (result.operations.length > 0) {
    applyEditOperations(view, result.operations);
  }

  return result.message;
}
