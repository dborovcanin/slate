import {
  autocompletion,
  type Completion,
  type CompletionContext,
  type CompletionResult,
  type CompletionSource,
} from "@codemirror/autocomplete";
import { variableIndexField } from "./calc-decoration.ts";
import type { VariableIndexEntry } from "../api.ts";

export interface VariableAutocompleteOptions {
  enabled?: boolean;
  minChars?: number;
  maxSuggestions?: number;
}

export interface CompletionPrefix {
  fromCol: number;
  toCol: number;
  query: string;
}

function isQueryChar(ch: string): boolean {
  return /[A-Za-z0-9_ ]/.test(ch);
}

export function extractCompletionPrefix(lineText: string, cursorCol: number): CompletionPrefix | null {
  const col = Math.max(0, Math.min(cursorCol, lineText.length));
  let from = col;

  while (from > 0 && isQueryChar(lineText[from - 1] ?? "")) {
    from--;
  }

  // Skip leading spaces in the extracted span so replacement does not swallow separators.
  while (from < col && lineText[from] === " ") {
    from++;
  }

  if (from >= col) return null;
  const query = lineText.slice(from, col);
  if (query.length === 0 || query.endsWith(" ")) return null;

  return {
    fromCol: from,
    toCol: col,
    query,
  };
}

export function buildVariableSuggestions(
  variables: VariableIndexEntry[],
  query: string,
  minChars: number,
  maxSuggestions: number,
): VariableIndexEntry[] {
  const normalizedQuery = query.trim().toLowerCase();
  if (normalizedQuery.length < minChars) return [];

  const byNormalized = new Map<string, VariableIndexEntry>();
  for (const entry of variables) {
    if (!byNormalized.has(entry.normalized)) {
      byNormalized.set(entry.normalized, entry);
    }
  }

  return [...byNormalized.values()]
    .filter(
      (entry) =>
        entry.normalized.startsWith(normalizedQuery) && entry.normalized !== normalizedQuery,
    )
    .sort(
      (a, b) =>
        a.normalized.length - b.normalized.length ||
        a.normalized.localeCompare(b.normalized) ||
        a.line - b.line,
    )
    .slice(0, maxSuggestions);
}

function toCompletion(entry: VariableIndexEntry): Completion {
  return {
    label: entry.name,
    type: "variable",
    detail: `line ${entry.line}`,
    apply: entry.name,
  };
}

function variableCompletionSource(
  minChars: number,
  maxSuggestions: number,
): CompletionSource {
  return (context: CompletionContext): CompletionResult | null => {
    const selection = context.state.selection.main;
    if (!selection.empty || selection.head !== context.pos) return null;

    const line = context.state.doc.lineAt(context.pos);
    const prefix = extractCompletionPrefix(line.text, context.pos - line.from);
    if (!prefix) return null;

    const variables = context.state.field(variableIndexField, false) ?? [];
    const suggestions = buildVariableSuggestions(
      variables,
      prefix.query,
      minChars,
      maxSuggestions,
    );

    if (suggestions.length === 0) return null;

    return {
      from: line.from + prefix.fromCol,
      to: line.from + prefix.toCol,
      options: suggestions.map(toCompletion),
      filter: false,
    };
  };
}

export function makeVariableCompletionSource(
  options: VariableAutocompleteOptions = {},
): CompletionSource | null {
  const enabled = options.enabled ?? true;
  if (!enabled) return null;
  const minChars = Math.max(1, Math.min(64, options.minChars ?? 3));
  const maxSuggestions = Math.max(1, Math.min(32, options.maxSuggestions ?? 8));
  return variableCompletionSource(minChars, maxSuggestions);
}

export function variableAutocompleteExtensions(
  options: VariableAutocompleteOptions = {},
) {
  const source = makeVariableCompletionSource(options);
  if (!source) return [];

  const maxSuggestions = Math.max(1, Math.min(32, options.maxSuggestions ?? 8));
  return [
    autocompletion({
      override: [source],
      activateOnTyping: true,
      closeOnBlur: true,
      defaultKeymap: true,
      maxRenderedOptions: maxSuggestions,
    }),
  ];
}
