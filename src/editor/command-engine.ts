import type { EditorView } from "@codemirror/view";
import {
  decryptNote,
  encryptNote,
  deleteNoteReminder,
  evaluateLines,
  lockNoteAccess,
  readSystemClipboardText,
  unlockNoteAccess,
  upsertNoteReminder,
} from "../api.ts";
import {
  executeCommand as executeCoreCommand,
  type FoldCommandAction,
  listCommandSuggestions as listCoreCommandSuggestions,
  type CommandMode,
  type CommandSuggestion,
} from "./core/commands.ts";
import { applyEditOperations, snapshotFromView } from "./core/codemirror-adapter.ts";
import { openDatePicker, openDateTimePicker } from "./date-picker.ts";
import { formatMarkdownTextAsync } from "./markdown-format.ts";
import { state } from "../state.ts";
import { applyReminderDelete, applyReminderUpsert } from "./notify-decoration.ts";
import { executeFoldCommand } from "./folding.ts";
import {
  listEditorProfilerCommandSuggestions,
  tryExecuteEditorProfilerCommand,
} from "../perf/editor-profiler.ts";
import type { NoteModules } from "../api.ts";
import {
  ensureWasmReady,
  planHostCommandFromWasm,
} from "./wasm.ts";

export type { CommandMode, CommandSuggestion };
export type ExportCommandFormat = "pdf" | "md" | "txt";

export interface CommandExecutionOptions {
  mode: CommandMode;
  dateFormat?: string;
  dateTimeFormat?: string;
  onWriteCommand?: (options?: { force?: boolean }) => Promise<void> | void;
  onExitCommand?: () => Promise<void> | void;
  onClipWatchStateChange?: (active: boolean) => void;
  onClipWatchPaste?: (text: string) => void;
  getNoteModules?: () => NoteModules | null;
  setNoteModules?: (modules: NoteModules) => Promise<void> | void;
  onExportCommand?: (options: {
    format: ExportCommandFormat;
    path: string | null;
  }) => Promise<string | void> | string | void;
  selectionOverride?: {
    anchor: number;
    head: number;
  };
}

async function tryExecuteNoteSecurityCommand(
  view: EditorView,
  parsed: {
    action: "lock" | "unlock" | "encrypt" | "decrypt" | "unprotect";
    password: string;
  },
): Promise<string | null> {
  const active = state.activeNote;
  if (!active) return "no active note";
  const password = parsed.password.trim().length > 0 ? parsed.password : null;
  if (!password) {
    return `usage: note ${parsed.action} <password>`;
  }

  const next = await (async () => {
    if (parsed.action === "lock") return lockNoteAccess(active.id, password);
    if (parsed.action === "unlock") return unlockNoteAccess(active.id, password);
    if (parsed.action === "encrypt") return encryptNote(active.id, password);
    if (parsed.action === "unprotect") return decryptNote(active.id, password);
    return decryptNote(active.id, password);
  })();

  state.setActiveNote(next);
  const { setEditorContent } = await import("./editor.ts");
  const prevHead = view.state.selection.main.head;
  setEditorContent(next.body);
  view.dispatch({
    selection: { anchor: Math.min(next.body.length, prevHead) },
    scrollIntoView: true,
  });

  if (parsed.action === "lock") return "note locked";
  if (parsed.action === "unlock") return "note unlocked";
  if (parsed.action === "encrypt") return "note encrypted at rest";
  if (parsed.action === "unprotect") return "note unprotected";
  return "note decrypted";
}

const CLIPBOARD_WATCH_POLL_MS = 400;

let clipboardWatchTimer: number | null = null;
let clipboardWatchInFlight = false;
let clipboardWatchLastText: string | null = null;
let clipboardWatchTarget: EditorView | null = null;
let clipboardWatchStateChangeCb: ((active: boolean) => void) | null = null;
let clipboardWatchPasteCb: ((text: string) => void) | null = null;

function clipboardWatchSupported(): boolean {
  return typeof window !== "undefined";
}

function isClipboardWatchActive(): boolean {
  return clipboardWatchTimer !== null;
}

async function readClipboardText(): Promise<string | null> {
  if (!clipboardWatchSupported()) return null;
  const backendText = await readSystemClipboardText();
  if (backendText && backendText.length > 0) {
    return backendText;
  }
  if (typeof navigator === "undefined" || !navigator.clipboard?.readText) {
    return null;
  }
  try {
    const value = await navigator.clipboard.readText();
    if (!value || value.length === 0) return null;
    return value;
  } catch {
    return null;
  }
}

function insertClipboardText(view: EditorView, text: string) {
  if (!text) return;
  const main = view.state.selection.main;
  view.dispatch({
    changes: { from: main.from, to: main.to, insert: text },
    selection: { anchor: main.from + text.length },
    scrollIntoView: true,
  });
}

