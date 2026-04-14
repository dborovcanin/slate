import {
  Prec,
  RangeSetBuilder,
  StateEffect,
  Transaction,
  type Extension,
} from "@codemirror/state";
import {
  Decoration,
  EditorView,
  ViewPlugin,
  keymap,
  type DecorationSet,
  type ViewUpdate,
} from "@codemirror/view";

interface SearchMatch {
  from: number;
  to: number;
}

const searchRefreshEffect = StateEffect.define<null>();

const decMatch = Decoration.mark({ class: "cm-search-match" });
const decCurrent = Decoration.mark({ class: "cm-search-current" });

function findMatches(text: string, query: string): SearchMatch[] {
  if (!query) return [];

  const haystack = text.toLocaleLowerCase();
  const needle = query.toLocaleLowerCase();
  const matches: SearchMatch[] = [];
  let from = 0;

  while (from <= haystack.length) {
    const idx = haystack.indexOf(needle, from);
    if (idx < 0) break;
    matches.push({ from: idx, to: idx + needle.length });
    from = idx + needle.length;
  }

  return matches;
}

function nearestMatchIndex(matches: readonly SearchMatch[], pos: number): number {
  for (let i = 0; i < matches.length; i++) {
    if ((matches[i]?.from ?? 0) >= pos) return i;
  }
  return 0;
}

function isCtrlLike(event: KeyboardEvent): boolean {
  return (event.ctrlKey || event.metaKey) && !event.altKey;
}

class EditorSearchController {
  decorations: DecorationSet = Decoration.none;

  private query = "";
  private matches: SearchMatch[] = [];
  private current = -1;

  private originAnchor = 0;
  private originHead = 0;

  private overlay: HTMLDivElement | null = null;
  private inputEl: HTMLInputElement | null = null;
  private metaEl: HTMLSpanElement | null = null;

  constructor(private readonly view: EditorView) {}

  update(update: ViewUpdate) {
    if (!update.docChanged || !this.query) return;
    const pivot = this.currentMatch()?.from ?? this.view.state.selection.main.head;
    this.recomputeMatches(pivot, true);
    this.refreshDecorations(true);
    if (this.overlay) {
      this.moveToCurrent();
      this.syncOverlayMeta();
    }
  }

  destroy() {
    this.closeOverlay();
  }

  hasMatches(): boolean {
    return this.matches.length > 0;
  }

  isOverlayTarget(target: EventTarget | null): boolean {
    return target instanceof Element && target.closest(".editor-search-bar") !== null;
  }

  open() {
    if (this.overlay && this.inputEl) {
      this.inputEl.focus();
      this.inputEl.select();
      return;
    }

    const main = this.view.state.selection.main;
    this.originAnchor = main.anchor;
    this.originHead = main.head;

    this.query = "";
    this.matches = [];
    this.current = -1;
    this.refreshDecorations();

    const bar = document.createElement("div");
    bar.className = "editor-search-bar";

    const prefix = document.createElement("span");
    prefix.className = "editor-search-prefix";
    prefix.textContent = "/";

    const input = document.createElement("input");
    input.className = "editor-search-input";
    input.type = "text";
    input.placeholder = "search";
    input.spellcheck = false;
    input.autocomplete = "off";
    input.setAttribute("autocorrect", "off");
    input.setAttribute("autocapitalize", "off");
    input.setAttribute("aria-label", "Search");

    const meta = document.createElement("span");
    meta.className = "editor-search-meta";

    bar.appendChild(prefix);
    bar.appendChild(input);
    bar.appendChild(meta);
    this.view.dom.appendChild(bar);

    this.overlay = bar;
    this.inputEl = input;
    this.metaEl = meta;
    this.syncOverlayMeta();

    input.addEventListener("input", () => {
      this.query = input.value;
      this.recomputeMatches(this.originHead, false);
      this.moveToCurrent();
      this.refreshDecorations();
      this.syncOverlayMeta();
    });

    input.addEventListener("keydown", (event) => {
      event.stopPropagation();

      if (event.key === "Escape") {
        event.preventDefault();
        this.cancel();
        return;
      }

      if (event.key === "Enter") {
        event.preventDefault();
        this.confirm();
        return;
      }

      const key = event.key.toLowerCase();
      if (isCtrlLike(event) && key === "n") {
        event.preventDefault();
        this.next();
        return;
      }
      if (isCtrlLike(event) && key === "p") {
        event.preventDefault();
        this.prev();
        return;
      }
    });

    window.setTimeout(() => input.focus(), 0);
  }

