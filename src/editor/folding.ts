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

export interface FoldRangeDescriptor {
  startLine: number;
  endLine: number;
  kind: FoldKind;
}

const toggleFoldAtLineEffect = StateEffect.define<number>();

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

  const lines: string[] = [];
  for (let lineNo = 1; lineNo <= lineCount; lineNo++) {
    lines.push(doc.line(lineNo).text);
  }
  const analyzed = markdownAnalyzeLines(lines, {
    inCodeBlock: false,
    codeFenceLang: null,
  }).lines;

  for (let lineNo = 1; lineNo <= lineCount; lineNo++) {
    const current = analyzed[lineNo - 1];
    if (!current) continue;
    const level = current.info.headingLevel;
    if (level === null || current.inCodeBlock) continue;

    let endLine = lineCount;
    for (let nextLine = lineNo + 1; nextLine <= lineCount; nextLine++) {
      const next = analyzed[nextLine - 1];
      if (!next || next.inCodeBlock) continue;
      const nextLevel = next.info.headingLevel;
      if (nextLevel === level) {
        endLine = nextLine - 1;
        break;
      }
    }

    if (endLine <= lineNo) continue;
    const from = doc.line(lineNo + 1).from;
    const to = endLine < lineCount ? doc.line(endLine + 1).from : doc.line(endLine).to;
    if (from >= to) continue;
    ranges.set(lineNo, {
      startLine: lineNo,
      endLine,
      from,
      to,
      kind: "heading",
    });
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
  if (collapsed.size === 0) return Decoration.none;
  const builder = new RangeSetBuilder<Decoration>();
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
      decorations: Decoration.none,
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

function toggleFoldAtLine(view: EditorView, line: number): boolean {
  const foldState = view.state.field(foldStateField, false);
  if (!foldState || !foldState.ranges.has(line)) return false;
  const range = foldState.ranges.get(line)!;
  const main = view.state.selection.main;
  const intersectsHidden = main.from < range.to && range.from < main.to;
  const headInsideHidden = main.head >= range.from && main.head < range.to;
  const nextCollapsed = !foldState.collapsed.has(line);
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

export function toggleFoldAtCursor(view: EditorView): boolean {
  const foldState = view.state.field(foldStateField, false);
  if (!foldState) return false;
  const line = view.state.doc.lineAt(view.state.selection.main.head).number;
  const foldStart = findFoldStartForLine(foldState.ranges, line);
  if (foldStart === null) return false;
  return toggleFoldAtLine(view, foldStart);
}

const foldMouseHandlers = EditorView.domEventHandlers({
  mousedown: (event, view) => {
    const target = event.target as Element | null;
    if (!target) return false;
    const placeholder = target.closest(".cm-fold-placeholder") as HTMLElement | null;
    if (!placeholder) return false;
    const lineRaw = placeholder.dataset.foldLine;
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
