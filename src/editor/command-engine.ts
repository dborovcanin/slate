import { EditorView } from "@codemirror/view";
import { executeExCommand } from "./ex-commands.ts";
import { openDatePicker } from "./date-picker.ts";
import { insertAtSelection } from "./editor-utils.ts";
import { applyMarkdownFormat } from "./markdown-format.ts";

export type CommandMode = "vim" | "editor";

export interface CommandSuggestion {
  value: string;
  description: string;
}

export interface CommandExecutionOptions {
  mode: CommandMode;
  dateFormat?: string;
  onExitCommand?: () => Promise<void> | void;
}

interface CommandDefinition {
  value: string;
  aliases?: string[];
  description: string;
  modes: CommandMode[];
  execute: (view: EditorView, options: CommandExecutionOptions) => Promise<string>;
}

function normalizeCommand(input: string): string {
  return input.trim().replace(/^:/, "").toLowerCase();
}

async function runDateCommand(view: EditorView, options: CommandExecutionOptions): Promise<string> {
  const value = await openDatePicker(options.dateFormat ?? "%Y-%m-%d");
  if (!value) return "date cancelled";
  insertAtSelection(view, value);
  return `inserted ${value}`;
}

async function runSumCommand(
  view: EditorView,
  _options: CommandExecutionOptions,
  command: string,
): Promise<string> {
  return executeExCommand(view, command);
}

async function runFormatCommand(view: EditorView): Promise<string> {
  const changed = applyMarkdownFormat(view);
  return changed ? "markdown formatted" : "already formatted";
}

async function runQuitCommand(_view: EditorView, options: CommandExecutionOptions): Promise<string> {
  if (!options.onExitCommand) return "quit unavailable";
  await options.onExitCommand();
  return "quit";
}

const COMMAND_DEFINITIONS: CommandDefinition[] = [
  {
    value: "sum",
    description: "sum paragraph (default scope)",
    modes: ["vim", "editor"],
    execute: (view, options) => runSumCommand(view, options, "sum"),
  },
  {
    value: "sum list",
    description: "sum list at cursor",
    modes: ["vim", "editor"],
    execute: (view, options) => runSumCommand(view, options, "sum list"),
  },
  {
    value: "sum table",
    description: "sum markdown table at cursor",
    modes: ["vim", "editor"],
    execute: (view, options) => runSumCommand(view, options, "sum table"),
  },
  {
    value: "sum doc",
    description: "sum whole document",
    aliases: ["sum_all", "sum all"],
    modes: ["vim", "editor"],
    execute: (view, options) => runSumCommand(view, options, "sum doc"),
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
    execute: (view) => runFormatCommand(view),
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
  view: EditorView,
  rawInput: string,
  options: CommandExecutionOptions,
): Promise<string> {
  const normalizedInput = normalizeCommand(rawInput);
  if (!normalizedInput) return "";
  const command = availableCommands(options.mode).find((def) =>
    commandMatches(def, normalizedInput),
  );
  if (!command) return `unknown command: ${normalizedInput}`;
  return command.execute(view, options);
}
