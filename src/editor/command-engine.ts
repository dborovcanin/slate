import type { EditorView } from "@codemirror/view";
import { evaluateLines } from "../api.ts";
import {
  executeCommand as executeCoreCommand,
  listCommandSuggestions as listCoreCommandSuggestions,
  type CommandMode,
  type CommandSuggestion,
} from "./core/commands.ts";
import { applyEditOperations, snapshotFromView } from "./core/codemirror-adapter.ts";
import { openDatePicker } from "./date-picker.ts";
import { formatMarkdownText } from "./markdown-format.ts";

export type { CommandMode, CommandSuggestion };

export interface CommandExecutionOptions {
  mode: CommandMode;
  dateFormat?: string;
  onExitCommand?: () => Promise<void> | void;
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
  const result = await executeCoreCommand(snapshot, rawInput, {
    mode: options.mode,
    dateFormat: options.dateFormat,
    pickDate: (dateFormat) => openDatePicker(dateFormat),
    evaluateExpression: async (expression) => {
      const [result] = await evaluateLines([expression]);
      return result ?? null;
    },
    copyText,
    onQuit: options.onExitCommand,
    formatMarkdown: formatMarkdownText,
  });

  if (result.operations.length > 0) {
    applyEditOperations(view, result.operations);
  }

  return result.message;
}
