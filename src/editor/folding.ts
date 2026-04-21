import {
  Annotation,
  type ChangeSet,
  type EditorState,
  type Range,
  StateEffect,
  StateField,
  Text,
} from "@codemirror/state";
import {
  Decoration,
  type DecorationSet,
  EditorView,
  ViewPlugin,
  type ViewUpdate,
  WidgetType,
  keymap,
} from "@codemirror/view";
import {
  codeFolding,
  foldEffect,
  foldService,
  foldState as cmFoldState,
  foldedRanges,
  unfoldEffect,
} from "@codemirror/language";
import { ensureWasmReady, markdownAnalyzeLines } from "./wasm.ts";

type FoldKind = "heading" | "fence";
const MAX_FOLD_ANALYSIS_LINES = 20_000;
const FOLD_VIEWPORT_MARGIN_LINES = 80;
const FOLD_ANALYSIS_IDLE_TIMEOUT_MS = 120;

interface FoldRange {
  startLine: number;
  endLine: number;
  headerFrom: number;
  from: number;
  to: number;
  kind: FoldKind;
}

interface FoldRangesValue {
  ranges: Map<number, FoldRange>;
  rangesByFrom: Map<number, FoldRange>;
}

export type FoldCommandAction = "fold" | "unfold" | "fold-toggle";

export interface FoldCommandResult {
  changed: boolean;
  message: string;
}

export interface FoldRangeDescriptor {
  startLine: number;
  endLine: number;
  kind: FoldKind;
}

interface FoldRangesReplacement {
  ranges: Map<number, FoldRange>;
  docLength: number;
}

interface FoldPlaceholderInfo {
  hiddenLineCount: number;
  kind: FoldKind | null;
}

const setFoldRangesEffect = StateEffect.define<FoldRangesReplacement>();
const foldWasmReadyAnnotation = Annotation.define<boolean>();

class FoldToggleWidget extends WidgetType {
  readonly foldLine: number;
  readonly collapsed: boolean;

  constructor(foldLine: number, collapsed: boolean) {
    super();
    this.foldLine = foldLine;
    this.collapsed = collapsed;
  }

  eq(other: FoldToggleWidget): boolean {
    return other.foldLine === this.foldLine && other.collapsed === this.collapsed;
  }

  toDOM(): HTMLElement {
    const span = document.createElement("span");
    span.className = `cm-fold-toggle${this.collapsed ? " cm-fold-toggle-collapsed" : ""}`;
    span.dataset.foldLine = `${this.foldLine}`;
    span.textContent = this.collapsed ? "" : "";
    span.title = this.collapsed ? "Click to unfold" : "Click to fold";
    span.setAttribute(
      "aria-label",
      this.collapsed
        ? "Folded section toggle. Click to unfold."
        : "Foldable section toggle. Click to fold.",
    );
    return span;
  }

  ignoreEvent(): boolean {
    return false;
  }
}

function foldRangeToSpan(range: FoldRange): number {
  return range.endLine - range.startLine;
}

