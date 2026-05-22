import type { Note, NoteModules, RuntimeFlags, ThemeConfig } from "../api.ts";

export const DEFAULT_NOTE_MODULES: NoteModules = {
  math: true,
  table: true,
  variables: true,
  style: true,
  cross_note: true,
};

export function normalizeModules(modules: Partial<NoteModules> | null | undefined): NoteModules {
  if (!modules) return { ...DEFAULT_NOTE_MODULES };
  return {
    math: modules.math ?? true,
    table: modules.table ?? true,
    variables: modules.variables ?? true,
    style: modules.style ?? true,
    cross_note: modules.cross_note ?? true,
  };
}

export function modulesForNote(note: Note | null, config: ThemeConfig): NoteModules {
  return normalizeModules(note?.modules ?? config.default_modules);
}

export function effectiveModules(modules: NoteModules, flags: RuntimeFlags): NoteModules {
  return {
    math: modules.math && !flags.calc_disable,
    table: modules.table,
    variables: modules.variables && !flags.autocomplete_disable,
    style: modules.style && !flags.markdown_disable,
    cross_note: modules.cross_note,
  };
}
