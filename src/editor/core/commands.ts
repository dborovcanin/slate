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

interface CommandDefinition {
  execute: (
    normalizedInput: string,
    ctx: ResolvedContext,
    runtime: CommandRuntime,
  ) => Promise<CommandExecutionResult>;
}

interface CommandRegistryEntry extends CommandDefinition {
  value: string;
  aliases?: string[];
  description: string;
  modes: CommandMode[];
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

type ListConversionKind = "checklist" | "unordered" | "ordered";

function convertLineToList(
  line: string,
  kind: ListConversionKind,
  orderedIndex: number,
): { text: string; converted: boolean } {
  if (line.trim().length === 0) return { text: line, converted: false };

  const indentMatch = line.match(/^\s*/);
  const indent = indentMatch?.[0] ?? "";
  const body = line.slice(indent.length);
  const listMatch = body.match(
    /^((?:->|[-*+]|\d+\.|\d+(?:\.\d+)+))\s+(?:\[(?: |x|X)\]\s*)?(.*)$/,
  );

  const marker = listMatch?.[1] ?? "";
  const content = (listMatch?.[2] ?? body).trimStart();

  if (kind === "checklist") {
    const prefix = marker.length > 0 ? `${marker} [ ]` : "- [ ]";
    return {
      text: content.length > 0 ? `${indent}${prefix} ${content}` : `${indent}${prefix}`,
      converted: true,
    };
  }

  if (kind === "unordered") {
    return {
      text: content.length > 0 ? `${indent}- ${content}` : `${indent}-`,
      converted: true,
    };
  }

  const orderedMarker = `${orderedIndex}.`;
  return {
    text: content.length > 0 ? `${indent}${orderedMarker} ${content}` : `${indent}${orderedMarker}`,
    converted: true,
  };
}

function listConversionLabel(kind: ListConversionKind): string {
  if (kind === "checklist") return "checklist";
  if (kind === "unordered") return "unordered list";
  return "ordered list";
}

async function runListConvertCommand(
  kind: ListConversionKind,
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
    const lineConversion = convertLineToList(source, kind, orderedIndex);
    converted.push(lineConversion.text);
    if (lineConversion.converted && kind === "ordered") {
      orderedIndex += 1;
    }
    if (lineConversion.text !== source) changed += 1;
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

const MODES_BOTH: CommandMode[] = ["vim", "editor"];

const COMMAND_REGISTRY: CommandRegistryEntry[] = [
  { value: "sum", description: "sum paragraph (default scope)", modes: MODES_BOTH, execute: runSumCommand },
  { value: "sum list", description: "sum list at cursor", modes: MODES_BOTH, execute: runSumCommand },
  { value: "sum row", aliases: ["sum_row"], description: "sum markdown table per row at cursor", modes: MODES_BOTH, execute: runSumCommand },
  { value: "sum column", aliases: ["sum_column"], description: "sum markdown table per column at cursor", modes: MODES_BOTH, execute: runSumCommand },
  { value: "sum doc", aliases: ["sum_all", "sum all"], description: "sum whole document", modes: MODES_BOTH, execute: runSumCommand },
  { value: "avg", description: "average paragraph (default scope)", modes: MODES_BOTH, execute: runAvgCommand },
  { value: "avg list", description: "average list at cursor", modes: MODES_BOTH, execute: runAvgCommand },
  { value: "avg row", aliases: ["avg_row"], description: "average markdown table per row at cursor", modes: MODES_BOTH, execute: runAvgCommand },
  { value: "avg column", aliases: ["avg_column"], description: "average markdown table per column at cursor", modes: MODES_BOTH, execute: runAvgCommand },
  { value: "avg doc", aliases: ["avg_all", "avg all"], description: "average whole document", modes: MODES_BOTH, execute: runAvgCommand },
  { value: "date", description: "insert picked date", modes: MODES_BOTH, execute: runDateCommand },
  { value: "notify", aliases: ["alarm", "remind"], description: "set reminder for current line", modes: MODES_BOTH, execute: runNotifyCommand },
  { value: "notify-delete", aliases: ["notify_delete", "notify-delte"], description: "delete reminder for current line", modes: MODES_BOTH, execute: runNotifyDeleteCommand },
  { value: "format", aliases: ["fmt"], description: "format markdown document", modes: MODES_BOTH, execute: runFormatCommand },
  { value: "fold", aliases: ["zc"], description: "fold at cursor", modes: MODES_BOTH, execute: runFoldCommand },
  { value: "unfold", aliases: ["zo"], description: "unfold at cursor", modes: MODES_BOTH, execute: runFoldCommand },
  { value: "fold-toggle", aliases: ["za"], description: "toggle fold at cursor", modes: MODES_BOTH, execute: runFoldCommand },
  { value: "clip-watch", aliases: ["clip_watch"], description: "watch clipboard and paste text at cursor", modes: MODES_BOTH, execute: runClipWatchCommand },
  { value: "clip-watch-stop", aliases: ["clip_watch_stop"], description: "stop clipboard watch", modes: MODES_BOTH, execute: runClipWatchStopCommand },
  { value: "clist", aliases: ["checklist", "checkbox", "checkboxes", "todo"], description: "convert selected lines to checklist", modes: MODES_BOTH, execute: runChecklistCommand },
  { value: "ulist", aliases: ["unordered-list", "unordered"], description: "convert selected lines to unordered list", modes: MODES_BOTH, execute: runUnorderedListCommand },
  { value: "olist", aliases: ["ordered-list", "ordered"], description: "convert selected lines to ordered list", modes: MODES_BOTH, execute: runOrderedListCommand },
  { value: "q", aliases: ["q!"], description: "quit", modes: ["vim"], execute: runQuitCommand },
];

function registryAvailableCommands(mode: CommandMode): CommandRegistryEntry[] {
  return COMMAND_REGISTRY.filter((command) => command.modes.includes(mode));
}

function registryResolveCommand(mode: CommandMode, rawInput: string): CommandRegistryEntry | null {
  const normalized = normalizeCommand(rawInput);
  if (!normalized) return null;
  return registryAvailableCommands(mode).find((entry) =>
    entry.value === normalized || entry.aliases?.includes(normalized),
  ) ?? null;
}

function registryListSuggestions(mode: CommandMode, rawInput: string): CommandSuggestion[] {
  const query = normalizeCommand(rawInput);
  const commands = registryAvailableCommands(mode);
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
  const fallback = registryListSuggestions(mode, rawInput);
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
  const command = resolvedFromWasm
    ? registryAvailableCommands(runtime.mode).find((entry) => entry.value === resolvedFromWasm) ?? null
    : registryResolveCommand(runtime.mode, rawInput);
  if (!command) {
    return { message: `unknown command: ${normalizedInput}`, operations: [] };
  }

  const ctx = new ResolvedContext(snapshot);
  return command.execute(command.value, ctx, runtime);
}
