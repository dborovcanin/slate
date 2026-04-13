import { ResolvedContext } from "./context.ts";
import { replaceRange } from "./operations.ts";
import { executeSumCommand, type SumExpressionEvaluator } from "./sum.ts";
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
  pickDate?: (dateFormat: string) => Promise<string | null>;
  evaluateExpression?: SumExpressionEvaluator;
  copyText?: (text: string) => Promise<void> | void;
  onQuit?: () => Promise<void> | void;
  formatMarkdown?: (input: string) => string;
}

export interface CommandExecutionResult {
  message: string;
  operations: EditOperation[];
}

interface CommandDefinition {
  value: string;
  aliases?: string[];
  description: string;
  modes: CommandMode[];
  execute: (
    normalizedInput: string,
    ctx: ResolvedContext,
    runtime: CommandRuntime,
  ) => Promise<CommandExecutionResult>;
}

function normalizeCommand(input: string): string {
  return input.trim().replace(/^:/, "").toLowerCase();
}

async function runDateCommand(
  _normalizedInput: string,
  ctx: ResolvedContext,
  runtime: CommandRuntime,
): Promise<CommandExecutionResult> {
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

async function runFormatCommand(
  _normalizedInput: string,
  ctx: ResolvedContext,
  runtime: CommandRuntime,
): Promise<CommandExecutionResult> {
  const source = ctx.text();
  const formatter = runtime.formatMarkdown ?? ((value: string) => value);
  const formatted = formatter(source);
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

const COMMAND_DEFINITIONS: CommandDefinition[] = [
  {
    value: "sum",
    description: "sum paragraph (default scope)",
    modes: ["vim", "editor"],
    execute: runSumCommand,
  },
  {
    value: "sum list",
    description: "sum list at cursor",
    modes: ["vim", "editor"],
    execute: runSumCommand,
  },
  {
    value: "sum row",
    aliases: ["sum_row"],
    description: "sum markdown table per row at cursor",
    modes: ["vim", "editor"],
    execute: runSumCommand,
  },
  {
    value: "sum column",
    aliases: ["sum_column"],
    description: "sum markdown table per column at cursor",
    modes: ["vim", "editor"],
    execute: runSumCommand,
  },
  {
    value: "sum doc",
    description: "sum whole document",
    aliases: ["sum_all", "sum all"],
    modes: ["vim", "editor"],
    execute: runSumCommand,
  },
  {
    value: "date",
    description: "insert picked date",
    modes: ["vim", "editor"],
    execute: runDateCommand,
  },
  {
    value: "format",
    description: "format markdown document",
    aliases: ["fmt"],
    modes: ["vim", "editor"],
    execute: runFormatCommand,
  },
  {
    value: "q",
    description: "hide window",
    aliases: ["q!"],
    modes: ["vim"],
    execute: runQuitCommand,
  },
];

function availableCommands(mode: CommandMode): CommandDefinition[] {
  return COMMAND_DEFINITIONS.filter((command) => command.modes.includes(mode));
}

function dedupeByValue(commands: CommandDefinition[]): CommandDefinition[] {
  const seen = new Set<string>();
  const out: CommandDefinition[] = [];
  for (const command of commands) {
    if (seen.has(command.value)) continue;
    seen.add(command.value);
    out.push(command);
  }
  return out;
}

function commandMatches(def: CommandDefinition, normalizedInput: string): boolean {
  if (def.value === normalizedInput) return true;
  if (!def.aliases) return false;
  return def.aliases.some((alias) => alias === normalizedInput);
}

export function listCommandSuggestions(mode: CommandMode, rawInput: string): CommandSuggestion[] {
  const query = normalizeCommand(rawInput);
  const commands = dedupeByValue(availableCommands(mode));
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

export async function executeCommand(
  snapshot: EditorContextSnapshot,
  rawInput: string,
  runtime: CommandRuntime,
): Promise<CommandExecutionResult> {
  const normalizedInput = normalizeCommand(rawInput);
  if (!normalizedInput) return { message: "", operations: [] };

  const command = availableCommands(runtime.mode).find((def) =>
    commandMatches(def, normalizedInput),
  );
  if (!command) {
    return { message: `unknown command: ${normalizedInput}`, operations: [] };
  }

  const ctx = new ResolvedContext(snapshot);
  return command.execute(normalizedInput, ctx, runtime);
}
