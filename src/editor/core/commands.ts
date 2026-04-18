import { ResolvedContext } from "./context.ts";
import { replaceRange } from "./operations.ts";
import { executeAvgCommand, executeSumCommand, type SumExpressionEvaluator } from "./sum.ts";
import {
  convertLineToList,
  isWasmReady,
  listCommandSuggestionsFromWasm,
  normalizeCommand,
  resolveCommandFromWasm,
  type ListKind,
} from "../wasm.ts";
import type {
  CommandMode,
  CommandSuggestion,
  EditOperation,
  EditorContextSnapshot,
} from "./types.ts";

export type { CommandMode, CommandSuggestion } from "./types.ts";

export type FoldCommandAction = "fold" | "unfold" | "fold-toggle";

export interface CommandRuntime {
  mode: CommandMode;
  dateFormat?: string;
  dateTimeFormat?: string;
  pickDate?: (dateFormat: string) => Promise<string | null>;
  pickDateTime?: (options: {
    dateFormat: string;
    dateTimeFormat: string;
    mode: "date" | "notify";
    requireTime: boolean;
  }) => Promise<{
    insertText: string;
    remindAtMs: number;
    displayAt: string;
    hasTime: boolean;
  } | null>;
  activeNoteId?: string | null;
  upsertReminder?: (reminder: {
    noteId: string;
    lineNumber: number;
    remindAtMs: number;
    displayAt: string;
    lineText: string;
  }) => Promise<void> | void;
  deleteReminder?: (reminder: {
    noteId: string;
    lineNumber: number;
  }) => Promise<boolean> | boolean;
  evaluateExpression?: SumExpressionEvaluator;
  copyText?: (text: string) => Promise<void> | void;
  onQuit?: () => Promise<void> | void;
  formatMarkdown?: (input: string) => string | Promise<string>;
  startClipboardWatch?: () => Promise<boolean> | boolean;
  stopClipboardWatch?: () => Promise<boolean> | boolean;
  runFoldCommand?: (action: FoldCommandAction) => {
    changed: boolean;
    message: string;
  };
}

export interface CommandExecutionResult {
  message: string;
  operations: EditOperation[];
}

type ExecuteFn = (
  normalizedInput: string,
  ctx: ResolvedContext,
  runtime: CommandRuntime,
) => Promise<CommandExecutionResult>;

function errorToMessage(error: unknown, fallback: string): string {
  if (error instanceof Error && error.message.trim().length > 0) {
    return error.message;
  }
  if (typeof error === "string" && error.trim().length > 0) {
    return error;
  }
  if (error && typeof error === "object") {
    const maybeMessage = (error as { message?: unknown }).message;
    if (typeof maybeMessage === "string" && maybeMessage.trim().length > 0) {
      return maybeMessage;
    }
  }
  return fallback;
}

async function runDateCommand(
  _normalizedInput: string,
  ctx: ResolvedContext,
  runtime: CommandRuntime,
): Promise<CommandExecutionResult> {
  if (runtime.pickDateTime) {
    const picked = await runtime.pickDateTime({
      dateFormat: runtime.dateFormat ?? "%Y-%m-%d",
      dateTimeFormat:
        runtime.dateTimeFormat ?? `${runtime.dateFormat ?? "%Y-%m-%d"} %H:%M`,
      mode: "date",
      requireTime: false,
    });
    if (!picked) return { message: "date cancelled", operations: [] };
    const value = picked.insertText;
    const selection = ctx.selection();
    const operation = replaceRange(selection.from, selection.to, value, {
      anchor: selection.from + value.length,
    });
    return {
      message: `inserted ${value}`,
      operations: [operation],
    };
  }

  if (!runtime.pickDate) {
    return { message: "date unavailable", operations: [] };
  }

  const value = await runtime.pickDate(runtime.dateFormat ?? "%Y-%m-%d");
  if (!value) return { message: "date cancelled", operations: [] };

  const selection = ctx.selection();
  const operation = replaceRange(selection.from, selection.to, value, {
    anchor: selection.from + value.length,
  });
  return {
    message: `inserted ${value}`,
    operations: [operation],
  };
}

async function runNotifyCommand(
  _normalizedInput: string,
  ctx: ResolvedContext,
  runtime: CommandRuntime,
): Promise<CommandExecutionResult> {
  if (!runtime.pickDateTime || !runtime.upsertReminder || !runtime.activeNoteId) {
    return { message: "notify unavailable", operations: [] };
  }

  const picked = await runtime.pickDateTime({
    dateFormat: runtime.dateFormat ?? "%Y-%m-%d",
    dateTimeFormat:
      runtime.dateTimeFormat ?? `${runtime.dateFormat ?? "%Y-%m-%d"} %H:%M`,
    mode: "notify",
    requireTime: true,
  });
  if (!picked) return { message: "notify cancelled", operations: [] };

  const line = ctx.currentLine();
  try {
    await runtime.upsertReminder({
      noteId: runtime.activeNoteId,
      lineNumber: line.number,
      remindAtMs: picked.remindAtMs,
      displayAt: picked.displayAt,
      lineText: line.text,
    });
  } catch (error) {
    const message = errorToMessage(error, "failed to persist reminder");
    return { message: `notify failed: ${message}`, operations: [] };
  }
  return {
    message: `notify set ${String.fromCodePoint(0x23f0)} ${picked.displayAt}`,
    operations: [],
  };
}