function buildFoldRanges(doc: Text): Map<number, FoldRange> {
  const lineCount = doc.lines;
  const ranges = new Map<number, FoldRange>();
  if (lineCount <= 1) return ranges;
  if (lineCount > MAX_FOLD_ANALYSIS_LINES) return ranges;

  const lines: string[] = [];
  for (let lineNo = 1; lineNo <= lineCount; lineNo++) {
    lines.push(doc.line(lineNo).text);
  }
  const analyzed = markdownAnalyzeLines(lines, {
    inCodeBlock: false,
    codeFenceLang: null,
  }).lines;

  const nextHeadingAtLevel: number[] = Array(7).fill(lineCount + 1);
  for (let lineNo = lineCount; lineNo >= 1; lineNo--) {
    const current = analyzed[lineNo - 1];
    if (!current) continue;
    const level = current.info.headingLevel;
    if (level === null || current.inCodeBlock) continue;

    const nextHeading = nextHeadingAtLevel[level];
    const endLine = nextHeading <= lineCount ? nextHeading - 1 : lineCount;
    if (endLine > lineNo) {
      const headerFrom = doc.line(lineNo).from;
      const from = doc.line(lineNo + 1).from;
      const to = endLine < lineCount ? doc.line(endLine + 1).from : doc.line(endLine).to;
      if (from < to) {
        ranges.set(lineNo, {
          startLine: lineNo,
          endLine,
          headerFrom,
          from,
          to,
          kind: "heading",
        });
      }
    }
    nextHeadingAtLevel[level] = lineNo;
  }

  let openFenceLine: number | null = null;
  for (let lineNo = 1; lineNo <= lineCount; lineNo++) {
    const current = analyzed[lineNo - 1];
    if (!current || !current.info.isCodeFence) continue;
    if (!current.inCodeBlock) {
      openFenceLine = lineNo;
      continue;
    }

    if (openFenceLine !== null) {
      const startLine = openFenceLine;
      const endLine = lineNo;
      if (endLine > startLine) {
        const headerFrom = doc.line(startLine).from;
        const from = doc.line(startLine + 1).from;
        const to = endLine < lineCount ? doc.line(endLine + 1).from : doc.line(endLine).to;
        if (from < to) {
          ranges.set(startLine, {
            startLine,
            endLine,
            headerFrom,
            from,
            to,
            kind: "fence",
          });
        }
      }
      openFenceLine = null;
    }
  }

  if (openFenceLine !== null && openFenceLine < lineCount) {
    const headerFrom = doc.line(openFenceLine).from;
    const from = doc.line(openFenceLine + 1).from;
    const to = doc.line(lineCount).to;
    if (from < to) {
      ranges.set(openFenceLine, {
        startLine: openFenceLine,
        endLine: lineCount,
        headerFrom,
        from,
        to,
        kind: "fence",
      });
    }
  }

  return ranges;
}

function mapFoldRangesThroughChanges(
  ranges: Map<number, FoldRange>,
  changes: ChangeSet,
  newDoc: Text,
): Map<number, FoldRange> {
  if (ranges.size === 0) return ranges;
  const next = new Map<number, FoldRange>();
  for (const range of ranges.values()) {
    const headerFrom = changes.mapPos(range.headerFrom, -1);
    const from = changes.mapPos(range.from, 1);
    const to = changes.mapPos(range.to, -1);
    if (from >= to) continue;
    if (headerFrom < 0 || headerFrom >= newDoc.length) continue;
    const headerLine = newDoc.lineAt(headerFrom);
    if (headerLine.from !== headerFrom) continue;
    const contentStartLine = from < newDoc.length
      ? newDoc.lineAt(from).number
      : newDoc.lines + 1;
    if (contentStartLine !== headerLine.number + 1) continue;
    const endLineNo = to >= newDoc.length
      ? newDoc.lines
      : Math.max(headerLine.number, newDoc.lineAt(to).number - 1);
    if (endLineNo <= headerLine.number) continue;
    const nextRange: FoldRange = {
      startLine: headerLine.number,
      endLine: endLineNo,
      headerFrom,
      from,
      to,
      kind: range.kind,
    };
    next.set(headerLine.number, nextRange);
  }
  return next;
}

function foldRangesEqual(a: Map<number, FoldRange>, b: Map<number, FoldRange>): boolean {
  if (a === b) return true;
  if (a.size !== b.size) return false;
  for (const [line, left] of a) {
    const right = b.get(line);
    if (!right) return false;
    if (
      left.startLine !== right.startLine ||
      left.endLine !== right.endLine ||
      left.headerFrom !== right.headerFrom ||
      left.from !== right.from ||
      left.to !== right.to ||
      left.kind !== right.kind
    ) {
      return false;
    }
  }
  return true;
}

function indexFoldRanges(ranges: Map<number, FoldRange>): Map<number, FoldRange> {
  const byFrom = new Map<number, FoldRange>();
  for (const range of ranges.values()) {
    byFrom.set(range.from, range);
  }
  return byFrom;
}

