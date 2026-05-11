import {
  EditorView,
  Decoration,
  type DecorationSet,
  ViewPlugin,
  ViewUpdate,
  WidgetType,
  keymap,
} from "@codemirror/view";
import {
  StateField,
  StateEffect,
  RangeSetBuilder,
  RangeSet,
  RangeValue,
  Annotation,
  type ChangeDesc,
  type EditorState,
  type Text,
} from "@codemirror/state";
import {
  evaluateNoteContext,
  evaluateNoteContextDelta,
  syncNoteLines,
  type TableCellEvaluation,
  type VariableIndexEntry,
} from "../api.ts";
import {
  builtinFormulaLabels,
  findCalcSegment,
  findSingleCalcTableCell,
  lineForCalcEvaluation,
} from "./calc-line-utils.ts";
import { calcFindTableFormulaSegments, type TableFormulaSegment } from "./wasm.ts";
import { planIncrementalCalc } from "./calc-incremental.ts";
import {
  calcBuiltinFormulaLabel,
  calcDecideEvalWindow,
  calcComputeRefresh,
  calcContainsBuiltinFormula,
  calcContainsVariableAssignment,
  calcFormatFormulaDisplayValue,
  calcLineUsesAssignmentPrefix,
  calcShouldScheduleEval,
} from "./wasm.ts";
import {
  editorProfilerNowMs,
  isEditorProfilerEnabled,
  recordEditorProfilerSample,
} from "../perf/editor-profiler.ts";

export interface CalcExtensionOptions {
  variablesEnabled?: boolean;
  tableEnabled?: boolean;
  getActiveNoteId?: () => string | null;
}

const MAX_CALC_EVAL_LINES = 200_000;
const MAX_CHANGED_LINES_SCAN_FOR_CALC_RELEVANCE = 512;
const MAX_VISIBLE_LINES_SCAN_FOR_CALC_RELEVANCE = 2_000;
const CALC_EVAL_DELAY_MS = 150;
const CALC_TABLE_EDIT_EVAL_DELAY_MS = 24;

// Pure cache: lineText → TableFormulaSegment[]. Results are deterministic so
// no invalidation is needed — a changed line produces a different key.
const FORMULA_SEGMENT_CACHE = new Map<string, TableFormulaSegment[]>();
const FORMULA_SEGMENT_CACHE_MAX = 512;
const tableRowRe = /^\s*\|.*\|\s*$/;

function cachedCalcFindTableFormulaSegments(lineText: string): TableFormulaSegment[] {
  const hit = FORMULA_SEGMENT_CACHE.get(lineText);
  if (hit !== undefined) return hit;
  const result = calcFindTableFormulaSegments(lineText);
  if (FORMULA_SEGMENT_CACHE.size >= FORMULA_SEGMENT_CACHE_MAX) {
    FORMULA_SEGMENT_CACHE.delete(FORMULA_SEGMENT_CACHE.keys().next().value!);
  }
  FORMULA_SEGMENT_CACHE.set(lineText, result);
  return result;
}

// Effect to update calc results from backend
const setCalcResults = StateEffect.define<Map<number, string>>();
const setCellCalcResults = StateEffect.define<Map<number, TableCellEvaluation[]>>();
const setVariableIndex = StateEffect.define<VariableIndexEntry[]>();

// Marks transactions originating from the refresh pass so the plugin does not
// re-schedule an eval in response to its own trailer rewrites.
const calcRefreshAnnotation = Annotation.define<boolean>();

// Effects managing the set of "committed trailer" markers. A marker means
// "the user Tab-committed a ` = <literal>` trailer on this line and we should
// keep it in sync with the backend result." The literal we last wrote travels
// with the marker so the ground truth is robust to line-index shifts caused
// by upstream edits.
const addCommitMark = StateEffect.define<{ pos: number; literal: string }>();
const removeCommitMarks = StateEffect.define<readonly number[]>();

class CommitMark extends RangeValue {
  readonly literal: string;
  constructor(literal: string) {
    super();
    this.literal = literal;
  }
  override eq(other: RangeValue): boolean {
    return other instanceof CommitMark && other.literal === this.literal;
  }
}

const commitMarksField = StateField.define<RangeSet<CommitMark>>({
  create() {
    return RangeSet.empty;
  },
  update(value, tr) {
    // Filter (prune) first, using pre-change positions from the effect payload;
    // then map through doc changes; then apply add effects, whose positions are
    // expressed in the post-change coordinate system of the same transaction.
    let next = value;
    for (const e of tr.effects) {
      if (e.is(removeCommitMarks) && e.value.length > 0) {
        const toRemove = new Set(e.value);
        next = next.update({ filter: (from) => !toRemove.has(from) });
      }
    }
    next = next.map(tr.changes);
    for (const e of tr.effects) {
      if (e.is(addCommitMark)) {
        next = next.update({
          add: [new CommitMark(e.value.literal).range(e.value.pos)],
          sort: true,
        });
      }
    }
    return next;
  },
});

// State field holding current calc results keyed by line number (0-based)
const calcResultsField = StateField.define<Map<number, string>>({
  create() {
    return new Map();
  },
  update(value, tr) {
    for (const e of tr.effects) {
      if (e.is(setCalcResults)) return e.value;
    }
    if (!tr.docChanged) return value;
    return remapCalcResultsForDocChange(value, tr.startState.doc, tr.changes, tr.newDoc);
  },
});

// Per-cell results for table rows containing one or more `:=...` formula cells.
// Parallel to `calcResultsField`. `calcResultsField` continues to hold the
// first formula's value for backward compatibility (committed-trailer refresh,
// non-table calc ghost), while this field carries every formula cell in the
// row in left-to-right order.
const cellCalcResultsField = StateField.define<Map<number, TableCellEvaluation[]>>({
  create() {
    return new Map();
  },
  update(value, tr) {
    for (const e of tr.effects) {
      if (e.is(setCellCalcResults)) return e.value;
    }
    if (!tr.docChanged) return value;
    // Re-key by tracking line index drift through the change set.
    return remapPerLineMapForDocChange(
      value,
      tr.startState.doc,
      tr.changes,
      tr.newDoc,
    );
  },
});

function remapPerLineMapForDocChange<T>(
  value: Map<number, T>,
  oldDoc: Text,
  changes: ChangeDesc,
  newDoc: Text,
): Map<number, T> {
  if (value.size === 0) return value;
  const next = new Map<number, T>();
  for (const [lineIdx, entry] of value) {
    const lineNumber = lineIdx + 1;
    if (lineNumber < 1 || lineNumber > oldDoc.lines) continue;
    const oldLine = oldDoc.line(lineNumber);
    const mappedFrom = changes.mapPos(oldLine.from, 1);
    if (mappedFrom < 0 || mappedFrom > newDoc.length) continue;
    const newLine = newDoc.lineAt(mappedFrom);
    next.set(newLine.number - 1, entry);
  }
  return next;
}

export const variableIndexField = StateField.define<VariableIndexEntry[]>({
  create() {
    return [];
  },
  update(value, tr) {
    for (const e of tr.effects) {
      if (e.is(setVariableIndex)) return e.value;
    }
    if (!tr.docChanged) return value;
    return remapVariableIndexForDocChange(value, tr.startState.doc, tr.changes, tr.newDoc);
  },
});

