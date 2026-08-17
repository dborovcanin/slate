import { ResolvedContext } from "./context.ts";
import { replaceRange } from "./operations.ts";
import type { TextChange } from "./types.ts";
import {
  convertLineToList,
  convertLineToTitle,
  isWasmReady,
  listCommandSuggestionsFromWasm,
  normalizeCommand,
  planModuleCommandFromWasm,
  resolveCommandFromWasm,
  tryExecuteVimSubstituteFromWasm,
  type ListKind,
} from "../wasm.ts";
import type {
  CommandMode,
  CommandSuggestion,
  EditOperation,
  EditorContextSnapshot,
} from "./types.ts";
import type { WasmCommandExecutionResult } from "../wasm.ts";
import type { NoteModules } from "../../api.ts";

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
    mode: "date" | "remind";
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
  /// Host-executed `:sum` / `:avg`. Injected rather than called directly so
  /// this module stays free of transport concerns.
  executeMathCommand?: (
    snapshot: EditorContextSnapshot,
    rawInput: string,
    mode: CommandMode,
  ) => Promise<WasmCommandExecutionResult | null>;
  copyText?: (text: string) => Promise<void> | void;
  onWrite?: (options?: { force?: boolean }) => Promise<void> | void;
  onQuit?: () => Promise<void> | void;
  formatMarkdown?: (input: string) => string | Promise<string>;
  startClipboardWatch?: () => Promise<boolean> | boolean;
  stopClipboardWatch?: () => Promise<boolean> | boolean;
  runFoldCommand?: (action: FoldCommandAction) => {
    changed: boolean;
    message: string;
  };
  getNoteModules?: () => NoteModules | null;
  setNoteModules?: (modules: NoteModules) => Promise<void> | void;
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

async function runRemindCommand(
  _normalizedInput: string,
  ctx: ResolvedContext,
  runtime: CommandRuntime,
): Promise<CommandExecutionResult> {
  if (!runtime.pickDateTime || !runtime.upsertReminder || !runtime.activeNoteId) {
    return { message: "remind unavailable", operations: [] };
  }

  const picked = await runtime.pickDateTime({
    dateFormat: runtime.dateFormat ?? "%Y-%m-%d",
    dateTimeFormat:
      runtime.dateTimeFormat ?? `${runtime.dateFormat ?? "%Y-%m-%d"} %H:%M`,
    mode: "remind",
    requireTime: true,
  });
  if (!picked) return { message: "remind cancelled", operations: [] };

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
    return { message: `remind failed: ${message}`, operations: [] };
  }
  return {
    message: `remind set ${String.fromCodePoint(0x23f0)} ${picked.displayAt}`,
    operations: [],
  };
}

async function runRemindToggleCommand(
  normalizedInput: string,
  ctx: ResolvedContext,
  runtime: CommandRuntime,
): Promise<CommandExecutionResult> {
  if (!runtime.activeNoteId) {
    return { message: "remind toggle unavailable", operations: [] };
  }
  if (!runtime.deleteReminder) {
    return runRemindCommand(normalizedInput, ctx, runtime);
  }

  const line = ctx.currentLine();
  try {
    const deleted = await runtime.deleteReminder({
      noteId: runtime.activeNoteId,
      lineNumber: line.number,
    });
    if (deleted) {
      return {
        message: `remind removed on line ${line.number}`,
        operations: [],
      };
    }
    return runRemindCommand(normalizedInput, ctx, runtime);
  } catch (error) {
    const message = errorToMessage(error, "failed to delete reminder");
    return { message: `remind toggle failed: ${message}`, operations: [] };
  }
}

async function runSumCommand(
  normalizedInput: string,
  ctx: ResolvedContext,
  runtime: CommandRuntime,
): Promise<CommandExecutionResult> {
  if (!runtime.executeMathCommand) {
    return { message: "sum unavailable", operations: [] };
  }
  const selection = ctx.selection();
  const result = await runtime.executeMathCommand({
    text: ctx.text(),
    selection: {
      anchor: selection.anchor,
      head: selection.head,
    },
  }, normalizedInput, runtime.mode);
  if (!result) {
    return { message: "sum unavailable", operations: [] };
  }
  if (result.clipboardText) {
    await runtime.copyText?.(result.clipboardText);
  }

  return {
    message: result.message,
    operations: result.operations,
  };
}

