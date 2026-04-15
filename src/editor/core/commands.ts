import { ResolvedContext } from "./context.ts";
import { replaceRange } from "./operations.ts";
import { executeAvgCommand, executeSumCommand, type SumExpressionEvaluator } from "./sum.ts";
import {
  isWasmReady,
  listCommandSuggestionsFromWasm,
  normalizeCommand,
  resolveCommandFromWasm,
} from "../wasm.ts";
import type {
  CommandMode,
  CommandSuggestion,
  EditOperation,
  EditorContextSnapshot,
} from "./types.ts";

export type { CommandMode, CommandSuggestion } from "./types.ts";

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
}

export interface CommandExecutionResult {
  message: string;
  operations: EditOperation[];
}

interface CommandDefinition {
  execute: (
    normalizedInput: string,
    ctx: ResolvedContext,
    runtime: CommandRuntime,
  ) => Promise<CommandExecutionResult>;
}

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

function normalizeChecklistLine(line: string): string {
  if (line.trim().length === 0) return line;

  const indentMatch = line.match(/^\s*/);
  const indent = indentMatch?.[0] ?? "";
  const body = line.slice(indent.length);

  const checklistMatch = body.match(
    /^((?:->|[-*+]|\d+\.|\d+(?:\.\d+)+)\s+)\[(?: |x|X)\]\s*(.*)$/,
  );
  if (checklistMatch) {
    const marker = checklistMatch[1] ?? "";
    const content = (checklistMatch[2] ?? "").trimStart();
    return content.length > 0 ? `${indent}${marker}[ ] ${content}` : `${indent}${marker}[ ]`;
  }

  const listMatch = body.match(/^((?:->|[-*+]|\d+\.|\d+(?:\.\d+)+)\s+)(.*)$/);
  if (listMatch) {
    const marker = listMatch[1] ?? "";
    const content = (listMatch[2] ?? "").trimStart();
    return content.length > 0 ? `${indent}${marker}[ ] ${content}` : `${indent}${marker}[ ]`;
  }

  const content = body.trimStart();
  return content.length > 0 ? `${indent}- [ ] ${content}` : `${indent}- [ ]`;
}

async function runChecklistCommand(
  _normalizedInput: string,
  ctx: ResolvedContext,
  _runtime: CommandRuntime,
): Promise<CommandExecutionResult> {
  const selection = ctx.selection();
  const startLine = ctx.lineAt(selection.from).number;
  const endCursor = selection.empty
    ? selection.head
    : Math.max(selection.from, selection.to - 1);
  const endLine = ctx.lineAt(endCursor).number;

  const converted: string[] = [];
  let changed = 0;
  for (let lineNo = startLine; lineNo <= endLine; lineNo++) {
    const source = ctx.lineText(lineNo);
    const next = normalizeChecklistLine(source);
    converted.push(next);
    if (next !== source) changed += 1;
  }

  if (changed === 0) {
    return { message: "already checklist", operations: [] };
  }

  const from = ctx.line(startLine).from;
  const to = ctx.line(endLine).to;
  const insert = converted.join("\n");
  const summary = changed === 1 ? "converted 1 line to checklist" : `converted ${changed} lines to checklist`;
  return {
    message: summary,
    operations: [replaceRange(from, to, insert, { anchor: from + insert.length })],
  };
}

const COMMAND_EXECUTORS: Record<string, CommandDefinition["execute"]> = {
  sum: runSumCommand,
  "sum list": runSumCommand,
  "sum row": runSumCommand,
  "sum column": runSumCommand,
  "sum doc": runSumCommand,
  avg: runAvgCommand,
  "avg list": runAvgCommand,
  "avg row": runAvgCommand,
  "avg column": runAvgCommand,
  "avg doc": runAvgCommand,
  date: runDateCommand,
  notify: runNotifyCommand,
  "notify-delete": runNotifyDeleteCommand,
  format: runFormatCommand,
  checklist: runChecklistCommand,
  q: runQuitCommand,
};

interface FallbackCommand {
  value: string;
  aliases?: string[];
  description: string;
  modes: CommandMode[];
}