  confirm() {
    this.closeOverlay();
    this.view.focus();
  }

  cancel() {
    this.query = "";
    this.matches = [];
    this.current = -1;
    this.refreshDecorations();
    this.closeOverlay();
    this.view.dispatch({
      selection: {
        anchor: this.originAnchor,
        head: this.originHead,
      },
      scrollIntoView: true,
      annotations: Transaction.addToHistory.of(false),
    });
    this.view.focus();
  }

  next(): boolean {
    if (this.matches.length === 0) return false;
    this.current = (this.current + 1) % this.matches.length;
    this.moveToCurrent();
    this.refreshDecorations();
    this.syncOverlayMeta();
    return true;
  }

  prev(): boolean {
    if (this.matches.length === 0) return false;
    this.current = (this.current + this.matches.length - 1) % this.matches.length;
    this.moveToCurrent();
    this.refreshDecorations();
    this.syncOverlayMeta();
    return true;
  }

  private closeOverlay() {
    if (this.overlay) this.overlay.remove();
    this.overlay = null;
    this.inputEl = null;
    this.metaEl = null;
  }

  private currentMatch(): SearchMatch | null {
    if (this.current < 0 || this.current >= this.matches.length) return null;
    return this.matches[this.current] ?? null;
  }

  private moveToCurrent() {
    const match = this.currentMatch();
    if (!match) return;
    this.view.dispatch({
      selection: { anchor: match.from, head: match.from },
      scrollIntoView: true,
      annotations: Transaction.addToHistory.of(false),
    });
  }

  private recomputeMatches(referencePos: number, keepNearestCurrent: boolean) {
    const previousPos = keepNearestCurrent ? this.currentMatch()?.from ?? null : null;
    this.matches = findMatches(this.view.state.doc.toString(), this.query);
    if (this.matches.length === 0) {
      this.current = -1;
      return;
    }

    if (previousPos !== null) {
      this.current = nearestMatchIndex(this.matches, previousPos);
      return;
    }

    this.current = nearestMatchIndex(this.matches, referencePos);
  }

  private syncOverlayMeta() {
    if (!this.metaEl) return;
    if (!this.query) {
      this.metaEl.textContent = "/";
      return;
    }
    if (this.matches.length === 0 || this.current < 0) {
      this.metaEl.textContent = "no matches";
      return;
    }
    this.metaEl.textContent = `${this.current + 1}/${this.matches.length}`;
  }

  private refreshDecorations(silent = false) {
    if (this.matches.length === 0) {
      this.decorations = Decoration.none;
    } else {
      const builder = new RangeSetBuilder<Decoration>();
      for (let i = 0; i < this.matches.length; i++) {
        const match = this.matches[i];
        if (!match) continue;
        builder.add(match.from, match.to, i === this.current ? decCurrent : decMatch);
      }
      this.decorations = builder.finish();
    }

    if (silent) return;
    this.view.dispatch({
      effects: searchRefreshEffect.of(null),
      annotations: Transaction.addToHistory.of(false),
    });
  }
}

const searchPlugin = ViewPlugin.fromClass(EditorSearchController, {
  decorations: (plugin) => plugin.decorations,
});

function withSearchController(view: EditorView): EditorSearchController | null {
  return view.plugin(searchPlugin);
}

export function openEditorSearch(view: EditorView): boolean {
  const plugin = withSearchController(view);
  if (!plugin) return false;
  plugin.open();
  return true;
}

export function editorSearchNext(view: EditorView): boolean {
  const plugin = withSearchController(view);
  if (!plugin) return false;
  return plugin.next();
}

export function editorSearchPrev(view: EditorView): boolean {
  const plugin = withSearchController(view);
  if (!plugin) return false;
  return plugin.prev();
}

export function editorSearchHasMatches(view: EditorView): boolean {
  const plugin = withSearchController(view);
  return plugin?.hasMatches() ?? false;
}

export function isEditorSearchOverlayTarget(view: EditorView, target: EventTarget | null): boolean {
  const plugin = withSearchController(view);
  return plugin?.isOverlayTarget(target) ?? false;
}

export function editorSearchExtensions(): Extension[] {
  return [
    searchPlugin,
    Prec.highest(
      keymap.of([
        {
          key: "Ctrl-f",
          run: openEditorSearch,
          preventDefault: true,
        },
        {
          key: "Mod-f",
          run: openEditorSearch,
          preventDefault: true,
        },
      ]),
    ),
  ];
}
