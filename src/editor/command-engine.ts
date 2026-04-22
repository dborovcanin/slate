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

export type { CommandMode, CommandSuggestion };

export interface CommandExecutionOptions {
  mode: CommandMode;
  dateFormat?: string;
  dateTimeFormat?: string;
  onExitCommand?: () => Promise<void> | void;
  onClipWatchStateChange?: (active: boolean) => void;
  onClipWatchPaste?: (text: string) => void;
  getNoteModules?: () => NoteModules | null;
  setNoteModules?: (modules: NoteModules) => Promise<void> | void;
  selectionOverride?: {
    anchor: number;
    head: number;
  };
}

function parseNoteSecurityCommand(rawInput: string): {
  action: "lock" | "unlock" | "encrypt" | "decrypt" | "unprotect";
  password: string | null;
} | null {
  const normalized = rawInput.replace(/^:/, "").trimStart();
  if (!normalized) return null;
  const [head, ...tailParts] = normalized.split(/\s+/);
  const tail = normalized.slice((head ?? "").length).trimStart();
  const normalizeAction = (
    token: string,
  ): "lock" | "unlock" | "encrypt" | "decrypt" | "unprotect" | null => {
    const lowered = token.toLowerCase();
    if (lowered === "lock" || lowered === "note-lock" || lowered === "lock-note") return "lock";
    if (lowered === "unlock" || lowered === "note-unlock" || lowered === "unlock-note")
      return "unlock";
    if (lowered === "encrypt" || lowered === "note-encrypt" || lowered === "encrypt-note")
      return "encrypt";
    if (lowered === "decrypt" || lowered === "note-decrypt" || lowered === "decrypt-note")
      return "decrypt";
    if (
      lowered === "unprotect" ||
      lowered === "unencrypt" ||
      lowered === "note-unprotect" ||
      lowered === "unprotect-note" ||
      lowered === "note-unencrypt" ||
      lowered === "unencrypt-note"
    ) {
      return "unprotect";
    }
    return null;
  };

  if ((head ?? "").toLowerCase() === "note") {
    const actionToken = tailParts[0] ?? "";
    const action = normalizeAction(actionToken);
    if (!action) return null;
    const passwordRaw = tail
      .slice(actionToken.length)
      .trimStart();
    return { action, password: passwordRaw.trim().length > 0 ? passwordRaw : null };
  }

  const action = normalizeAction(head ?? "");
  if (!action) return null;
  return { action, password: tail.trim().length > 0 ? tail : null };
}

async function tryExecuteNoteSecurityCommand(
  view: EditorView,
  rawInput: string,
): Promise<string | null> {
  const parsed = parseNoteSecurityCommand(rawInput);
  if (!parsed) return null;
  const active = state.activeNote;
  if (!active) return "no active note";
  if (!parsed.password) {
    return `usage: note ${parsed.action} <password>`;
  }

  const next = await (async () => {
    if (parsed.action === "lock") return lockNoteAccess(active.id, parsed.password!);
    if (parsed.action === "unlock") return unlockNoteAccess(active.id, parsed.password!);
    if (parsed.action === "encrypt") return encryptNote(active.id, parsed.password!);
    if (parsed.action === "unprotect") return decryptNote(active.id, parsed.password!);
    return decryptNote(active.id, parsed.password!);
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
  return [...extra, ...core];
}

export async function executeCommand(
  view: EditorView,
  rawInput: string,
  options: CommandExecutionOptions,
): Promise<string> {
  const profilerMessage = tryExecuteEditorProfilerCommand(rawInput);
  if (profilerMessage !== null) return profilerMessage;

  const noteSecurityMessage = await tryExecuteNoteSecurityCommand(view, rawInput);
  if (noteSecurityMessage !== null) return noteSecurityMessage;

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