class CalcResultWidget extends WidgetType {
  readonly result: string;
  readonly prefix: string;

  constructor(result: string, prefix: string) {
    super();
    this.result = result;
    this.prefix = prefix;
  }

  toDOM(): HTMLElement {
    const span = document.createElement("span");
    span.className = "calc-ghost";
    span.textContent = `${this.prefix}${this.result}`;
    return span;
  }

  eq(other: CalcResultWidget): boolean {
    return this.result === other.result && this.prefix === other.prefix;
  }
}

class FormulaCellWidget extends WidgetType {
  readonly value: string;
  readonly marker: string;
  readonly minWidthCh: number;

  constructor(value: string, marker: string, minWidthCh: number) {
    super();
    this.value = value;
    this.marker = marker;
    this.minWidthCh = minWidthCh;
  }

  toDOM(): HTMLElement {
    const span = document.createElement("span");
    span.className = "calc-formula-inline";
    span.style.minWidth = `${this.minWidthCh}ch`;

    const value = document.createElement("span");
    value.className = "calc-formula-value";
    value.textContent = this.value;
    span.append(value);

    const marker = document.createElement("span");
    marker.className = "calc-formula-marker";
    marker.textContent = this.marker;
    span.append(marker);

    return span;
  }

  eq(other: FormulaCellWidget): boolean {
    return (
      this.value === other.value &&
      this.marker === other.marker &&
      this.minWidthCh === other.minWidthCh
    );
  }

  override ignoreEvent(): boolean {
    // Allow pointer events so clicking a masked formula cell can place the
    // caret and reveal the source expression.
    return false;
  }
}

export function builtinFormulaExplanation(expression: string): string | null {
  return calcBuiltinFormulaLabel(expression);
}

export function formatFormulaDisplayValue(raw: string): string {
  return calcFormatFormulaDisplayValue(raw);
}

function selectionTouchesSegment(
  selection: { from: number; to: number },
  lineFrom: number,
  fromCol: number,
  toCol: number,
): boolean {
  const from = lineFrom + fromCol;
  const to = lineFrom + toCol;
  return selection.from <= to && selection.to >= from;
}

interface TableCellBounds {
  fromCol: number;
  toCol: number;
}

function tableCellBoundsForSegment(
  lineText: string,
  segment: { fromCol: number; toCol: number },
): TableCellBounds | null {
  const leftPipe = lineText.lastIndexOf("|", Math.max(0, segment.fromCol - 1));
  const rightPipe = lineText.indexOf("|", segment.toCol);
  if (leftPipe < 0 || rightPipe < 0 || rightPipe <= leftPipe + 1) return null;
  return { fromCol: leftPipe + 1, toCol: rightPipe };
}

const FORMULA_GHOST_MARKER = "*";

function formulaMarkerToken(index: number): string {
  return FORMULA_GHOST_MARKER.repeat(index + 1);
}

function formulaMarkerSuffix(count: number): string {
  const parts: string[] = [];
  for (let i = 0; i < count; i++) {
    parts.push(formulaMarkerToken(i));
  }
  return parts.join(" ");
}

function formulaGhostExplanation(labels: readonly string[]): string {
  return labels
    .map((label, index) => `${formulaMarkerToken(index)} \u279c ${label}`)
    .join("  ");
}

function segmentFormulaSource(lineText: string, segment: TableFormulaSegment): string {
  return lineText.slice(segment.fromChar, segment.toChar).trim();
}

const focusedPipeMark = Decoration.mark({ class: "cm-table-pipe-focused" });

const CALC_VIEWPORT_MARGIN_LINES = 80;

interface CalcVisibleLineSpan {
  fromLine: number;
  toLine: number;
}