const FALLBACK_COMMANDS: FallbackCommand[] = [
  { value: "sum", description: "sum paragraph (default scope)", modes: ["vim", "editor"] },
  { value: "sum list", description: "sum list at cursor", modes: ["vim", "editor"] },
  { value: "sum row", aliases: ["sum_row"], description: "sum markdown table per row at cursor", modes: ["vim", "editor"] },
  { value: "sum column", aliases: ["sum_column"], description: "sum markdown table per column at cursor", modes: ["vim", "editor"] },
  { value: "sum doc", aliases: ["sum_all", "sum all"], description: "sum whole document", modes: ["vim", "editor"] },
  { value: "avg", description: "average paragraph (default scope)", modes: ["vim", "editor"] },
  { value: "avg list", description: "average list at cursor", modes: ["vim", "editor"] },
  { value: "avg row", aliases: ["avg_row"], description: "average markdown table per row at cursor", modes: ["vim", "editor"] },
  { value: "avg column", aliases: ["avg_column"], description: "average markdown table per column at cursor", modes: ["vim", "editor"] },
  { value: "avg doc", aliases: ["avg_all", "avg all"], description: "average whole document", modes: ["vim", "editor"] },
  { value: "date", description: "insert picked date", modes: ["vim", "editor"] },
  { value: "notify", aliases: ["alarm", "remind"], description: "set reminder for current line", modes: ["vim", "editor"] },
  { value: "notify-delete", aliases: ["notify_delete", "notify-delte"], description: "delete reminder for current line", modes: ["vim", "editor"] },
  { value: "format", aliases: ["fmt"], description: "format markdown document", modes: ["vim", "editor"] },
  { value: "checklist", aliases: ["checkbox", "checkboxes", "todo"], description: "convert selected lines to checklist", modes: ["vim", "editor"] },
  { value: "q", aliases: ["q!"], description: "quit", modes: ["vim"] },
];

function fallbackAvailableCommands(mode: CommandMode): FallbackCommand[] {
  return FALLBACK_COMMANDS.filter((command) => command.modes.includes(mode));
}

function fallbackResolveCommand(mode: CommandMode, rawInput: string): string | null {
  const normalized = normalizeCommand(rawInput);
  if (!normalized) return null;
  const command = fallbackAvailableCommands(mode).find((entry) =>
    entry.value === normalized || entry.aliases?.includes(normalized),
  );
  return command?.value ?? null;
}

function fallbackListSuggestions(mode: CommandMode, rawInput: string): CommandSuggestion[] {
  const query = normalizeCommand(rawInput);
  const commands = fallbackAvailableCommands(mode);
  if (!query) {
    return commands.map((command) => ({
      value: command.value,
      description: command.description,
    }));
  }

  return commands
    .map((command) => {
      const value = command.value.toLowerCase();
      const starts = value.startsWith(query);
      const includes = value.includes(query);
      const score = starts ? 0 : includes ? 1 : 2;
      return { command, score };
    })
    .filter((entry) => entry.score < 2)
    .sort((a, b) => a.score - b.score || a.command.value.localeCompare(b.command.value))
    .map(({ command }) => ({
      value: command.value,
      description: command.description,
    }));
}

export function listCommandSuggestions(mode: CommandMode, rawInput: string): CommandSuggestion[] {
  const fallback = fallbackListSuggestions(mode, rawInput);
  if (!isWasmReady()) {
    return fallback;
  }
  const wasm = listCommandSuggestionsFromWasm(mode, rawInput);
  if (wasm.length === 0) return fallback;

  const merged: CommandSuggestion[] = [...fallback];
  const seen = new Set(fallback.map((entry) => entry.value));
  for (const entry of wasm) {
    if (seen.has(entry.value)) continue;
    seen.add(entry.value);
    merged.push(entry);
  }
  return merged;
}

export async function executeCommand(
  snapshot: EditorContextSnapshot,
  rawInput: string,
  runtime: CommandRuntime,
): Promise<CommandExecutionResult> {
  const normalizedInput = normalizeCommand(rawInput);
  if (!normalizedInput) return { message: "", operations: [] };

  const resolvedFromWasm = isWasmReady() ? resolveCommandFromWasm(runtime.mode, rawInput) : null;
  const resolved = resolvedFromWasm ?? fallbackResolveCommand(runtime.mode, rawInput);
  if (!resolved) {
    return { message: `unknown command: ${normalizedInput}`, operations: [] };
  }
  const execute = COMMAND_EXECUTORS[resolved];
  if (!execute) {
    return { message: `unknown command: ${normalizedInput}`, operations: [] };
  }

  const ctx = new ResolvedContext(snapshot);
  return execute(resolved, ctx, runtime);
}
