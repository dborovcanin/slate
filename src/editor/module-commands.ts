import type { NoteModules } from "../api.ts";

export interface NoteModuleCommandRuntime {
  getNoteModules?: () => NoteModules | null;
  setNoteModules?: (modules: NoteModules) => Promise<void> | void;
}

export interface NoteModuleCommandSuggestion {
  value: string;
  description: string;
}

type ModuleName = "math" | "table" | "variables" | "style";

const MODULES: readonly ModuleName[] = ["math", "table", "variables", "style"];

const SUGGESTIONS: NoteModuleCommandSuggestion[] = [
  { value: "module status", description: "show modules for current note" },
  { value: "module on math", description: "enable math module" },
  { value: "module off math", description: "disable math module" },
  { value: "module on table", description: "enable table module" },
  { value: "module off table", description: "disable table module" },
  { value: "module on variables", description: "enable variables module" },
  { value: "module off variables", description: "disable variables module" },
  { value: "module on style", description: "enable style module" },
  { value: "module off style", description: "disable style module" },
];

function normalizeCommand(rawInput: string): string {
  return rawInput.trim().replace(/^:/, "").toLowerCase();
}

function formatModules(modules: NoteModules): string {
  const parts = MODULES.map((name) => `${name}=${modules[name] ? "on" : "off"}`);
  return `modules ${parts.join(" ")}`;
}

function parseModuleName(token: string | undefined): ModuleName | null {
  if (!token) return null;
  const normalized = token.trim().toLowerCase();
  return MODULES.includes(normalized as ModuleName) ? (normalized as ModuleName) : null;
}

export function listNoteModuleCommandSuggestions(rawInput: string): NoteModuleCommandSuggestion[] {
  const normalized = normalizeCommand(rawInput);
  if (!normalized) return [...SUGGESTIONS];
  if (!normalized.startsWith("module") && !normalized.startsWith("modules")) return [];
  return SUGGESTIONS.filter((entry) => entry.value.startsWith(normalized));
}

export async function tryExecuteNoteModuleCommand(
  rawInput: string,
  runtime: NoteModuleCommandRuntime,
): Promise<string | null> {
  const normalized = normalizeCommand(rawInput);
  if (!normalized) return null;
  const tokens = normalized.split(/\s+/).filter((token) => token.length > 0);
  const root = tokens[0];
  if (root !== "module" && root !== "modules") return null;

  const current = runtime.getNoteModules?.() ?? null;
  if (!current) return "module unavailable";

  const action = tokens[1] ?? "status";
  if (action === "status") {
    return formatModules(current);
  }

  if (action !== "on" && action !== "off" && action !== "toggle") {
    return "module usage: module [status|on|off|toggle] <math|table|variables|style>";
  }

  const target = parseModuleName(tokens[2]);
  if (!target) {
    return "module usage: module [status|on|off|toggle] <math|table|variables|style>";
  }

  const next: NoteModules = { ...current };
  if (action === "toggle") next[target] = !next[target];
  else next[target] = action === "on";

  await runtime.setNoteModules?.(next);
  return formatModules(next);
}