async function runAvgCommand(
  normalizedInput: string,
  ctx: ResolvedContext,
  runtime: CommandRuntime,
): Promise<CommandExecutionResult> {
  if (!runtime.executeMathCommand) {
    return { message: "avg unavailable", operations: [] };
  }
  const selection = ctx.selection();
  const result = await runtime.executeMathCommand({
    text: ctx.text(),
    selection: {
      anchor: selection.anchor,
      head: selection.head,
    },
  }, normalizedInput, runtime.mode);
  if (!result) {
    return { message: "avg unavailable", operations: [] };
  }
  if (result.clipboardText) {
    await runtime.copyText?.(result.clipboardText);
  }

  return {
    message: result.message,
    operations: result.operations,
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

async function runWriteCommand(
  _normalizedInput: string,
  _ctx: ResolvedContext,
  runtime: CommandRuntime,
): Promise<CommandExecutionResult> {
  if (!runtime.onWrite) return { message: "write unavailable", operations: [] };
  try {
    await runtime.onWrite();
  } catch (error) {
    return {
      message: `write failed: ${errorToMessage(error, "failed to save note")}`,
      operations: [],
    };
  }
  return { message: "written", operations: [] };
}

async function runWriteQuitCommand(
  normalizedInput: string,
  ctx: ResolvedContext,
  runtime: CommandRuntime,
): Promise<CommandExecutionResult> {
  const writeResult = await runWriteCommand(normalizedInput, ctx, runtime);
  if (writeResult.message !== "written") {
    return writeResult;
  }
  if (!runtime.onQuit) return { message: "quit unavailable", operations: [] };
  await runtime.onQuit();
  return { message: "written and quit", operations: [] };
}

async function runClipWatchCommand(
  normalizedInput: string,
  _ctx: ResolvedContext,
  runtime: CommandRuntime,
): Promise<CommandExecutionResult> {
  if (!runtime.startClipboardWatch) {
    return { message: `${normalizedInput} unavailable`, operations: [] };
  }
  const started = await runtime.startClipboardWatch();
  return {
    message: started ? "clip-watch started" : "clip-watch already active",
    operations: [],
  };
}

async function runClipWatchStopCommand(
  normalizedInput: string,
  _ctx: ResolvedContext,
  runtime: CommandRuntime,
): Promise<CommandExecutionResult> {
  if (!runtime.stopClipboardWatch) {
    return { message: `${normalizedInput} unavailable`, operations: [] };
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

async function runModuleCommand(
  normalizedInput: string,
  _ctx: ResolvedContext,
  runtime: CommandRuntime,
): Promise<CommandExecutionResult> {
  const current = runtime.getNoteModules?.() ?? null;
  if (!current) {
    return { message: "module unavailable", operations: [] };
  }
  const plan = planModuleCommandFromWasm(runtime.mode, normalizedInput, current);
  if (!plan) {
    return { message: "module unavailable", operations: [] };
  }
  if (!plan.changed) {
    return { message: plan.message, operations: [] };
  }

  if (!runtime.setNoteModules) {
    return { message: "module unavailable", operations: [] };
  }
  try {
    await runtime.setNoteModules(plan.next as NoteModules);
  } catch (error) {
    return {
      message: `module update failed: ${errorToMessage(error, "unknown module error")}`,
      operations: [],
    };
  }
  return { message: plan.message, operations: [] };
}

type ParagraphConversionKind = "title" | ListKind;

function listConversionLabel(kind: ParagraphConversionKind): string {
  if (kind === "title") return "title";
  if (kind === "checklist") return "checklist";
  if (kind === "unordered") return "unordered list";
  return "ordered list";
}

async function runListConvertCommand(
  kind: ParagraphConversionKind,
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
    const text = kind === "title"
      ? convertLineToTitle(source).text
      : convertLineToList(source, kind, orderedIndex).text;
    converted.push(text);
    if (kind === "ordered" && source.trim().length > 0) orderedIndex += 1;
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

async function runTitleCommand(
  normalizedInput: string,
  ctx: ResolvedContext,
  runtime: CommandRuntime,
): Promise<CommandExecutionResult> {
  return runListConvertCommand("title", normalizedInput, ctx, runtime);
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

function makeInlineWrapExecutor(
  left: string,
  right: string,
  label: string,
): ExecuteFn {
  return async (_normalizedInput, ctx) => {
    const text = ctx.text();
    const sel = ctx.selection();
    const { from, to } = sel;

    if (sel.empty) {
      return {
        message: `${label} markers inserted`,
        operations: [{ changes: [{ from, to, insert: left + right }], selection: { anchor: from + left.length } }],
      };
    }

    const isWrapped =
      from >= left.length &&
      text.slice(from - left.length, from) === left &&
      to + right.length <= text.length &&
      text.slice(to, to + right.length) === right;

    if (isWrapped) {
      return {
        message: `${label} removed`,
        operations: [{
          changes: [
            { from: from - left.length, to: from, insert: "" },
            { from: to, to: to + right.length, insert: "" },
          ] as TextChange[],
          selection: { anchor: from - left.length, head: to - left.length },
        }],
      };
    }

    return {
      message: `${label} applied`,
      operations: [{
        changes: [
          { from, to: from, insert: left },
          { from: to, to, insert: right },
        ] as TextChange[],
        selection: { anchor: from + left.length, head: to + left.length },
      }],
    };
  };
}

const INLINE_FORMAT_MARKERS = ["**", "~~", "*", "`"];

async function runFormatClearCommand(
  _normalizedInput: string,
  ctx: ResolvedContext,
): Promise<CommandExecutionResult> {
  const sel = ctx.selection();
  const { from, to } = sel;
  if (sel.empty) return { message: "no selection", operations: [] };
  const selected = ctx.text().slice(from, to);
  let stripped = selected;
  for (const marker of INLINE_FORMAT_MARKERS) {
    stripped = stripped.split(marker).join("");
  }
  if (stripped === selected) return { message: "no inline formatting found", operations: [] };
  return {
    message: "inline formatting cleared",
    operations: [{ changes: [{ from, to, insert: stripped }], selection: { anchor: from, head: from + stripped.length } }],
  };
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
  "remind": runRemindCommand,
  "remind toggle": runRemindToggleCommand,
  "module status": runModuleCommand,
  "module math on": runModuleCommand,
  "module math off": runModuleCommand,
  "module math toggle": runModuleCommand,
  "module table on": runModuleCommand,
  "module table off": runModuleCommand,
  "module table toggle": runModuleCommand,
  "module variables on": runModuleCommand,
  "module variables off": runModuleCommand,
  "module variables toggle": runModuleCommand,
  "module style on": runModuleCommand,
  "module style off": runModuleCommand,
  "module style toggle": runModuleCommand,
  "format": runFormatCommand,
  "fold": runFoldCommand,
  "unfold": runFoldCommand,
  "fold-toggle": runFoldCommand,
  "clip-watch on": runClipWatchCommand,
  "clip-watch off": runClipWatchStopCommand,
  "paragraph title": runTitleCommand,
  "format clear": runFormatClearCommand,
  "paragraph clist": runChecklistCommand,
  "paragraph ulist": runUnorderedListCommand,
  "paragraph olist": runOrderedListCommand,
  "format bold": makeInlineWrapExecutor("**", "**", "bold"),
  "format italic": makeInlineWrapExecutor("*", "*", "italic"),
  "format strike": makeInlineWrapExecutor("~~", "~~", "strikethrough"),
  "format code": makeInlineWrapExecutor("`", "`", "inline code"),
  "w": runWriteCommand,
  "wq": runWriteQuitCommand,
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

  const substituteResult = tryExecuteVimSubstituteFromWasm(
    snapshot,
    rawInput,
    runtime.mode,
  );
  if (substituteResult) {
    return {
      message: substituteResult.message,
      operations: substituteResult.operations,
    };
  }

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
