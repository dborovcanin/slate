import {
  RangeSetBuilder,
  StateEffect,
  StateField,
  Text,
} from "@codemirror/state";
import {
  Decoration,
  type DecorationSet,
  EditorView,
  WidgetType,
  keymap,
} from "@codemirror/view";
import { markdownAnalyzeLines } from "./wasm.ts";

type FoldKind = "heading" | "fence";
const MAX_FOLD_ANALYSIS_LINES = 20_000;

interface FoldRange {
  startLine: number;
  endLine: number;
  from: number;
  to: number;
  kind: FoldKind;
}

interface FoldStateValue {
  ranges: Map<number, FoldRange>;
  collapsed: Set<number>;
  decorations: DecorationSet;
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
    span.textContent = this.collapsed ? "▸" : "▾";
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
    span.textContent = `⯈ ${this.hiddenLineCount} line${this.hiddenLineCount === 1 ? "" : "s"} folded`;
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
      const from = doc.line(lineNo + 1).from;
      const to = endLine < lineCount ? doc.line(endLine + 1).from : doc.line(endLine).to;
      if (from < to) {
        ranges.set(lineNo, {
          startLine: lineNo,
          endLine,
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
        const from = doc.line(startLine + 1).from;
        const to = endLine < lineCount ? doc.line(endLine + 1).from : doc.line(endLine).to;
        if (from < to) {
          ranges.set(startLine, {
            startLine,
            endLine,
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
    const from = doc.line(openFenceLine + 1).from;
    const to = doc.line(lineCount).to;
    if (from < to) {
      ranges.set(openFenceLine, {
        startLine: openFenceLine,
        endLine: lineCount,
        from,
        to,
        kind: "fence",
      });
    }
  }

  return ranges;
}

function buildFoldDecorations(
  doc: Text,
  ranges: Map<number, FoldRange>,
  collapsed: Set<number>,
): DecorationSet {
  if (ranges.size === 0) return Decoration.none;
  const builder = new RangeSetBuilder<Decoration>();
  const allRanges = [...ranges.values()].sort((a, b) =>
    a.startLine - b.startLine || a.endLine - b.endLine
  );

  for (const range of allRanges) {
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
    const startLine = doc.line(range.startLine);
    builder.add(startLine.from, startLine.from, Decoration.line({ class: "cm-folded-start-line" }));
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
    return {
      ranges,
      collapsed,
      decorations: buildFoldDecorations(state.doc, ranges, collapsed),
    };
  },
  update(value, tr) {
    let ranges = value.ranges;
    let collapsed = value.collapsed;
    let changed = false;

    if (tr.docChanged) {
      ranges = buildFoldRanges(tr.state.doc);
      collapsed = normalizeCollapsed(collapsed, ranges);
      changed = true;
    }

    for (const effect of tr.effects) {
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
    return {
      ranges,
      collapsed,
      decorations: buildFoldDecorations(tr.state.doc, ranges, collapsed),
    };
  },
  provide: (field) => EditorView.decorations.from(field, (value) => value.decorations),
});

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
  mousedown: (event, view) => {
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