const foldRangesField = StateField.define<FoldRangesValue>({
  create(state) {
    const ranges = buildFoldRanges(state.doc);
    return {
      ranges,
      rangesByFrom: indexFoldRanges(ranges),
    };
  },
  update(value, tr) {
    let ranges = value.ranges;
    let changed = false;

    if (tr.docChanged) {
      const nextRanges = mapFoldRangesThroughChanges(ranges, tr.changes, tr.state.doc);
      if (nextRanges !== ranges) {
        ranges = nextRanges;
        changed = true;
      }
    }

    for (const effect of tr.effects) {
      if (!effect.is(setFoldRangesEffect)) continue;
      if (effect.value.docLength !== tr.state.doc.length) continue;
      const nextRanges = effect.value.ranges;
      if (foldRangesEqual(ranges, nextRanges)) continue;
      ranges = nextRanges;
      changed = true;
    }

    if (!changed) return value;
    return {
      ranges,
      rangesByFrom: indexFoldRanges(ranges),
    };
  },
});

type IdleHandle = { cancel(): void };

function scheduleIdle(fn: () => void): IdleHandle {
  const w = globalThis as typeof globalThis & {
    requestIdleCallback?: (cb: () => void, opts?: { timeout: number }) => number;
    cancelIdleCallback?: (handle: number) => void;
  };
  if (typeof w.requestIdleCallback === "function") {
    const id = w.requestIdleCallback(fn, { timeout: FOLD_ANALYSIS_IDLE_TIMEOUT_MS });
    return {
      cancel: () => w.cancelIdleCallback?.(id),
    };
  }
  const id = setTimeout(fn, 0);
  return { cancel: () => clearTimeout(id) };
}