async function runNotifyDeleteCommand(
  _normalizedInput: string,
  ctx: ResolvedContext,
  runtime: CommandRuntime,
): Promise<CommandExecutionResult> {
  if (!runtime.deleteReminder || !runtime.activeNoteId) {
    return { message: "notify-delete unavailable", operations: [] };
  }

  const line = ctx.currentLine();
  try {
    const deleted = await runtime.deleteReminder({
      noteId: runtime.activeNoteId,
      lineNumber: line.number,
    });
    if (!deleted) {
      return { message: `notify-delete: no reminder on line ${line.number}`, operations: [] };
    }
  } catch (error) {
    const message = errorToMessage(error, "failed to delete reminder");
    return { message: `notify-delete failed: ${message}`, operations: [] };
  }
  return {
    message: `notify deleted on line ${line.number}`,
    operations: [],
  };
}

async function runSumCommand(
  normalizedInput: string,
  ctx: ResolvedContext,
  runtime: CommandRuntime,
): Promise<CommandExecutionResult> {
  const result = await executeSumCommand(normalizedInput, ctx, {
    evaluateExpression: runtime.evaluateExpression,
  });
  if (result.clipboardText) {
    await runtime.copyText?.(result.clipboardText);
  }

  return {
    message: result.message,
    operations: result.operation ? [result.operation] : [],
  };
}

async function runAvgCommand(
  normalizedInput: string,
  ctx: ResolvedContext,
  runtime: CommandRuntime,
): Promise<CommandExecutionResult> {
  const result = await executeAvgCommand(normalizedInput, ctx, {
    evaluateExpression: runtime.evaluateExpression,
  });
  if (result.clipboardText) {
    await runtime.copyText?.(result.clipboardText);
  }

  return {
    message: result.message,
    operations: result.operation ? [result.operation] : [],
  };
}

async function runFormatCommand(
  _normalizedInput: string,
  ctx: ResolvedContext,
  runtime: CommandRuntime,
): Promise<CommandExecutionResult> {
  const source = ctx.text();
  const formatter = runtime.formatMarkdown ?? ((value: string) => value);
  const formatted = await formatter(source);
  if (formatted === source) {
    return { message: "already formatted", operations: [] };
  }

  const selection = ctx.selection();
  const anchor = Math.min(selection.anchor, formatted.length);
  const head = Math.min(selection.head, formatted.length);
  return {
    message: "markdown formatted",
    operations: [replaceRange(0, source.length, formatted, { anchor, head })],
  };
}

async function runQuitCommand(
  _normalizedInput: string,
  _ctx: ResolvedContext,
  runtime: CommandRuntime,
): Promise<CommandExecutionResult> {
  if (!runtime.onQuit) return { message: "quit unavailable", operations: [] };
  await runtime.onQuit();
  return { message: "quit", operations: [] };
}

async function runClipWatchCommand(
  _normalizedInput: string,
  _ctx: ResolvedContext,
  runtime: CommandRuntime,
): Promise<CommandExecutionResult> {
  if (!runtime.startClipboardWatch) {
    return { message: "clip-watch unavailable", operations: [] };
  }
  const started = await runtime.startClipboardWatch();
  return {
    message: started ? "clip-watch started" : "clip-watch already active",
    operations: [],
  };
}

async function runClipWatchStopCommand(
  _normalizedInput: string,
  _ctx: ResolvedContext,
  runtime: CommandRuntime,
): Promise<CommandExecutionResult> {
  if (!runtime.stopClipboardWatch) {
    return { message: "clip-watch-stop unavailable", operations: [] };
  }
  const stopped = await runtime.stopClipboardWatch();
  return {
    message: stopped ? "clip-watch stopped" : "clip-watch not active",
    operations: [],
  };
}

async function runFoldCommand(
  normalizedInput: string,
  _ctx: ResolvedContext,
  runtime: CommandRuntime,
): Promise<CommandExecutionResult> {
  if (!runtime.runFoldCommand) {
    return { message: "fold unavailable", operations: [] };
  }
  const result = runtime.runFoldCommand(normalizedInput as FoldCommandAction);
  return {
    message: result.message,
    operations: [],
  };
}

