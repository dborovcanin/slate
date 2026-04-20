import {
  Annotation,
  type ChangeSet,
  RangeSetBuilder,
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

interface FoldStateValue {
  ranges: Map<number, FoldRange>;
  collapsed: Set<number>;
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

const toggleFoldAtLineEffect = StateEffect.define<number>();

interface FoldRangesReplacement {
  ranges: Map<number, FoldRange>;
  docLength: number;
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

class FoldPlaceholderWidget extends WidgetType {
  readonly foldLine: number;
  readonly hiddenLineCount: number;
  readonly kind: FoldKind;

  constructor(foldLine: number, hiddenLineCount: number, kind: FoldKind) {
    super();
    this.foldLine = foldLine;
    this.hiddenLineCount = hiddenLineCount;
    this.kind = kind;
  }

  eq(other: FoldPlaceholderWidget): boolean {
    return other.foldLine === this.foldLine
      && other.hiddenLineCount === this.hiddenLineCount
      && other.kind === this.kind;
  }

  toDOM(): HTMLElement {
    const span = document.createElement("span");
    span.className = "cm-fold-placeholder";
    span.dataset.foldLine = `${this.foldLine}`;
    span.dataset.foldKind = this.kind;
    span.textContent = ` ${this.hiddenLineCount} line${this.hiddenLineCount === 1 ? "" : "s"} folded`;
    span.title = "Click to unfold";
    span.setAttribute("aria-label", "Folded section. Click to unfold.");
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

function remapCollapsed(
  collapsed: Set<number>,
  prevRanges: Map<number, FoldRange>,
  changes: ChangeSet,
  newDoc: Text,
): Set<number> {
  if (collapsed.size === 0) return collapsed;
  const next = new Set<number>();
  for (const prevStart of collapsed) {
    const prev = prevRanges.get(prevStart);
    if (!prev) continue;
    const headerFrom = changes.mapPos(prev.headerFrom, -1);
    if (headerFrom < 0 || headerFrom >= newDoc.length) continue;
    const headerLine = newDoc.lineAt(headerFrom);
    if (headerLine.from !== headerFrom) continue;
    next.add(headerLine.number);
  }
  return next;
}

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

function buildFoldDecorations(
  doc: Text,
  ranges: Map<number, FoldRange>,
  collapsed: Set<number>,
  spans: readonly FoldVisibleLineSpan[],
): DecorationSet {
  if (ranges.size === 0 || spans.length === 0) return Decoration.none;
  const builder = new RangeSetBuilder<Decoration>();
  const visibleToggleRanges = [...ranges.values()]
    .filter((range) => foldLineInSpans(range.startLine, spans))
    .sort((a, b) => a.startLine - b.startLine || a.endLine - b.endLine);

  for (const range of visibleToggleRanges) {
    const startLine = doc.line(range.startLine);
    builder.add(
      startLine.from,
      startLine.from,
      Decoration.widget({
        side: -1,
        widget: new FoldToggleWidget(range.startLine, collapsed.has(range.startLine)),
      }),
    );
  }

  if (collapsed.size === 0) {
    return builder.finish();
  }

  const ordered = [...collapsed]
    .map((line) => ranges.get(line))
    .filter((range): range is FoldRange => !!range)
    .sort((a, b) => a.from - b.from || a.to - b.to);

  let coveredTo = -1;
  for (const range of ordered) {
    if (range.from < coveredTo) continue;
    const hiddenLineCount = range.endLine - range.startLine;
    if (hiddenLineCount <= 0) continue;
    builder.add(
      range.from,
      range.to,
      Decoration.replace({
        block: true,
        widget: new FoldPlaceholderWidget(range.startLine, hiddenLineCount, range.kind),
      }),
    );
    if (foldLineInSpans(range.startLine, spans)) {
      const startLine = doc.line(range.startLine);
      builder.add(startLine.from, startLine.from, Decoration.line({ class: "cm-folded-start-line" }));
    }
    coveredTo = range.to;
  }

  return builder.finish();
}

function normalizeCollapsed(
  collapsed: Set<number>,
  ranges: Map<number, FoldRange>,
): Set<number> {
  const next = new Set<number>();
  for (const line of collapsed) {
    if (ranges.has(line)) next.add(line);
  }
  return next;
}

const foldStateField = StateField.define<FoldStateValue>({
  create(state) {
    const ranges = buildFoldRanges(state.doc);
    const collapsed = new Set<number>();
    return { ranges, collapsed };
  },
  update(value, tr) {
    let ranges = value.ranges;
    let collapsed = value.collapsed;
    let changed = false;

    if (tr.docChanged) {
      const nextRanges = mapFoldRangesThroughChanges(ranges, tr.changes, tr.state.doc);
      const nextCollapsed = remapCollapsed(collapsed, ranges, tr.changes, tr.state.doc);
      if (nextRanges !== ranges || nextCollapsed !== collapsed) {
        ranges = nextRanges;
        collapsed = nextCollapsed;
        changed = true;
      }
    }

    for (const effect of tr.effects) {
      if (effect.is(setFoldRangesEffect)) {
        if (effect.value.docLength !== tr.state.doc.length) continue;
        const nextRanges = effect.value.ranges;
        const nextCollapsed = normalizeCollapsed(collapsed, nextRanges);
        const rangesEqual = foldRangesEqual(ranges, nextRanges);
        const collapsedEqual = setsEqual(collapsed, nextCollapsed);
        if (rangesEqual && collapsedEqual) continue;
        ranges = nextRanges;
        collapsed = nextCollapsed;
        changed = true;
        continue;
      }
      if (!effect.is(toggleFoldAtLineEffect)) continue;
      const line = effect.value;
      if (!ranges.has(line)) continue;
      if (!changed) {
        collapsed = new Set(collapsed);
        changed = true;
      }
      if (collapsed.has(line)) {
        collapsed.delete(line);
      } else {
        collapsed.add(line);
      }
    }

    if (!changed) return value;
    return { ranges, collapsed };
  },
});

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

function setsEqual(a: Set<number>, b: Set<number>): boolean {
  if (a === b) return true;
  if (a.size !== b.size) return false;
  for (const v of a) if (!b.has(v)) return false;
  return true;
}

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
      if (FOLD_STRUCTURAL_RE.test(deleted)) { might = true; return; }
    }
    if (toB > fromB) {
      const inserted = update.state.doc.sliceString(fromB, toB);
      if (FOLD_STRUCTURAL_RE.test(inserted)) { might = true; }
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
      if (update.docChanged && editMightAffectFolds(update)) schedule();
    },
    destroy() {
      destroyed = true;
      if (scheduled) scheduled.cancel();
    },
  };
});

function buildVisibleFoldDecorations(view: EditorView): DecorationSet {
  const foldState = view.state.field(foldStateField, false);
  if (!foldState) return Decoration.none;
  return buildFoldDecorations(
    view.state.doc,
    foldState.ranges,
    foldState.collapsed,
    expandedFoldVisibleSpans(view),
  );
}

const foldDecorationsPlugin = ViewPlugin.fromClass(
  class {
    decorations: DecorationSet;

    constructor(view: EditorView) {
      this.decorations = this.safeBuild(view, Decoration.none);
    }

    update(update: ViewUpdate) {
      const foldChanged =
        update.startState.field(foldStateField, false) !==
        update.state.field(foldStateField, false);
      if (!foldChanged && !update.viewportChanged) {
        return;
      }
      this.decorations = this.safeBuild(update.view, this.decorations);
    }

    private safeBuild(view: EditorView, fallback: DecorationSet): DecorationSet {
      try {
        return buildVisibleFoldDecorations(view);
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

function applyFoldStateAtLine(view: EditorView, line: number, nextCollapsed: boolean): boolean {
  const foldState = view.state.field(foldStateField, false);
  if (!foldState || !foldState.ranges.has(line)) return false;
  const range = foldState.ranges.get(line)!;
  const currentCollapsed = foldState.collapsed.has(line);
  if (currentCollapsed === nextCollapsed) return false;
  const main = view.state.selection.main;
  const intersectsHidden = main.from < range.to && range.from < main.to;
  const headInsideHidden = main.head >= range.from && main.head < range.to;
  const effects = [toggleFoldAtLineEffect.of(line)];

  if (nextCollapsed && (headInsideHidden || intersectsHidden)) {
    const anchor = view.state.doc.line(line).to;
    view.dispatch({
      effects,
      selection: { anchor },
      scrollIntoView: true,
    });
    return true;
  }

  view.dispatch({ effects });
  return true;
}

function toggleFoldAtLine(view: EditorView, line: number): boolean {
  const foldState = view.state.field(foldStateField, false);
  if (!foldState || !foldState.ranges.has(line)) return false;
  return applyFoldStateAtLine(view, line, !foldState.collapsed.has(line));
}

function formatFoldMessage(action: "folded" | "unfolded", range: FoldRange): string {
  const hiddenLineCount = range.endLine - range.startLine;
  const hiddenSuffix = hiddenLineCount === 1 ? "" : "s";
  const kind = range.kind === "heading" ? "heading" : "code block";
  return `fold: ${action} ${kind} (${hiddenLineCount} line${hiddenSuffix})`;
}

export function toggleFoldAtCursor(view: EditorView): boolean {
  const foldState = view.state.field(foldStateField, false);
  if (!foldState) return false;
  const line = view.state.doc.lineAt(view.state.selection.main.head).number;
  const foldStart = findFoldStartForLine(foldState.ranges, line);
  if (foldStart === null) return false;
  return toggleFoldAtLine(view, foldStart);
}

export function executeFoldCommand(view: EditorView, action: FoldCommandAction): FoldCommandResult {
  const foldState = view.state.field(foldStateField, false);
  if (!foldState) {
    return { changed: false, message: "fold: unavailable" };
  }

  const line = view.state.doc.lineAt(view.state.selection.main.head).number;
  const foldStart = findFoldStartForLine(foldState.ranges, line);
  if (foldStart === null) {
    return { changed: false, message: "fold: no foldable block at cursor" };
  }

  const range = foldState.ranges.get(foldStart);
  if (!range) {
    return { changed: false, message: "fold: no foldable block at cursor" };
  }
  const isCollapsed = foldState.collapsed.has(foldStart);

  if (action === "fold" && isCollapsed) {
    return { changed: false, message: "fold: already folded" };
  }
  if (action === "unfold" && !isCollapsed) {
    return { changed: false, message: "fold: already unfolded" };
  }

  const nextCollapsed = action === "fold-toggle" ? !isCollapsed : action === "fold";
  const changed = applyFoldStateAtLine(view, foldStart, nextCollapsed);
  if (!changed) {
    return { changed: false, message: "fold: no foldable block at cursor" };
  }

  return {
    changed: true,
    message: formatFoldMessage(nextCollapsed ? "folded" : "unfolded", range),
  };
}

const foldMouseHandlers = EditorView.domEventHandlers({
  mousedown: (event) => {
    if (event.button !== 0) return false;
    const rawTarget = event.target;
    const target = rawTarget instanceof Element
      ? rawTarget
      : rawTarget instanceof Node
      ? rawTarget.parentElement
      : null;
    if (!target) return false;
    const foldTarget = target.closest(".cm-fold-placeholder, .cm-fold-toggle") as HTMLElement | null;
    if (!foldTarget) return false;
    event.preventDefault();
    event.stopPropagation();
    return true;
  },
  click: (event, view) => {
    if (event.button !== 0) return false;
    const rawTarget = event.target;
    const target = rawTarget instanceof Element
      ? rawTarget
      : rawTarget instanceof Node
      ? rawTarget.parentElement
      : null;
    if (!target) return false;
    const foldTarget = target.closest(".cm-fold-placeholder, .cm-fold-toggle") as HTMLElement | null;
    if (!foldTarget) return false;
    const lineRaw = foldTarget.dataset.foldLine;
    const line = Number(lineRaw);
    if (!Number.isFinite(line) || line <= 0) return false;
    event.preventDefault();
    event.stopPropagation();
    return toggleFoldAtLine(view, line);
  },
});

export function foldingExtensions() {
  return [
    foldStateField,
    foldAnalyzerPlugin,
    foldDecorationsPlugin,
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
