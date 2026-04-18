import { keymap, EditorView, ViewPlugin, type ViewUpdate } from "@codemirror/view";
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

interface ActiveCompletion {
  from: number;
  to: number;
  query: string;
  suggestions: VariableIndexEntry[];
  selected: number;
}

class VariableAutocompletePlugin {
  private readonly view: EditorView;
  private readonly minChars: number;
  private readonly maxSuggestions: number;
  private readonly menu: HTMLDivElement;
  private active: ActiveCompletion | null = null;
  private animateItems = true;

  constructor(view: EditorView, minChars: number, maxSuggestions: number) {
    this.view = view;
    this.minChars = minChars;
    this.maxSuggestions = maxSuggestions;

    this.menu = document.createElement("div");
    this.menu.className = "variable-autocomplete";
    this.menu.style.display = "none";
    this.view.dom.appendChild(this.menu);

    this.refresh();
  }

  update(_update: ViewUpdate) {
    this.refresh();
  }

  destroy() {
    this.menu.remove();
    this.active = null;
  }

  moveSelection(delta: number): boolean {
    if (!this.active || this.active.suggestions.length === 0) return false;
    const len = this.active.suggestions.length;
    this.active.selected = (this.active.selected + delta + len) % len;
    this.animateItems = false;
    this.render();
    return true;
  }

  accept(): boolean {
    if (!this.active || this.active.suggestions.length === 0) return false;

    const pick = this.active.suggestions[this.active.selected];
    if (!pick) return false;

    this.view.dispatch({
      changes: { from: this.active.from, to: this.active.to, insert: pick.name },
      selection: { anchor: this.active.from + pick.name.length },
      scrollIntoView: true,
    });

    this.close();
    return true;
  }

  close(): boolean {
    if (!this.active) return false;
    this.active = null;
    this.menu.style.display = "none";
    this.menu.replaceChildren();
    return true;
  }

  isOpen(): boolean {
    return this.active !== null;
  }

  private refresh() {
    const selection = this.view.state.selection;
    if (selection.ranges.length !== 1 || !selection.main.empty) {
      this.close();
      return;
    }

    const pos = selection.main.head;
    const line = this.view.state.doc.lineAt(pos);
    const prefix = extractCompletionPrefix(line.text, pos - line.from);
    if (!prefix) {
      this.close();
      return;
    }

    const variables = this.view.state.field(variableIndexField, false) ?? [];
    const suggestions = buildVariableSuggestions(
      variables,
      prefix.query,
      this.minChars,
      this.maxSuggestions,
    );

    if (suggestions.length === 0) {
      this.close();
      return;
    }

    const from = line.from + prefix.fromCol;
    const to = line.from + prefix.toCol;
    const selected =
      this.active &&
      this.active.query === prefix.query &&
      this.active.from === from &&
      this.active.to === to
        ? Math.min(this.active.selected, suggestions.length - 1)
        : 0;

    this.active = {
      from,
      to,
      query: prefix.query,
      suggestions,
      selected,
    };

    this.render();
  }

  private render() {
    if (!this.active) {
      this.menu.style.display = "none";
      this.menu.replaceChildren();
      this.animateItems = true;
      return;
    }

    this.menu.replaceChildren();

    this.active.suggestions.forEach((entry, index) => {
      const item = document.createElement("button");
      item.type = "button";
      item.className = "variable-autocomplete-item";
      item.style.setProperty("--item-index", `${index}`);
      if (!this.animateItems) {
        item.style.animation = "none";
      }
      if (index === this.active?.selected) {
        item.classList.add("variable-autocomplete-item--active");
      }

      const title = document.createElement("span");
      title.className = "variable-autocomplete-item-title";
      title.textContent = entry.name;

      const hint = document.createElement("span");
      hint.className = "variable-autocomplete-item-hint";
      hint.textContent = `line ${entry.line}`;

      item.appendChild(title);
      item.appendChild(hint);

      item.addEventListener("mousedown", (event) => {
        event.preventDefault();
      });
      item.addEventListener("click", () => {
        if (this.active) {
          this.active.selected = index;
        }
        void this.accept();
      });

      this.menu.appendChild(item);
    });

    const coords = this.view.coordsAtPos(this.active.to);
    if (coords) {
      const host = this.view.dom.getBoundingClientRect();
      this.menu.style.left = `${Math.max(0, coords.left - host.left)}px`;
      this.menu.style.top = `${Math.max(0, coords.bottom - host.top + 4)}px`;
    }

    this.menu.style.display = "flex";
    this.animateItems = false;
  }
}

export function variableAutocompleteExtensions(
  options: VariableAutocompleteOptions = {},
) {
  const enabled = options.enabled ?? true;
  if (!enabled) return [];

  const minChars = Math.max(1, Math.min(64, options.minChars ?? 3));
  const maxSuggestions = Math.max(1, Math.min(32, options.maxSuggestions ?? 8));

  const plugin = ViewPlugin.fromClass(
    class {
      private readonly impl: VariableAutocompletePlugin;

      constructor(view: EditorView) {
        this.impl = new VariableAutocompletePlugin(view, minChars, maxSuggestions);
      }

      update(update: ViewUpdate) {
        this.impl.update(update);
      }

      destroy() {
        this.impl.destroy();
      }

      moveSelection(delta: number): boolean {
        return this.impl.moveSelection(delta);
      }

      accept(): boolean {
        return this.impl.accept();
      }

      close(): boolean {
        return this.impl.close();
      }

      isOpen(): boolean {
        return this.impl.isOpen();
      }
    },
  );

  const km = keymap.of([
    {
      key: "Tab",
      run(view): boolean {
        const instance = view.plugin(plugin);
        if (!instance?.isOpen()) return false;
        return instance.accept();
      },
    },
    {
      key: "Enter",
      run(view): boolean {
        const instance = view.plugin(plugin);
        if (!instance?.isOpen()) return false;
        return instance.accept();
      },
    },
    {
      key: "ArrowDown",
      run(view): boolean {
        const instance = view.plugin(plugin);
        if (!instance?.isOpen()) return false;
        return instance.moveSelection(1);
      },
    },
    {
      key: "ArrowUp",
      run(view): boolean {
        const instance = view.plugin(plugin);
        if (!instance?.isOpen()) return false;
        return instance.moveSelection(-1);
      },
    },
    {
      key: "Escape",
      run(view): boolean {
        const instance = view.plugin(plugin);
        if (!instance?.isOpen()) return false;
        return instance.close();
      },
    },
  ]);

  return [plugin, km];
}