function listConversionLabel(kind: ListKind): string {
  if (kind === "checklist") return "checklist";
  if (kind === "unordered") return "unordered list";
  return "ordered list";
}

async function runListConvertCommand(
  kind: ListKind,
  _normalizedInput: string,
  ctx: ResolvedContext,
  runtime: CommandRuntime,
): Promise<CommandExecutionResult> {
  const selection = ctx.selection();
  let startLine = ctx.lineAt(selection.from).number;
  let endLine = startLine;
  if (!selection.empty) {
    if (runtime.mode === "vim") {
      const anchorLine = ctx.lineAt(selection.anchor).number;
      const headLine = ctx.lineAt(selection.head).number;
      startLine = Math.min(anchorLine, headLine);
      endLine = Math.max(anchorLine, headLine);
    } else {
      const endCursor = Math.max(selection.from, selection.to - 1);
      endLine = ctx.lineAt(endCursor).number;
    }
  }

  const converted: string[] = [];
  let changed = 0;
  let orderedIndex = 1;
  for (let lineNo = startLine; lineNo <= endLine; lineNo++) {
    const source = ctx.lineText(lineNo);
    const { text, changed: lineChanged } = convertLineToList(source, kind, orderedIndex);
    converted.push(text);
    if (lineChanged && kind === "ordered") orderedIndex += 1;
    if (text !== source) changed += 1;
  }

  if (changed === 0) {
    return { message: `already ${listConversionLabel(kind)}`, operations: [] };
  }

  const from = ctx.line(startLine).from;
  const to = ctx.line(endLine).to;
  const insert = converted.join("\n");
  const label = listConversionLabel(kind);
  const summary = changed === 1 ? `converted 1 line to ${label}` : `converted ${changed} lines to ${label}`;
  return {
    message: summary,
    operations: [replaceRange(from, to, insert, { anchor: from + insert.length })],
  };
}

async function runChecklistCommand(
  normalizedInput: string,
  ctx: ResolvedContext,
  runtime: CommandRuntime,
): Promise<CommandExecutionResult> {
  return runListConvertCommand("checklist", normalizedInput, ctx, runtime);
}

async function runUnorderedListCommand(
  normalizedInput: string,
  ctx: ResolvedContext,
  runtime: CommandRuntime,
): Promise<CommandExecutionResult> {
  return runListConvertCommand("unordered", normalizedInput, ctx, runtime);
}

async function runOrderedListCommand(
  normalizedInput: string,
  ctx: ResolvedContext,
  runtime: CommandRuntime,
): Promise<CommandExecutionResult> {
  return runListConvertCommand("ordered", normalizedInput, ctx, runtime);
}

// Executor map: canonical command value (from command_catalog.rs) → execute fn.
// Suggestions, aliases, and resolution come from wasm/command_catalog.rs.
const EXECUTOR_MAP: Record<string, ExecuteFn> = {
  "sum": runSumCommand,
  "sum list": runSumCommand,
  "sum row": runSumCommand,
  "sum column": runSumCommand,
  "sum doc": runSumCommand,
  "avg": runAvgCommand,
  "avg list": runAvgCommand,
  "avg row": runAvgCommand,
  "avg column": runAvgCommand,
  "avg doc": runAvgCommand,
  "date": runDateCommand,
  "notify": runNotifyCommand,
  "notify-delete": runNotifyDeleteCommand,
  "format": runFormatCommand,
  "fold": runFoldCommand,
  "unfold": runFoldCommand,
  "fold-toggle": runFoldCommand,
  "clip-watch": runClipWatchCommand,
  "clip-watch-stop": runClipWatchStopCommand,
  "clist": runChecklistCommand,
  "ulist": runUnorderedListCommand,
  "olist": runOrderedListCommand,
  "q": runQuitCommand,
};

export function listCommandSuggestions(mode: CommandMode, rawInput: string): CommandSuggestion[] {
  if (!isWasmReady()) return [];
  return listCommandSuggestionsFromWasm(mode, rawInput);
}

export async function executeCommand(
  snapshot: EditorContextSnapshot,
  rawInput: string,
  runtime: CommandRuntime,
): Promise<CommandExecutionResult> {
  const normalizedInput = normalizeCommand(rawInput);
  if (!normalizedInput) return { message: "", operations: [] };

  const canonical = isWasmReady() ? resolveCommandFromWasm(runtime.mode, rawInput) : null;
  if (!canonical) {
    return { message: `unknown command: ${normalizedInput}`, operations: [] };
  }

  const executor = EXECUTOR_MAP[canonical];
  if (!executor) {
    return { message: `unknown command: ${normalizedInput}`, operations: [] };
  }

  const ctx = new ResolvedContext(snapshot);
  return executor(canonical, ctx, runtime);
}