async function pollClipboardWatch() {
  if (clipboardWatchInFlight) return;
  if (!clipboardWatchTarget) return;
  clipboardWatchInFlight = true;
  try {
    const text = await readClipboardText();
    if (!text || text === clipboardWatchLastText) return;
    clipboardWatchLastText = text;
    insertClipboardText(clipboardWatchTarget, text);
    clipboardWatchPasteCb?.(text);
  } finally {
    clipboardWatchInFlight = false;
  }
}

async function startClipboardWatch(
  view: EditorView,
  callbacks: {
    onStateChange?: (active: boolean) => void;
    onPaste?: (text: string) => void;
  } = {},
): Promise<boolean> {
  if (!clipboardWatchSupported()) return false;
  clipboardWatchStateChangeCb = callbacks.onStateChange ?? null;
  clipboardWatchPasteCb = callbacks.onPaste ?? null;
  clipboardWatchTarget = view;
  if (clipboardWatchTimer !== null) return false;

  clipboardWatchLastText = await readClipboardText();
  clipboardWatchTimer = window.setInterval(() => {
    void pollClipboardWatch();
  }, CLIPBOARD_WATCH_POLL_MS);
  clipboardWatchStateChangeCb?.(true);
  return true;
}

function stopClipboardWatch(): boolean {
  if (clipboardWatchTimer === null) {
    clipboardWatchStateChangeCb?.(false);
    return false;
  }
  window.clearInterval(clipboardWatchTimer);
  clipboardWatchTimer = null;
  clipboardWatchTarget = null;
  clipboardWatchInFlight = false;
  clipboardWatchLastText = null;
  clipboardWatchStateChangeCb?.(false);
  clipboardWatchStateChangeCb = null;
  clipboardWatchPasteCb = null;
  return true;
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
  const core = listCoreCommandSuggestions(mode, rawInput);
  const profiler = listEditorProfilerCommandSuggestions(rawInput);
  if (profiler.length === 0) return core;
  const seen = new Set(core.map((entry) => entry.value));
  const extra = profiler.filter((entry) => !seen.has(entry.value));
  return [...core, ...extra];
}

export async function executeCommand(
  view: EditorView,
  rawInput: string,
  options: CommandExecutionOptions,
): Promise<string> {
  // Command semantics are wasm-owned; avoid temporary fallback parsing paths.
  await ensureWasmReady();

  const profilerMessage = tryExecuteEditorProfilerCommand(rawInput);
  if (profilerMessage !== null) return profilerMessage;

  const hostPlan = planHostCommandFromWasm(options.mode, rawInput);
  if (hostPlan?.kind === "write") {
    if (!options.onWriteCommand) return "write unavailable";
    try {
      await options.onWriteCommand({ force: hostPlan.force });
    } catch (error) {
      const message =
        error instanceof Error
          ? error.message
          : typeof error === "string"
            ? error
            : String(error);
      return `write failed: ${message}`;
    }
    if (hostPlan.quit) {
      if (!options.onExitCommand) return "quit unavailable";
      await options.onExitCommand();
      return "written and quit";
    }
    return "written";
  }
  if (hostPlan?.kind === "note_security") {
    const noteSecurityMessage = await tryExecuteNoteSecurityCommand(view, hostPlan);
    if (noteSecurityMessage !== null) {
      return noteSecurityMessage;
    }
  }
  if (hostPlan?.kind === "export") {
    const format = hostPlan.format;
    const path = hostPlan.path && hostPlan.path.trim().length > 0
      ? hostPlan.path.trim()
      : null;
    if (format === "pdf" && !path) {
      return "usage: export pdf <path>";
    }
    if (!options.onExportCommand) {
      return "export unavailable";
    }
    try {
      const message = await options.onExportCommand({ format, path });
      if (typeof message === "string" && message.trim().length > 0) {
        return message;
      }
      if (path) return `exported ${format} to ${path}`;
      return `exported ${format} to clipboard`;
    } catch (error) {
      const message =
        error instanceof Error
          ? error.message
          : typeof error === "string"
            ? error
            : String(error);
      return `export failed: ${message}`;
    }
  }

  const snapshot = snapshotFromView(view);
  if (options.selectionOverride) {
    snapshot.selection = {
      anchor: options.selectionOverride.anchor,
      head: options.selectionOverride.head,
    };
  }
  const canClipboardWatch = clipboardWatchSupported();
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
    startClipboardWatch: canClipboardWatch
      ? () => startClipboardWatch(view, {
        onStateChange: options.onClipWatchStateChange,
        onPaste: options.onClipWatchPaste,
      })
      : undefined,
    stopClipboardWatch: canClipboardWatch || isClipboardWatchActive()
      ? () => stopClipboardWatch()
      : undefined,
    runFoldCommand: (action: FoldCommandAction) => executeFoldCommand(view, action),
    onWrite: options.onWriteCommand,
    onQuit: options.onExitCommand,
    formatMarkdown: formatMarkdownTextAsync,
    getNoteModules: options.getNoteModules,
    setNoteModules: options.setNoteModules,
  });

  if (result.operations.length > 0) {
    applyEditOperations(view, result.operations);
  }

  return result.message;
}