function mergeCalcLineSpans(spans: readonly CalcVisibleLineSpan[]): CalcVisibleLineSpan[] {
  if (spans.length <= 1) return [...spans];
  const sorted = [...spans].sort((a, b) => a.fromLine - b.fromLine || a.toLine - b.toLine);
  const merged: CalcVisibleLineSpan[] = [];
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

let _cachedVisibleRanges: EditorView["visibleRanges"] | null = null;
let _cachedVisibleSpans: CalcVisibleLineSpan[] = [];

function expandedCalcVisibleSpans(view: EditorView): CalcVisibleLineSpan[] {
  if (view.visibleRanges === _cachedVisibleRanges) return _cachedVisibleSpans;
  _cachedVisibleRanges = view.visibleRanges;
  if (view.visibleRanges.length === 0) {
    _cachedVisibleSpans = [];
    return _cachedVisibleSpans;
  }
  const doc = view.state.doc;
  const spans = view.visibleRanges.map(({ from, to }) => ({
    fromLine: Math.max(1, doc.lineAt(from).number - CALC_VIEWPORT_MARGIN_LINES),
    toLine: Math.min(doc.lines, doc.lineAt(to).number + CALC_VIEWPORT_MARGIN_LINES),
  }));
  _cachedVisibleSpans = mergeCalcLineSpans(spans);
  return _cachedVisibleSpans;
}

function calcLineInSpans(lineNumber: number, spans: readonly CalcVisibleLineSpan[]): boolean {
  for (const span of spans) {
    if (lineNumber < span.fromLine) return false;
    if (lineNumber <= span.toLine) return true;
  }
  return false;
}

function buildCalcDecorationsForSpans(
  state: EditorState,
  spans: readonly CalcVisibleLineSpan[],
): DecorationSet {
  if (spans.length === 0) return Decoration.none;
  const results = state.field(calcResultsField);
  const cellResults = state.field(cellCalcResultsField);
  // Range additions must be sorted by from-position. Collect them in a
  // throwaway list then add to the builder in document order at the end.
  const items: { from: number; to: number; deco: Decoration }[] = [];
  const selection = state.selection.main;

  for (const span of spans) {
    const fromLine = Math.max(1, span.fromLine);
    const toLine = Math.min(state.doc.lines, span.toLine);
    for (let lineNumber = fromLine; lineNumber <= toLine; lineNumber++) {
      const lineIndex = lineNumber - 1;
      const line = state.doc.line(lineNumber); // 1-based
      const result = results.get(lineIndex);
      const cellsForLine = cellResults.get(lineIndex);
      if (result == null && (!cellsForLine || cellsForLine.length === 0)) continue;

      const segments = cachedCalcFindTableFormulaSegments(line.text);
      if (segments.length > 0) {
        const cells = cellsForLine ?? [];
        const valueForCell = (
          cellIndex: number,
        ): { value: string; hasError: boolean } | null => {
          const hit = cells.find((c) => c.cell_index === cellIndex);
          if (!hit) return null;
          return {
            value: formatFormulaDisplayValue(hit.value),
            hasError: hit.error_kind != null,
          };
        };

        const trailerParts: string[] = [];
        segments.forEach((seg, fi) => {
          const marker = formulaMarkerToken(fi);
          const computed = valueForCell(seg.cellIndex);
          const formulaSource = segmentFormulaSource(line.text, seg);
          const editingCell = selectionTouchesSegment(
            selection,
            line.from,
            seg.cellLeftPipeChar + 1,
            seg.cellRightPipeChar,
          );

          if (computed && !computed.hasError && !editingCell) {
            const minWidthCh = Math.max(
              1,
              seg.toChar - seg.fromChar,
              computed.value.length + marker.length,
            );
            items.push({
              from: line.from + seg.fromChar,
              to: line.from + seg.toChar,
              deco: Decoration.replace({
                widget: new FormulaCellWidget(computed.value, marker, minWidthCh),
              }),
            });
            trailerParts.push(`${marker} \u279c ${formulaSource}`);
            return;
          }

          if (computed && !computed.hasError && editingCell) {
            trailerParts.push(`${marker} \u279c ${computed.value}`);
            return;
          }

          if (formulaSource.length > 0) {
            trailerParts.push(`${marker} \u279c ${formulaSource}`);
          }
        });

        if (trailerParts.length > 0) {
          items.push({
            from: line.to,
            to: line.to,
            deco: Decoration.widget({
              widget: new CalcResultWidget(trailerParts.join("  "), " "),
              side: 1,
            }),
          });
        }

        continue;
      }

      // Non-formula line: legacy single calc-ghost trailer.
      if (result == null) continue;
      const cell = findSingleCalcTableCell(line.text);
      const labels = cell ? builtinFormulaLabels(cell.expr) : [];
      if (cell && labels.length > 0) {
        const revealBounds = tableCellBoundsForSegment(line.text, cell) ?? {
          fromCol: cell.fromCol,
          toCol: cell.toCol,
        };
        const editingCell = selectionTouchesSegment(
          selection,
          line.from,
          revealBounds.fromCol,
          revealBounds.toCol,
        );
        if (editingCell) continue;

        const formatted = formatFormulaDisplayValue(result);
        const marker = formulaMarkerSuffix(labels.length);
        const minWidthCh = Math.max(
          1,
          cell.toCol - cell.fromCol,
          formatted.length + marker.length,
        );

        items.push({
          from: line.from + cell.fromCol,
          to: line.from + cell.toCol,
          deco: Decoration.replace({
            widget: new FormulaCellWidget(formatted, marker, minWidthCh),
          }),
        });
        items.push({
          from: line.to,
          to: line.to,
          deco: Decoration.widget({
            widget: new CalcResultWidget(formulaGhostExplanation(labels), " "),
            side: 1,
          }),
        });
        continue;
      }
      const prefix = lineUsesAssignmentGhostPrefix(line.text) ? " = " : " \u2192 ";
      items.push({
        from: line.to,
        to: line.to,
        deco: Decoration.widget({
          widget: new CalcResultWidget(result, prefix),
          side: 1,
        }),
      });
    }
  }

  // Highlight the focused table cell's pipe characters.
  const cursor = selection.head;
  if (cursor >= 0 && cursor <= state.doc.length) {
    const cursorLine = state.doc.lineAt(cursor);
    if (calcLineInSpans(cursorLine.number, spans)) {
      const text = cursorLine.text;
      const trimmed = text.trim();
      if (trimmed.startsWith("|") && trimmed.endsWith("|")) {
        const col = cursor - cursorLine.from;
        const leftPipe = text.lastIndexOf("|", Math.max(0, col - 1));
        const rightPipe = text.indexOf("|", Math.max(col, leftPipe + 1));
        if (leftPipe >= 0 && rightPipe > leftPipe) {
          items.push({
            from: cursorLine.from + leftPipe,
            to: cursorLine.from + leftPipe + 1,
            deco: focusedPipeMark,
          });
          items.push({
            from: cursorLine.from + rightPipe,
            to: cursorLine.from + rightPipe + 1,
            deco: focusedPipeMark,
          });
        }
      }
    }
  }

  items.sort((a, b) => a.from - b.from || a.to - b.to);
  const builder = new RangeSetBuilder<Decoration>();
  for (const item of items) {
    builder.add(item.from, item.to, item.deco);
  }
  return builder.finish();
}

function buildCalcDecorations(
  view: EditorView,
): DecorationSet {
  return buildCalcDecorationsForSpans(
    view.state,
    expandedCalcVisibleSpans(view),
  );
}

interface CalcDecorationBuildMetrics extends Record<string, number> {
  spanCount: number;
  lineCount: number;
  totalChars: number;
  maxLineLength: number;
}

function calcDecorationBuildMetrics(view: EditorView): CalcDecorationBuildMetrics {
  const doc = view.state.doc;
  const spans = expandedCalcVisibleSpans(view);
  let lineCount = 0;
  let totalChars = 0;
  let maxLineLength = 0;

  for (const span of spans) {
    const fromLine = Math.max(1, span.fromLine);
    const toLine = Math.min(doc.lines, span.toLine);
    if (fromLine > toLine) continue;
    for (let lineNo = fromLine; lineNo <= toLine; lineNo++) {
      const text = doc.line(lineNo).text;
      lineCount += 1;
      totalChars += text.length;
      if (text.length > maxLineLength) maxLineLength = text.length;
    }
  }

  return {
    spanCount: spans.length,
    lineCount,
    totalChars,
    maxLineLength,
  };
}

function calcDecorationRebuildReason(
  update: ViewUpdate,
  resultsChanged: boolean,
  cellsChanged: boolean,
): string {
  const reasons: string[] = [];
  if (resultsChanged) reasons.push("resultsChanged");
  if (cellsChanged) reasons.push("cellsChanged");
  if (update.docChanged) reasons.push("docChanged");
  if (update.selectionSet) reasons.push("selectionSet");
  if (update.viewportChanged) reasons.push("viewportChanged");
  return reasons.length > 0 ? reasons.join("+") : "unspecified";
}

function calcDecorationsPlugin() {
  return ViewPlugin.fromClass(
    class {
      decorations: DecorationSet;

      constructor(view: EditorView) {
        this.decorations = this.safeBuild(view, Decoration.none, "init");
      }

      update(update: ViewUpdate) {
        const resultsChanged =
          update.startState.field(calcResultsField) !== update.state.field(calcResultsField);
        const cellsChanged =
          update.startState.field(cellCalcResultsField) !== update.state.field(cellCalcResultsField);

        if (
          !resultsChanged &&
          !cellsChanged &&
          !update.docChanged &&
          !update.selectionSet &&
          !update.viewportChanged
        ) {
          return;
        }

        this.decorations = this.safeBuild(
          update.view,
          this.decorations,
          calcDecorationRebuildReason(update, resultsChanged, cellsChanged),
        );
      }

      private safeBuild(
        view: EditorView,
        fallback: DecorationSet,
        reason: string,
      ): DecorationSet {
        const profilingEnabled = isEditorProfilerEnabled();
        const startedAt = profilingEnabled ? editorProfilerNowMs() : 0;
        const metrics = profilingEnabled ? calcDecorationBuildMetrics(view) : null;
        try {
          const next = buildCalcDecorations(view);
          if (profilingEnabled) {
            recordEditorProfilerSample(
              "calc.decorations.safeBuild",
              editorProfilerNowMs() - startedAt,
              {
                reason,
                metrics,
              },
            );
          }
          return next;
        } catch (error) {
          console.error("Calc decoration build failed:", error);
          if (profilingEnabled) {
            recordEditorProfilerSample(
              "calc.decorations.safeBuild",
              editorProfilerNowMs() - startedAt,
              { reason: `${reason}_error` },
            );
          }
          return fallback;
        }
      }
    },
    {
      decorations: (plugin) => plugin.decorations,
    },
  );
}

export function lineUsesAssignmentGhostPrefix(lineText: string): boolean {
  return calcLineUsesAssignmentPrefix(lineText);
}

export function containsVariableAssignment(lines: readonly string[]): boolean {
  return calcContainsVariableAssignment(lines);
}

export function containsBuiltinFormula(lines: readonly string[]): boolean {
  return calcContainsBuiltinFormula(lines);
}

function lineHasBuiltinFormula(lineText: string): boolean {
  const tableSegments = cachedCalcFindTableFormulaSegments(lineText);
  if (tableSegments.length > 0) {
    for (const segment of tableSegments) {
      const expr = lineText.slice(segment.fromChar, segment.toChar).trim();
      if (builtinFormulaLabels(expr).length > 0) return true;
    }
    return false;
  }

  const segment = findCalcSegment(lineText);
  if (!segment) return false;
  return builtinFormulaLabels(segment.expr).length > 0;
}

function lineHasCalcGlobalSyntax(lineText: string): boolean {
  return lineHasBuiltinFormula(lineText) || lineUsesAssignmentGhostPrefix(lineText);
}

function scanDocHasCalcGlobalSyntax(doc: Text): boolean {
  for (let i = 1; i <= doc.lines; i++) {
    if (lineHasCalcGlobalSyntax(doc.line(i).text)) return true;
  }
  return false;
}

function rangeTouchesCalcGlobalSyntax(doc: Text, from: number, to: number): boolean {
  if (doc.length === 0) return false;
  const clampedFrom = Math.min(from, doc.length);
  const clampedTo = Math.min(to, doc.length);
  const startLine = doc.lineAt(clampedFrom).number;
  const endPos = clampedTo > clampedFrom ? clampedTo - 1 : clampedFrom;
  const endLine = doc.lineAt(Math.max(clampedFrom, endPos)).number;
  if (endLine - startLine > MAX_CHANGED_LINES_SCAN_FOR_CALC_RELEVANCE) return true;
  for (let lineNo = startLine; lineNo <= endLine; lineNo++) {
    if (lineHasCalcGlobalSyntax(doc.line(lineNo).text)) return true;
  }
  return false;
}

function updateTouchesCalcGlobalSyntax(update: ViewUpdate): boolean {
  let touches = false;
  update.changes.iterChangedRanges((fromA, toA, fromB, toB) => {
    if (touches) return;
    if (rangeTouchesCalcGlobalSyntax(update.startState.doc, fromA, toA)) {
      touches = true;
      return;
    }
    if (rangeTouchesCalcGlobalSyntax(update.state.doc, fromB, toB)) {
      touches = true;
    }
  });
  return touches;
}

function lineHasCalcExpression(lineText: string): boolean {
  return lineForCalcEvaluation(lineText).trim().length > 0;
}

function visibleSpansTouchCalcSyntax(view: EditorView): boolean {
  const doc = view.state.doc;
  const spans = expandedCalcVisibleSpans(view);
  let scannedLines = 0;

  for (const span of spans) {
    const fromLine = Math.max(1, span.fromLine);
    const toLine = Math.min(doc.lines, span.toLine);
    scannedLines += toLine - fromLine + 1;
    if (scannedLines > MAX_VISIBLE_LINES_SCAN_FOR_CALC_RELEVANCE) return true;

    for (let lineNo = fromLine; lineNo <= toLine; lineNo++) {
      const lineText = doc.line(lineNo).text;
      if (lineHasCalcExpression(lineText) || calcLineUsesAssignmentPrefix(lineText)) {
        return true;
      }
    }
  }

  return false;
}

function rangeTouchesCalcExpression(doc: Text, from: number, to: number): boolean {
  const startLine = doc.lineAt(from).number;
  const endPos = to > from ? to - 1 : from;
  const endLine = doc.lineAt(Math.max(from, endPos)).number;
  if (endLine - startLine > MAX_CHANGED_LINES_SCAN_FOR_CALC_RELEVANCE) return true;
  for (let lineNo = startLine; lineNo <= endLine; lineNo++) {
    if (lineHasCalcExpression(doc.line(lineNo).text)) return true;
  }
  return false;
}

function updateTouchesCalcExpression(update: ViewUpdate): boolean {
  let touches = false;
  update.changes.iterChangedRanges((fromA, toA, fromB, toB) => {
    if (touches) return;
    if (rangeTouchesCalcExpression(update.startState.doc, fromA, toA)) {
      touches = true;
      return;
    }
    if (rangeTouchesCalcExpression(update.state.doc, fromB, toB)) {
      touches = true;
    }
  });
  return touches;
}

function rangeTouchesTableRow(doc: Text, from: number, to: number): boolean {
  const startLine = doc.lineAt(from).number;
  const endPos = to > from ? to - 1 : from;
  const endLine = doc.lineAt(Math.max(from, endPos)).number;
  if (endLine - startLine > MAX_CHANGED_LINES_SCAN_FOR_CALC_RELEVANCE) return true;
  for (let lineNo = startLine; lineNo <= endLine; lineNo++) {
    if (tableRowRe.test(doc.line(lineNo).text)) return true;
  }
  return false;
}

function updateTouchesTableRows(update: ViewUpdate): boolean {
  let touches = false;
  update.changes.iterChangedRanges((fromA, toA, fromB, toB) => {
    if (touches) return;
    if (rangeTouchesTableRow(update.startState.doc, fromA, toA)) {
      touches = true;
      return;
    }
    if (rangeTouchesTableRow(update.state.doc, fromB, toB)) {
      touches = true;
    }
  });
  return touches;
}

export interface CommitMarkerLoc {
  docPos: number;
  lineIdx: number;
  offsetInLine: number;
  lastLiteral: string;
}

export interface CalcRefreshChange {
  lineIdx: number;
  from: number;
  to: number;
  insert: string;
  newLiteral: string;
}

export interface CalcRefreshPlan {
  changes: CalcRefreshChange[];
  prune: number[];
  // Lines whose committed trailer will display the result inline after
  // this transaction. The caller should drop these from the calc results
  // map so the ghost widget doesn't double up with the literal.
  syncedLines: number[];
}

export function computeCalcRefresh(
  markers: readonly CommitMarkerLoc[],
  lines: readonly string[],
  lineStarts: readonly number[],
  nextResults: ReadonlyMap<number, string>,
  selection: { from: number; to: number },
): CalcRefreshPlan {
  return calcComputeRefresh(markers, lines, lineStarts, nextResults, selection);
}

export function mergePartialCalcResults(
  baseResults: ReadonlyMap<number, string>,
  lineResults: readonly (string | null)[],
  evalFrom: number,
  evalTo: number,
): Map<number, string> {
  const next = new Map(baseResults);
  for (let i = evalFrom; i < evalTo; i++) {
    const result = lineResults[i];
    if (result != null) {
      next.set(i, result);
    } else {
      next.delete(i);
    }
  }
  return next;
}

interface ChangedRange {
  fromA: number;
  toA: number;
  fromB: number;
  toB: number;
}

function collectChangedRanges(changes: ChangeDesc): ChangedRange[] {
  const ranges: ChangedRange[] = [];
  changes.iterChangedRanges((fromA, toA, fromB, toB) => {
    ranges.push({ fromA, toA, fromB, toB });
  });
  return ranges;
}

function lineTextsForRange(doc: Text, fromLine: number, toLine: number): string[] {
  const start = Math.max(1, fromLine);
  const end = Math.min(doc.lines, toLine);
  if (start > end) return [];
  const out: string[] = [];
  for (let lineNo = start; lineNo <= end; lineNo++) {
    out.push(doc.line(lineNo).text);
  }
  return out;
}

export function patchLineArrayForDocChange(
  lines: readonly string[],
  oldDoc: Text,
  changes: ChangeDesc,
  newDoc: Text,
): string[] {
  const ranges = collectChangedRanges(changes);
  if (ranges.length === 0) return [...lines];

  const next = [...lines];
  for (const range of [...ranges].reverse()) {
    const oldFrom = clampPos(range.fromA, oldDoc.length);
    const oldTo = clampPos(range.toA, oldDoc.length);
    const newFrom = clampPos(range.fromB, newDoc.length);
    const newTo = clampPos(range.toB, newDoc.length);

    const oldStartLine = oldDoc.lineAt(oldFrom).number;
    const oldEndLine = oldDoc.lineAt(oldTo > oldFrom ? oldTo : oldFrom).number;
    const newStartLine = newDoc.lineAt(newFrom).number;
    const newEndLine = newDoc.lineAt(newTo > newFrom ? newTo : newFrom).number;

    next.splice(
      oldStartLine - 1,
      oldEndLine - oldStartLine + 1,
      ...lineTextsForRange(newDoc, newStartLine, newEndLine),
    );
  }

  // Boundary-position ambiguity in complex ChangeDesc values should fail closed
  // to the authoritative rope rather than poison the calc cache.
  return next.length === newDoc.lines ? next : newDoc.toJSON();
}

function lineOverlapsChangedRanges(
  lineFrom: number,
  lineTo: number,
  ranges: readonly ChangedRange[],
): boolean {
  return ranges.some((range) => range.fromA < lineTo && range.toA > lineFrom);
}

function changedRangeOverlappingLine(
  lineFrom: number,
  lineTo: number,
  ranges: readonly ChangedRange[],
): ChangedRange | null {
  for (const range of ranges) {
    if (range.fromA < lineTo && range.toA > lineFrom) return range;
  }
  return null;
}

function lineEvalKey(lineText: string): string | null {
  const key = lineForCalcEvaluation(lineText).trim();
  return key.length > 0 ? key : null;
}

function lineEvalKeyCached(
  lineText: string,
  cache: Map<string, string | null>,
): string | null {
  if (cache.has(lineText)) return cache.get(lineText)!;
  const key = lineEvalKey(lineText);
  cache.set(lineText, key);
  return key;
}

function clampPos(pos: number, max: number): number {
  return Math.min(Math.max(0, pos), max);
}

function clampLineIndex(index: number, lineCount: number): number {
  if (lineCount <= 0) return 0;
  return Math.min(Math.max(0, index), lineCount - 1);
}

function rangeSearchLineWindow(
  nextDoc: Text,
  range: ChangedRange | null,
): { startLine: number; endLine: number } {
  if (!range) {
    return { startLine: 1, endLine: nextDoc.lines };
  }

  const from = clampPos(range.fromB, nextDoc.length);
  const to = clampPos(range.toB, nextDoc.length);
  const startLine = nextDoc.lineAt(from).number;
  const endPos = to > from ? to - 1 : from;
  const endLine = nextDoc.lineAt(endPos).number;
  return {
    startLine: Math.max(1, startLine),
    endLine: Math.max(startLine, endLine),
  };
}

function preferredLineIndexAfterRangeRewrite(
  lineIndex: number,
  range: ChangedRange | null,
  startDoc: Text,
  nextDoc: Text,
): number {
  if (!range) return lineIndex;

  const oldStartLine = startDoc.lineAt(range.fromA).number - 1;
  const newStartLine = nextDoc.lineAt(clampPos(range.fromB, nextDoc.length)).number - 1;
  const offset = lineIndex - oldStartLine;
  return clampLineIndex(newStartLine + offset, nextDoc.lines);
}

function findClosestLineByEvalKey(
  nextDoc: Text,
  key: string,
  preferredLineIndex: number,
  range: ChangedRange | null,
  claimedLineIndexes: ReadonlySet<number>,
  evalKeyCache: Map<string, string | null>,
): number | null {
  const window = rangeSearchLineWindow(nextDoc, range);
  let closest: number | null = null;
  let bestDistance = Number.POSITIVE_INFINITY;

  for (let lineNo = window.startLine; lineNo <= window.endLine; lineNo++) {
    const lineIndex = lineNo - 1;
    if (claimedLineIndexes.has(lineIndex)) continue;
    const line = nextDoc.line(lineNo);
    if (lineEvalKeyCached(line.text, evalKeyCache) !== key) continue;

    const distance = Math.abs(lineIndex - preferredLineIndex);
    if (distance < bestDistance) {
      bestDistance = distance;
      closest = lineIndex;
      if (distance === 0) break;
    }
  }

  return closest;
}

function changesTouchCalcExpression(
  lineText: string,
  lineFrom: number,
  lineTo: number,
  ranges: readonly ChangedRange[],
): boolean {
  const segment = findCalcSegment(lineText);
  // For plain lines we cannot isolate a safe prefix; any line touch invalidates.
  if (!segment) return lineOverlapsChangedRanges(lineFrom, lineTo, ranges);

  for (const range of ranges) {
    if (!(range.fromA < lineTo && range.toA > lineFrom)) continue;

    const localStart = Math.max(range.fromA, lineFrom) - lineFrom;
    const localEnd = Math.min(range.toA, lineTo) - lineFrom;

    // Insertion (no old text replaced): only safe when it happens before calc segment.
    if (localStart === localEnd) {
      if (localStart >= segment.fromCol) return true;
      continue;
    }

    // Replacement/deletion: safe only if it is strictly before calc segment.
    if (localEnd > segment.fromCol) return true;
  }
  return false;
}

export function remapCalcResultsForDocChange(
  results: ReadonlyMap<number, string>,
  startDoc: Text,
  changes: ChangeDesc,
  nextDoc: Text,
): Map<number, string> {
  if (results.size === 0) return new Map();

  const changedRanges = collectChangedRanges(changes);
  if (changedRanges.length === 0) return new Map(results);

  const remapped = new Map<number, string>();
  const claimedLineIndexes = new Set<number>();
  const orderedEntries = [...results.entries()].sort((a, b) => a[0] - b[0]);
  // Shared cache for lineEvalKey WASM calls within this remapping pass.
  // Avoids re-crossing the WASM boundary for the same line text when
  // findClosestLineByEvalKey scans the same window for multiple results.
  const evalKeyCache = new Map<string, string | null>();

  for (const [lineIndex, result] of orderedEntries) {
    const oldLineNumber = lineIndex + 1;
    if (oldLineNumber < 1 || oldLineNumber > startDoc.lines) continue;

    const oldLine = startDoc.line(oldLineNumber);
    // Map using an anchor inside the line to avoid boundary ambiguity when
    // insertions happen exactly at line start.
    const anchor = oldLine.from + (oldLine.length > 0 ? 1 : 0);
    const mappedPos = changes.mapPos(anchor, 1);
    const clampedPos = clampPos(mappedPos, nextDoc.length);
    const newLineIndex = nextDoc.lineAt(clampedPos).number - 1;
    const lineRange = changedRangeOverlappingLine(
      oldLine.from,
      oldLine.to,
      changedRanges,
    );

    // If edits touched the expression region, try to preserve line identity
    // by matching equivalent eval text in the rewritten region.
    if (changesTouchCalcExpression(oldLine.text, oldLine.from, oldLine.to, changedRanges)) {
      const evalKey = lineEvalKey(oldLine.text);
      if (!evalKey) continue;

      const preferredLineIndex = preferredLineIndexAfterRangeRewrite(
        lineIndex,
        lineRange,
        startDoc,
        nextDoc,
      );
      const rescuedLineIndex = findClosestLineByEvalKey(
        nextDoc,
        evalKey,
        preferredLineIndex,
        lineRange,
        claimedLineIndexes,
        evalKeyCache,
      );
      if (rescuedLineIndex === null) continue;
      claimedLineIndexes.add(rescuedLineIndex);
      remapped.set(rescuedLineIndex, result);
      continue;
    }

    if (claimedLineIndexes.has(newLineIndex)) {
      // Keep the map one-to-one even under complex rewrites.
      const evalKey = lineEvalKey(oldLine.text);
      if (!evalKey) continue;
      const rescuedLineIndex = findClosestLineByEvalKey(
        nextDoc,
        evalKey,
        newLineIndex,
        lineRange,
        claimedLineIndexes,
        evalKeyCache,
      );
      if (rescuedLineIndex === null) continue;
      claimedLineIndexes.add(rescuedLineIndex);
      remapped.set(rescuedLineIndex, result);
      continue;
    }

    claimedLineIndexes.add(newLineIndex);
    remapped.set(newLineIndex, result);
  }
  return remapped;
}

export function remapVariableIndexForDocChange(
  entries: readonly VariableIndexEntry[],
  startDoc: Text,
  changes: ChangeDesc,
  nextDoc: Text,
): VariableIndexEntry[] {
  if (entries.length === 0) return [];

  const next: VariableIndexEntry[] = [];
  for (const entry of entries) {
    const oldLineNumber = entry.line;
    if (oldLineNumber < 1 || oldLineNumber > startDoc.lines) continue;

    const oldLine = startDoc.line(oldLineNumber);
    const anchor = oldLine.from + (oldLine.length > 0 ? 1 : 0);
    const mappedPos = changes.mapPos(anchor, 1);
    const clampedPos = clampPos(mappedPos, nextDoc.length);
    const newLineNumber = nextDoc.lineAt(clampedPos).number;
    next.push({ ...entry, line: newLineNumber });
  }

  return next;
}

function calcResultMapsEqual(
  a: ReadonlyMap<number, string>,
  b: ReadonlyMap<number, string>,
): boolean {
  if (a.size !== b.size) return false;
  for (const [key, value] of a) {
    if (b.get(key) !== value) return false;
  }
  return true;
}

function perLineMapsEqual(
  a: ReadonlyMap<number, TableCellEvaluation[]>,
  b: ReadonlyMap<number, TableCellEvaluation[]>,
): boolean {
  if (a.size !== b.size) return false;
  for (const [key, left] of a) {
    const right = b.get(key);
    if (!right || right.length !== left.length) return false;
    for (let i = 0; i < left.length; i++) {
      if (
        left[i]!.cell_index !== right[i]!.cell_index ||
        left[i]!.value !== right[i]!.value ||
        left[i]!.error_kind !== right[i]!.error_kind
      ) {
        return false;
      }
    }
  }
  return true;
}

function variableIndexEqual(
  a: readonly VariableIndexEntry[],
  b: readonly VariableIndexEntry[],
): boolean {
  if (a.length !== b.length) return false;
  for (let i = 0; i < a.length; i++) {
    const left = a[i];
    const right = b[i];
    if (!left || !right) return false;
    if (
      left.name !== right.name ||
      left.normalized !== right.normalized ||
      left.line !== right.line
    ) {
      return false;
    }
  }
  return true;
}

function buildCalcPlugin(options: CalcExtensionOptions) {
  const variablesEnabled = options.variablesEnabled ?? true;
  const tableEnabled = options.tableEnabled ?? true;
  const getActiveNoteId = options.getActiveNoteId;

  return ViewPlugin.define((view) => {
    let timer: number | null = null;
    let inFlight = false;
    let rerunRequested = false;
    let destroyed = false;
    let deferredEval = false;
    let pendingFullTableRecalc = false;
    let prevLines: string[] = [];
    let prevResults: Map<number, string> = new Map();
    let prevVariables: VariableIndexEntry[] = [];
    let cachedLines: string[] | null = null;
    let cachedLinesDoc: Text | null = null;
    // Track whether we've seeded the server-side cache for the current note.
    let serverCacheSeeded = false;
    // null = needs (re)scan. Cached whenever scan runs.
    let cachedHasGlobalSyntax: boolean | null = null;

    function linesForDoc(doc: Text): string[] {
      if (cachedLines && cachedLinesDoc === doc) return cachedLines;
      cachedLines = doc.toJSON();
      cachedLinesDoc = doc;
      return cachedLines;
    }

    function mapLineCacheThroughUpdate(update: ViewUpdate) {
      if (!update.docChanged) return;
      if (!cachedLines || cachedLinesDoc !== update.startState.doc) {
        cachedLines = null;
        cachedLinesDoc = null;
        return;
      }
      cachedLines = patchLineArrayForDocChange(
        cachedLines,
        update.startState.doc,
        update.changes,
        update.state.doc,
      );
      cachedLinesDoc = update.state.doc;
    }

    function hasGlobalSyntax(doc: Text): boolean {
      if (cachedHasGlobalSyntax !== null) return cachedHasGlobalSyntax;
      cachedHasGlobalSyntax = scanDocHasCalcGlobalSyntax(doc);
      return cachedHasGlobalSyntax;
    }

    function clearCalcState(view: EditorView) {
      const prevCellResults = view.state.field(cellCalcResultsField);
      const prevVars = view.state.field(variableIndexField);
      const effects: StateEffect<unknown>[] = [];
      if (prevResults.size > 0) effects.push(setCalcResults.of(new Map()));
      if (prevCellResults.size > 0) effects.push(setCellCalcResults.of(new Map()));
      if (prevVars.length > 0) effects.push(setVariableIndex.of([]));
      if (effects.length > 0) view.dispatch({ effects });
      prevLines = [];
      prevResults = new Map();
      prevVariables = [];
      cachedLines = null;
      cachedLinesDoc = null;
      serverCacheSeeded = false;
    }

    function scheduleEval(delayMs = CALC_EVAL_DELAY_MS) {
      deferredEval = false;
      if (timer !== null) clearTimeout(timer);
      timer = window.setTimeout(() => {
        void runEval(view);
      }, Math.max(0, delayMs));
    }

    async function runEval(view: EditorView) {
      if (inFlight) {
        rerunRequested = true;
        return;
      }

      inFlight = true;
      do {
        rerunRequested = false;
        try {
          const doc = view.state.doc;
          if (doc.lines > MAX_CALC_EVAL_LINES) {
            clearCalcState(view);
            cachedHasGlobalSyntax = null;
            continue;
          }

          const snapshotDoc = doc;
          const nextLines = linesForDoc(doc);
          const lineStarts: number[] = new Array(nextLines.length);
          for (let i = 0, pos = 0; i < nextLines.length; i++) {
            lineStarts[i] = pos;
            pos += nextLines[i].length + 1;
          }

          const plan = planIncrementalCalc(prevLines, prevResults, nextLines);
          const hasPrev = prevLines.length > 0;
          // Prev-side changed slice mirrors the next-side plan:
          //   prevChangedTo = prevLen - suffix
          //                 = prevLen - (nextLen - plan.evalFrom - plan.evalLines.length)
          const prevChangedTo =
            prevLines.length - nextLines.length + plan.evalFrom + plan.evalLines.length;
          const prevChangedLines = prevLines.slice(plan.evalFrom, prevChangedTo);
          const forceFullTableRecalc = pendingFullTableRecalc;
          pendingFullTableRecalc = false;
          const evalWindow = forceFullTableRecalc
            ? {
              evalFrom: 0,
              evalTo: nextLines.length,
              touchesAnyAssignment: true,
              touchesBuiltinFormula: true,
              canUsePartial: false,
            }
            : calcDecideEvalWindow(
              nextLines,
              plan.evalFrom,
              plan.evalFrom + plan.evalLines.length,
              prevChangedLines,
              hasPrev,
              variablesEnabled,
              tableEnabled,
            );
          const canUsePartial = evalWindow.canUsePartial;
          const noteId = getActiveNoteId?.() ?? null;

          let evaluated;
          let evalFrom = evalWindow.evalFrom;
          let evalTo = evalWindow.evalTo;

          if (canUsePartial && plan.evalLines.length === 0 && evalFrom >= evalTo) {
            if (noteId && serverCacheSeeded && prevChangedTo > plan.evalFrom) {
              // Delete-only delta: keep backend line cache aligned even when no
              // lines need evaluation.
              await syncNoteLines(noteId, plan.evalFrom, prevChangedTo, []);
            }
            // No lines changed in the middle — prefix/suffix cover everything.
            const nextMap = new Map(plan.baseResults);
            if (!calcResultMapsEqual(prevResults, nextMap)) {
              view.dispatch({
                effects: [setCalcResults.of(nextMap)],
              });
            }
            prevLines = nextLines;
            prevResults = nextMap;
            continue;
          }

          if (canUsePartial) {
            if (noteId && serverCacheSeeded) {
              // Delta path: sync only the changed window, then eval.
              await syncNoteLines(noteId, plan.evalFrom, prevChangedTo, plan.evalLines);
              evaluated = await evaluateNoteContextDelta(
                noteId,
                variablesEnabled,
                { evalFrom, evalTo },
                tableEnabled,
              );
            } else {
              evaluated = await evaluateNoteContext(
                nextLines,
                variablesEnabled,
                { evalFrom, evalTo },
                tableEnabled,
              );
            }
          } else {
            if (noteId) {
              // Full sync — seeds the server cache for future delta calls.
              await syncNoteLines(noteId, 0, Number.MAX_SAFE_INTEGER, nextLines);
              serverCacheSeeded = true;
              evaluated = await evaluateNoteContextDelta(
                noteId,
                variablesEnabled,
                undefined,
                tableEnabled,
              );
            } else {
              evaluated = await evaluateNoteContext(
                nextLines,
                variablesEnabled,
                undefined,
                tableEnabled,
              );
            }
          }

          if (destroyed) break;

          // If the document changed during async evaluation, drop stale results and rerun.
          if (view.state.doc !== snapshotDoc) {
            rerunRequested = true;
            continue;
          }

          const nextMap = canUsePartial
            ? mergePartialCalcResults(plan.baseResults, evaluated.line_results, evalFrom, evalTo)
            : (() => {
                const m = new Map<number, string>();
                evaluated.line_results.forEach((result, lineIndex) => {
                  if (result !== null) m.set(lineIndex, result);
                });
                return m;
              })();

          // Per-cell formula values for table rows. We update the *eval range*
          // when partial, otherwise replace the whole map. Lines outside the
          // eval range carry their previous per-cell values (kept in the
          // state field and remapped through doc changes).
          const evalCellResults = evaluated.table_cell_results;
          const prevCellResults = view.state.field(cellCalcResultsField);
          const nextCellMap = canUsePartial ? new Map(prevCellResults) : new Map();
          if (canUsePartial) {
            for (let i = evalFrom; i < evalTo; i++) {
              const cells = evalCellResults[i];
              if (cells && cells.length > 0) {
                nextCellMap.set(i, cells);
              } else {
                nextCellMap.delete(i);
              }
            }
          } else {
            evalCellResults.forEach((cells, lineIndex) => {
              if (cells && cells.length > 0) {
                nextCellMap.set(lineIndex, cells);
              }
            });
          }

          // Compute refresh plan for committed trailers. Collect markers in
          // doc order (which is line order) via RangeSet.between so the
          // resulting changes are already sorted for CodeMirror's dispatch.
          const commitMarks = view.state.field(commitMarksField);
          const markerLocs: CommitMarkerLoc[] = [];
          commitMarks.between(0, doc.length, (from, _to, value) => {
            const line = doc.lineAt(from);
            markerLocs.push({
              docPos: from,
              lineIdx: line.number - 1,
              offsetInLine: from - line.from,
              lastLiteral: value.literal,
            });
          });

          const sel = view.state.selection.main;
          const refreshPlan = computeCalcRefresh(
            markerLocs,
            nextLines,
            lineStarts,
            nextMap,
            { from: sel.from, to: sel.to },
          );

          // Drop results for lines whose committed trailer will carry the
          // value inline, otherwise the ghost widget renders on top of
          // the literal after a refresh rewrite.
          for (const idx of refreshPlan.syncedLines) {
            nextMap.delete(idx);
          }

          const nextVariables = evaluated.variables ?? [];
          const resultsChanged = !calcResultMapsEqual(prevResults, nextMap);
          const variablesChanged = !variableIndexEqual(prevVariables, nextVariables);

          const effects: StateEffect<unknown>[] = [];
          if (resultsChanged) {
            effects.push(setCalcResults.of(nextMap));
          }
          if (!perLineMapsEqual(prevCellResults, nextCellMap)) {
            effects.push(setCellCalcResults.of(nextCellMap));
          }
          if (variablesChanged) {
            effects.push(setVariableIndex.of(nextVariables));
          }

          // Refreshed markers need to be dropped and re-added with the new
          // literal. Stack their pre-change positions onto the prune list so
          // they get filtered in the same transaction.
          const refreshedMarkerPositions = refreshPlan.changes.map((c) => c.from);
          const allPrune = refreshPlan.prune.concat(refreshedMarkerPositions);
          if (allPrune.length > 0) {
            effects.push(removeCommitMarks.of(allPrune));
          }

          // Post-change positions for the new markers, computed with a
          // running length delta over the ordered refresh changes. Each
          // change's trailer starts at its own pre-change `from`, offset by
          // the cumulative length delta of earlier changes in this batch.
          let cumulativeDelta = 0;
          for (const change of refreshPlan.changes) {
            const postPos = change.from + cumulativeDelta;
            effects.push(
              addCommitMark.of({ pos: postPos, literal: change.newLiteral }),
            );
            cumulativeDelta += change.insert.length - (change.to - change.from);
          }

          if (refreshPlan.changes.length > 0) {
            view.dispatch({
              effects,
              changes: refreshPlan.changes.map((c) => ({
                from: c.from,
                to: c.to,
                insert: c.insert,
              })),
              annotations: calcRefreshAnnotation.of(true),
            });
            // Apply the refresh rewrites locally so prevLines reflects the
            // post-dispatch doc; otherwise the next diff would treat refreshed
            // lines as user edits.
            for (const change of refreshPlan.changes) {
              const lineText = nextLines[change.lineIdx];
              const offsetInLine = change.from - lineStarts[change.lineIdx];
              nextLines[change.lineIdx] =
                lineText.slice(0, offsetInLine) + change.insert;
            }
            cachedLines = nextLines;
            cachedLinesDoc = view.state.doc;
          } else if (effects.length > 0) {
            view.dispatch({ effects });
          }

          prevLines = nextLines;
          prevResults = nextMap;
          prevVariables = nextVariables;
        } catch (e) {
          console.error("Calc evaluation failed:", e);
        }
      } while (rerunRequested && !destroyed);

      inFlight = false;
    }

    function shouldScheduleEval(
      view: EditorView,
      touchesCalcExpression: boolean,
    ): boolean {
      const doc = view.state.doc;
      return calcShouldScheduleEval(
        doc.lines,
        MAX_CALC_EVAL_LINES,
        hasGlobalSyntax(doc),
        touchesCalcExpression,
        visibleSpansTouchCalcSyntax(view),
      );
    }

    // initial evaluation
    if (shouldScheduleEval(view, false)) {
      scheduleEval();
    } else {
      deferredEval = true;
    }

    return {
      update(update: ViewUpdate) {
        if (!update.docChanged) {
          if (deferredEval && update.viewportChanged && shouldScheduleEval(update.view, false)) {
            scheduleEval();
          }
          return;
        }
        // Refresh-only transactions should not retrigger eval; the rewrites
        // they carry are purely cosmetic and the backend state is already
        // up to date.
        const isRefresh = update.transactions.some((t) =>
          t.annotation(calcRefreshAnnotation),
        );
        if (isRefresh) return;

        mapLineCacheThroughUpdate(update);

        // Any change that could add or remove a global-syntax line
        // invalidates the cached presence flag, forcing a rescan next time.
        if (updateTouchesCalcGlobalSyntax(update)) {
          cachedHasGlobalSyntax = null;
        }

        const touchesTableRows = tableEnabled && updateTouchesTableRows(update);
        if (touchesTableRows) {
          // Table formula dependencies can span rows/cols; force a full pass
          // to keep UI formula rendering in sync after table cell edits.
          pendingFullTableRecalc = true;
        }
        const touchesCalcExpression = updateTouchesCalcExpression(update) || touchesTableRows;
        if (shouldScheduleEval(update.view, touchesCalcExpression)) {
          scheduleEval(touchesTableRows ? CALC_TABLE_EDIT_EVAL_DELAY_MS : CALC_EVAL_DELAY_MS);
        } else {
          deferredEval = true;
        }
      },
      destroy() {
        destroyed = true;
        if (timer !== null) clearTimeout(timer);
      },
    };
  });
}

export function getCalcResultAtCursor(view: EditorView): string | null {
  const results = view.state.field(calcResultsField, false);
  if (!results) return null;
  const cursor = view.state.selection.main.head;
  const line = view.state.doc.lineAt(cursor);
  const segment = findCalcSegment(line.text);
  if (segment && builtinFormulaLabels(segment.expr).length > 0) return null;
  return results.get(line.number - 1) ?? null;
}

export function getVariableIndexEntries(view: EditorView): VariableIndexEntry[] {
  return view.state.field(variableIndexField, false) ?? [];
}

// Tab keymap: if cursor line has a calc result, apply it
const calcTabKeymap = keymap.of([
  {
    key: "Tab",
    run(view): boolean {
      const results = view.state.field(calcResultsField);
      const cursor = view.state.selection.main.head;
      const line = view.state.doc.lineAt(cursor);
      const lineIndex = line.number - 1; // 0-based
      const result = getCalcResultAtCursor(view);

      if (!result) return false;
      const lineText = line.text;
      if (lineUsesAssignmentGhostPrefix(lineText)) return false;

      // check if line already has " = <result>" at the end
      const segment = findCalcSegment(lineText);
      if (segment) {
        if (builtinFormulaLabels(segment.expr).length > 0) return false;
        const replaceFrom = line.from + segment.fromCol;
        const replaceTo = line.from + segment.toCol;
        const nextResults = new Map(results);
        nextResults.delete(lineIndex);

        view.dispatch({
          changes: { from: replaceFrom, to: replaceTo, insert: result },
          selection: { anchor: replaceFrom + result.length },
          effects: setCalcResults.of(nextResults),
          scrollIntoView: true,
        });
        return true;
      }

      const suffix = ` = ${result}`;
      if (lineText.endsWith(suffix)) return false;

      // if line has a previous " = ...", replace it
      const eqIdx = lineText.lastIndexOf(" = ");
      const insertFrom = eqIdx >= 0 ? line.from + eqIdx : line.to;
      const nextResults = new Map(results);
      nextResults.delete(lineIndex);
      const cursorAt = insertFrom + suffix.length;

      view.dispatch({
        changes: { from: insertFrom, to: line.to, insert: suffix },
        selection: { anchor: cursorAt },
        effects: [
          setCalcResults.of(nextResults),
          addCommitMark.of({ pos: insertFrom, literal: result }),
        ],
        scrollIntoView: true,
      });
      return true;
    },
  },
]);

export function getTableCellEvaluationMap(view: EditorView): Map<number, TableCellEvaluation[]> {
  return view.state.field(cellCalcResultsField, false) ?? new Map();
}

export function getTableCellEvaluationMapFromState(
  state: EditorState,
): Map<number, TableCellEvaluation[]> {
  return state.field(cellCalcResultsField, false) ?? new Map();
}

export function calcExtensions(options: CalcExtensionOptions = {}) {
  return [
    calcResultsField,
    cellCalcResultsField,
    variableIndexField,
    commitMarksField,
    calcDecorationsPlugin(),
    buildCalcPlugin(options),
    calcTabKeymap,
  ];
}