// Heading markers (#), code fence markers (`), and newlines are the only characters
// that can create or destroy a fold boundary. Skip the full re-analysis when the edit
// contains none of these.
const FOLD_STRUCTURAL_RE = /[#`\n]/;

function editMightAffectFolds(update: ViewUpdate): boolean {
  let might = false;
  update.changes.iterChangedRanges((fromA, toA, fromB, toB) => {
    if (might) return;
    if (toA > fromA) {
      const deleted = update.startState.doc.sliceString(fromA, toA);
      if (FOLD_STRUCTURAL_RE.test(deleted)) {
        might = true;
        return;
      }
    }
    if (toB > fromB) {
      const inserted = update.state.doc.sliceString(fromB, toB);
      if (FOLD_STRUCTURAL_RE.test(inserted)) {
        might = true;
      }
    }
  });
  return might;
}

const foldAnalyzerPlugin = ViewPlugin.define((view) => {
  let generation = 0;
  let scheduled: IdleHandle | null = null;
  let destroyed = false;

  function schedule() {
    const myGen = ++generation;
    if (scheduled) {
      scheduled.cancel();
      scheduled = null;
    }
    scheduled = scheduleIdle(() => {
      scheduled = null;
      if (destroyed || myGen !== generation) return;
      const doc = view.state.doc;
      let ranges: Map<number, FoldRange>;
      try {
        ranges = buildFoldRanges(doc);
      } catch (error) {
        console.error("Fold analysis failed:", error);
        return;
      }
      if (destroyed || myGen !== generation) return;
      if (view.state.doc !== doc) {
        schedule();
        return;
      }
      view.dispatch({
        effects: setFoldRangesEffect.of({ ranges, docLength: doc.length }),
      });
    });
  }

  // Build fold ranges once on mount, then rebuild once wasm parser is ready.
  schedule();
  void ensureWasmReady()
    .then(() => {
      if (destroyed) return;
      view.dispatch({ annotations: foldWasmReadyAnnotation.of(true) });
    })
    .catch((error) => {
      console.error("Fold wasm init failed:", error);
    });

  return {
    update(update: ViewUpdate) {
      if (
        update.transactions.some((transaction) =>
          transaction.annotation(foldWasmReadyAnnotation),
        )
      ) {
        schedule();
        return;
      }
      if (update.docChanged && editMightAffectFolds(update)) {
        schedule();
      }
    },
    destroy() {
      destroyed = true;
      if (scheduled) scheduled.cancel();
    },
  };
});

interface FoldVisibleLineSpan {
  fromLine: number;
  toLine: number;
}

function mergeFoldLineSpans(spans: readonly FoldVisibleLineSpan[]): FoldVisibleLineSpan[] {
  if (spans.length <= 1) return [...spans];
  const sorted = [...spans].sort((a, b) => a.fromLine - b.fromLine || a.toLine - b.toLine);
  const merged: FoldVisibleLineSpan[] = [];
  for (const span of sorted) {
    const last = merged[merged.length - 1];
    if (!last || span.fromLine > last.toLine + 1) {
      merged.push({ ...span });
      continue;
    }
    last.toLine = Math.max(last.toLine, span.toLine);
  }
  return merged;
}

function expandedFoldVisibleSpans(view: EditorView): FoldVisibleLineSpan[] {
  if (view.visibleRanges.length === 0) return [];
  const doc = view.state.doc;
  const spans = view.visibleRanges.map(({ from, to }) => ({
    fromLine: Math.max(1, doc.lineAt(from).number - FOLD_VIEWPORT_MARGIN_LINES),
    toLine: Math.min(doc.lines, doc.lineAt(to).number + FOLD_VIEWPORT_MARGIN_LINES),
  }));
  return mergeFoldLineSpans(spans);
}

function foldLineInSpans(lineNumber: number, spans: readonly FoldVisibleLineSpan[]): boolean {
  for (const span of spans) {
    if (lineNumber < span.fromLine) return false;
    if (lineNumber <= span.toLine) return true;
  }
  return false;
}

function isRangeCollapsed(set: DecorationSet, range: FoldRange): boolean {
  let collapsed = false;
  set.between(range.from, range.to, (from, to) => {
    if (from <= range.from && to >= range.to) {
      collapsed = true;
    }
  });
  return collapsed;
}

function buildVisibleFoldToggleDecorations(view: EditorView): DecorationSet {
  const foldRanges = view.state.field(foldRangesField, false);
  if (!foldRanges) return Decoration.none;

  const spans = expandedFoldVisibleSpans(view);
  if (spans.length === 0) return Decoration.none;

  const collapsed = foldedRanges(view.state);
  const doc = view.state.doc;
  const decorationRanges: Range<Decoration>[] = [];
  const visibleRanges = [...foldRanges.ranges.values()]
    .filter((range) => foldLineInSpans(range.startLine, spans))
    .sort((a, b) => a.startLine - b.startLine || a.endLine - b.endLine);

  for (const range of visibleRanges) {
    const collapsedHere = isRangeCollapsed(collapsed, range);
    const line = doc.line(range.startLine);
    decorationRanges.push(
      Decoration.widget({
        side: -1,
        widget: new FoldToggleWidget(range.startLine, collapsedHere),
      }).range(line.from),
    );
    if (collapsedHere) {
      decorationRanges.push(Decoration.line({ class: "cm-folded-start-line" }).range(line.from));
    }
  }

  return decorationRanges.length > 0 ? Decoration.set(decorationRanges, true) : Decoration.none;
}

const foldToggleDecorationsPlugin = ViewPlugin.fromClass(
  class {
    decorations: DecorationSet;

    constructor(view: EditorView) {
      this.decorations = this.safeBuild(view, Decoration.none);
    }

    update(update: ViewUpdate) {
      const foldRangesChanged =
        update.startState.field(foldRangesField, false) !==
        update.state.field(foldRangesField, false);
      const foldedStateChanged =
        update.startState.field(cmFoldState, false) !==
        update.state.field(cmFoldState, false);

      if (!foldRangesChanged && !foldedStateChanged && !update.viewportChanged) {
        return;
      }
      this.decorations = this.safeBuild(update.view, this.decorations);
    }

    private safeBuild(view: EditorView, fallback: DecorationSet): DecorationSet {
      try {
        return buildVisibleFoldToggleDecorations(view);
      } catch (error) {
        console.error("Fold decoration build failed:", error);
        return fallback;
      }
    }
  },
  {
    decorations: (plugin) => plugin.decorations,
  },
);

function findFoldStartForLine(ranges: Map<number, FoldRange>, line: number): number | null {
  if (ranges.has(line)) return line;

  let bestStart: number | null = null;
  let bestSpan = Number.MAX_SAFE_INTEGER;
  for (const range of ranges.values()) {
    if (range.startLine < line && line <= range.endLine) {
      const span = foldRangeToSpan(range);
      if (span < bestSpan) {
        bestSpan = span;
        bestStart = range.startLine;
      }
    }
  }
  return bestStart;
}

function applyFoldStateAtRange(view: EditorView, range: FoldRange, nextCollapsed: boolean): boolean {
  const currentCollapsed = isRangeCollapsed(foldedRanges(view.state), range);
  if (currentCollapsed === nextCollapsed) return false;

  const effect = nextCollapsed
    ? foldEffect.of({ from: range.from, to: range.to })
    : unfoldEffect.of({ from: range.from, to: range.to });

  const main = view.state.selection.main;
  const intersectsHidden = main.from < range.to && range.from < main.to;
  const headInsideHidden = main.head >= range.from && main.head < range.to;

  if (nextCollapsed && (headInsideHidden || intersectsHidden)) {
    const anchor = view.state.doc.line(range.startLine).to;
    view.dispatch({
      effects: [effect],
      selection: { anchor },
      scrollIntoView: true,
    });
    return true;
  }

  view.dispatch({ effects: [effect] });
  return true;
}

function toggleFoldAtLine(view: EditorView, line: number): boolean {
  const foldRanges = view.state.field(foldRangesField, false);
  if (!foldRanges) return false;
  const range = foldRanges.ranges.get(line);
  if (!range) return false;
  const currentCollapsed = isRangeCollapsed(foldedRanges(view.state), range);
  return applyFoldStateAtRange(view, range, !currentCollapsed);
}

function formatFoldMessage(action: "folded" | "unfolded", range: FoldRange): string {
  const hiddenLineCount = range.endLine - range.startLine;
  const hiddenSuffix = hiddenLineCount === 1 ? "" : "s";
  const kind = range.kind === "heading" ? "heading" : "code block";
  return `fold: ${action} ${kind} (${hiddenLineCount} line${hiddenSuffix})`;
}

export function toggleFoldAtCursor(view: EditorView): boolean {
  const foldRanges = view.state.field(foldRangesField, false);
  if (!foldRanges) return false;

  const line = view.state.doc.lineAt(view.state.selection.main.head).number;
  const foldStart = findFoldStartForLine(foldRanges.ranges, line);
  if (foldStart === null) return false;
  const range = foldRanges.ranges.get(foldStart);
  if (!range) return false;

  const currentCollapsed = isRangeCollapsed(foldedRanges(view.state), range);
  return applyFoldStateAtRange(view, range, !currentCollapsed);
}

export function executeFoldCommand(view: EditorView, action: FoldCommandAction): FoldCommandResult {
  const foldRanges = view.state.field(foldRangesField, false);
  if (!foldRanges) {
    return { changed: false, message: "fold: unavailable" };
  }

  const line = view.state.doc.lineAt(view.state.selection.main.head).number;
  const foldStart = findFoldStartForLine(foldRanges.ranges, line);
  if (foldStart === null) {
    return { changed: false, message: "fold: no foldable block at cursor" };
  }

  const range = foldRanges.ranges.get(foldStart);
  if (!range) {
    return { changed: false, message: "fold: no foldable block at cursor" };
  }

  const isCollapsed = isRangeCollapsed(foldedRanges(view.state), range);

  if (action === "fold" && isCollapsed) {
    return { changed: false, message: "fold: already folded" };
  }
  if (action === "unfold" && !isCollapsed) {
    return { changed: false, message: "fold: already unfolded" };
  }

  const nextCollapsed = action === "fold-toggle" ? !isCollapsed : action === "fold";
  const changed = applyFoldStateAtRange(view, range, nextCollapsed);
  if (!changed) {
    return { changed: false, message: "fold: no foldable block at cursor" };
  }

  return {
    changed: true,
    message: formatFoldMessage(nextCollapsed ? "folded" : "unfolded", range),
  };
}

function prepareFoldPlaceholder(
  state: EditorState,
  range: { from: number; to: number },
): FoldPlaceholderInfo {
  const foldRanges = state.field(foldRangesField, false);
  const known = foldRanges?.rangesByFrom.get(range.from);
  if (known) {
    return {
      hiddenLineCount: known.endLine - known.startLine,
      kind: known.kind,
    };
  }

  const firstHiddenLine = state.doc.lineAt(range.from).number;
  let lastHiddenLine: number;
  if (range.to >= state.doc.length) {
    lastHiddenLine = state.doc.lines;
  } else {
    const toLine = state.doc.lineAt(range.to);
    lastHiddenLine = range.to === toLine.from ? toLine.number - 1 : toLine.number;
  }

  return {
    hiddenLineCount: Math.max(1, lastHiddenLine - firstHiddenLine + 1),
    kind: null,
  };
}

const foldMouseHandlers = EditorView.domEventHandlers({
  mousedown: (event, view) => {
    if (event.button !== 0) return false;
    const rawTarget = event.target;
    const target = rawTarget instanceof Element
      ? rawTarget
      : rawTarget instanceof Node
      ? rawTarget.parentElement
      : null;
    if (!target) return false;

    const foldTarget = target.closest(".cm-fold-toggle") as HTMLElement | null;
    if (!foldTarget) return false;

    const lineRaw = foldTarget.dataset.foldLine;
    const line = Number(lineRaw);
    if (!Number.isFinite(line) || line <= 0) return false;

    event.preventDefault();
    event.stopPropagation();
    return toggleFoldAtLine(view, line);
  },
});

const markdownFoldService = foldService.of((state, lineStart, _lineEnd) => {
  const foldRanges = state.field(foldRangesField, false);
  if (!foldRanges) return null;

  const line = state.doc.lineAt(lineStart).number;
  const foldStart = findFoldStartForLine(foldRanges.ranges, line);
  if (foldStart === null) return null;
  const range = foldRanges.ranges.get(foldStart);
  if (!range) return null;

  return { from: range.from, to: range.to };
});

export function foldingExtensions() {
  return [
    foldRangesField,
    foldAnalyzerPlugin,
    markdownFoldService,
    codeFolding({
      preparePlaceholder: prepareFoldPlaceholder,
      placeholderDOM: (_view, onclick, prepared) => {
        const info = prepared as FoldPlaceholderInfo | null;
        const hiddenLineCount = info?.hiddenLineCount ?? 1;
        const span = document.createElement("span");
        span.className = "cm-fold-placeholder";
        if (info?.kind) {
          span.dataset.foldKind = info.kind;
        }
        span.textContent = ` ${hiddenLineCount} line${hiddenLineCount === 1 ? "" : "s"} folded`;
        span.title = "Click to unfold";
        span.setAttribute("aria-label", "Folded section. Click to unfold.");
        span.addEventListener("click", (event) => {
          event.preventDefault();
          event.stopPropagation();
          onclick(event);
        });
        return span;
      },
    }),
    foldToggleDecorationsPlugin,
    foldMouseHandlers,
    keymap.of([
      {
        key: "Ctrl-Alt-z",
        run: toggleFoldAtCursor,
        preventDefault: true,
      },
      {
        key: "Mod-Alt-z",
        run: toggleFoldAtCursor,
        preventDefault: true,
      },
    ]),
  ];
}

export function describeFoldRanges(text: string): FoldRangeDescriptor[] {
  const lines = text.split("\n");
  // Empty string should still be one empty doc line.
  const docLines = lines.length > 0 ? lines : [""];
  const doc = Text.of(docLines);
  const ranges = buildFoldRanges(doc);
  return [...ranges.values()]
    .map((range) => ({
      startLine: range.startLine,
      endLine: range.endLine,
      kind: range.kind,
    }))
    .sort((a, b) => a.startLine - b.startLine || a.endLine - b.endLine);
}
