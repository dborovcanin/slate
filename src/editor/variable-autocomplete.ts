import {
  autocompletion,
  completionStatus,
  type Completion,
  type CompletionContext,
  type CompletionResult,
  type CompletionSource,
} from "@codemirror/autocomplete";
import { ViewPlugin, type EditorView, type ViewUpdate } from "@codemirror/view";
import { variableIndexField } from "./calc-decoration.ts";
import { getCrossNoteVars } from "../api.ts";
import type { VariableIndexEntry } from "../api.ts";

// Matches [[SHORTID]]. optionally followed by a partial variable name.
const CROSS_NOTE_PREFIX_RE = /\[\[([A-Za-z0-9]{8})\]\]\.([A-Za-z0-9_][A-Za-z0-9_ ]*)?$/;

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
    // Defer to crossNoteCompletionSource when inside [[SHORTID]]. syntax.
    const textBefore = line.text.slice(0, context.pos - line.from);
    if (CROSS_NOTE_PREFIX_RE.test(textBefore)) return null;

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

export function crossNoteCompletionSource(maxSuggestions: number): CompletionSource {
  return async (context: CompletionContext): Promise<CompletionResult | null> => {
    const selection = context.state.selection.main;
    if (!selection.empty || selection.head !== context.pos) return null;

    const line = context.state.doc.lineAt(context.pos);
    const textBefore = line.text.slice(0, context.pos - line.from);
    const match = CROSS_NOTE_PREFIX_RE.exec(textBefore);
    if (!match) return null;

    const shortId = match[1]!;
    const partialRaw = match[2] ?? "";
    const partial = partialRaw.trim().toLowerCase();

    // from = position right after the dot
    const dotOffset = textBefore.lastIndexOf(`[[${shortId}]].`);
    const varStart = line.from + dotOffset + `[[${shortId}]].`.length;

    const snapshotDoc = context.state.doc;
    let vars: VariableIndexEntry[];
    try {
      vars = await getCrossNoteVars(shortId);
    } catch {
      return null;
    }

    if (context.state.doc !== snapshotDoc) return null;

    const filtered = vars
      .filter((v) => v.normalized.startsWith(partial) && v.normalized !== partial)
      .sort((a, b) => a.normalized.length - b.normalized.length || a.normalized.localeCompare(b.normalized))
      .slice(0, maxSuggestions);

    if (filtered.length === 0) return null;

    return {
      from: varStart,
      to: context.pos,
      options: filtered.map((v): Completion => ({
        label: v.name,
        type: "variable",
        detail: `[[${shortId}]]`,
        apply: v.name,
      })),
      filter: false,
    };
  };
}

export function variableAutocompleteExtensions(
  options: VariableAutocompleteOptions = {},
) {
  const enabled = options.enabled ?? true;
  const maxSuggestions = Math.max(1, Math.min(32, options.maxSuggestions ?? 8));

  const sources: CompletionSource[] = [crossNoteCompletionSource(maxSuggestions)];

  if (enabled) {
    const minChars = Math.max(1, Math.min(64, options.minChars ?? 3));
    sources.unshift(variableCompletionSource(minChars, maxSuggestions));
  }

  return [
    autocompletion({
      override: sources,
      activateOnTyping: true,
      closeOnBlur: true,
      defaultKeymap: true,
      maxRenderedOptions: maxSuggestions,
    }),
  ];
}

const TOOLTIP_MARGIN = 6;

/**
 * Repositions the autocomplete tooltip at the cursor position after each CM
 * layout cycle. Tries: lower-right → lower-left → upper-right → upper-left.
 * CM positions the tooltip at `from` (start of the completion range), which
 * may differ from the cursor. This plugin overrides that with cursor tracking.
 */
export function autocompleteTooltipPositioner() {
  return ViewPlugin.fromClass(
    class {
      private rafId: number | null = null;
      private view: EditorView;

      constructor(view: EditorView) {
        this.view = view;
      }

      update(update: ViewUpdate) {
        // Only run when autocomplete is active to avoid unnecessary RAF cost.
        const status = completionStatus(update.state);
        if (status !== "active" && status !== "pending") return;
        if (this.rafId !== null) return;
        this.rafId = requestAnimationFrame(() => {
          this.rafId = null;
          this.reposition();
        });
      }

      private reposition() {
        // Find the autocomplete tooltip. Use the editor's own document to avoid
        // cross-frame issues.
        const doc = this.view.dom.ownerDocument;
        const tooltip = doc.querySelector(".cm-tooltip-autocomplete") as HTMLElement | null;
        if (!tooltip) return;

        const cursor = this.view.state.selection.main.head;
        const cc = this.view.coordsAtPos(cursor);
        if (!cc) return;

        const w = tooltip.offsetWidth;
        const h = tooltip.offsetHeight;
        if (!w || !h) return;

        const vw = window.innerWidth;
        const vh = window.innerHeight;
        const m = TOOLTIP_MARGIN;

        // Evaluate the four candidate positions in preference order.
        let left: number, top: number;
        if (cc.left + w <= vw - m && cc.bottom + h <= vh - m) {
          // Lower-right (preferred)
          left = cc.left;
          top = cc.bottom;
        } else if (cc.left - w >= m && cc.bottom + h <= vh - m) {
          // Lower-left
          left = cc.left - w;
          top = cc.bottom;
        } else if (cc.left + w <= vw - m && cc.top - h >= m) {
          // Upper-right
          left = cc.left;
          top = cc.top - h;
        } else {
          // Upper-left (fallback)
          left = Math.max(m, cc.left - w);
          top = Math.max(m, cc.top - h);
        }

        tooltip.style.left = `${left}px`;
        tooltip.style.top = `${top}px`;
      }

      destroy() {
        if (this.rafId !== null) {
          cancelAnimationFrame(this.rafId);
          this.rafId = null;
        }
      }
    },
  );
}
