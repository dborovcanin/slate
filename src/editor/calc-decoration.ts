import {
  EditorView,
  Decoration,
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
  type Text,
} from "@codemirror/state";
import { evaluateNoteContext, type VariableIndexEntry } from "../api.ts";
import {
  findCalcSegment,
  findSingleCalcTableCell,
  isBuiltinFormula,
  lineForCalcEvaluation,
} from "./calc-line-utils.ts";
import { planIncrementalCalc } from "./calc-incremental.ts";

export interface CalcExtensionOptions {
  variablesEnabled?: boolean;
}

// Effect to update calc results from backend
const setCalcResults = StateEffect.define<Map<number, string>>();
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
}

const ASSIGNMENT_RE = /(^|[^:!<>=])(:=)(?!=)/;

export function builtinFormulaExplanation(expression: string): string | null {
  const trimmed = expression.trim();
  if (!isBuiltinFormula(trimmed)) return null;
  const withoutEquals = trimmed.startsWith("=") ? trimmed.slice(1).trim() : trimmed;
  const compact = withoutEquals.replace(/\s+/g, "").toLowerCase();
  const token = compact.endsWith("()") ? compact.slice(0, -2) : compact;
  const normalized =
    token === "sum_column"
      ? "sum_col"
      : token === "avg_column"
        ? "avg_col"
        : token;
  return `${normalized}()`;
}

export function formatFormulaDisplayValue(raw: string): string {
  const trimmed = raw.trim();
  if (trimmed.length === 0) return raw;

  let cleaned = trimmed
    .replace(/^≈\s*/u, "")
    .replace(/^~\s*/u, "")
    .replace(/^approximately\s+/i, "")
    .replace(/^approx\.?\s+/i, "")
    .replace(/^about\s+/i, "")
    .trim();
  if (cleaned.length === 0) cleaned = trimmed;

  const match = cleaned.match(/^([+-]?\d+(?:\.\d+)?)(\s+.*)?$/);
  if (!match) return cleaned;

  const num = Number.parseFloat(match[1] ?? "");
  if (!Number.isFinite(num)) return cleaned;

  const rounded = Math.round((num + Number.EPSILON) * 100) / 100;
  const value = Number.isInteger(rounded)
    ? rounded.toString()
    : rounded.toFixed(2).replace(/\.?0+$/, "");
  return `${value}${match[2] ?? ""}`.trimEnd();
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

const FORMULA_GHOST_MARKER = "*";

// Decoration set derived from the calc results field
const calcDecorations = EditorView.decorations.compute(
  [calcResultsField, "doc", "selection"],
  (state) => {
    const results = state.field(calcResultsField);
    const builder = new RangeSetBuilder<Decoration>();
    const selection = state.selection.main;

    for (const [lineIndex, result] of results) {
      const lineNumber = lineIndex + 1;
      if (lineNumber < 1 || lineNumber > state.doc.lines) continue;
      const line = state.doc.line(lineNumber); // 1-based

      const cell = findSingleCalcTableCell(line.text);
      const explanation =
        cell && isBuiltinFormula(cell.expr) ? builtinFormulaExplanation(cell.expr) : null;
      if (cell && explanation) {
        const editingCell = selectionTouchesSegment(
          selection,
          line.from,
          cell.fromCol,
          cell.toCol,
        );
        if (editingCell) continue;

        const formatted = formatFormulaDisplayValue(result);
        const minWidthCh = Math.max(
          1,
          cell.toCol - cell.fromCol,
          formatted.length + FORMULA_GHOST_MARKER.length,
        );

        builder.add(
          line.from + cell.fromCol,
          line.from + cell.toCol,
          Decoration.replace({
            widget: new FormulaCellWidget(
              formatted,
              FORMULA_GHOST_MARKER,
              minWidthCh,
            ),
          }),
        );

        builder.add(
          line.to,
          line.to,
          Decoration.widget({
            widget: new CalcResultWidget(
              `${FORMULA_GHOST_MARKER} \u279c ${explanation}`,
              " ",
            ),
            side: 1,
          }),
        );

        continue;
      }
      const prefix = lineUsesAssignmentGhostPrefix(line.text) ? " = " : " \u2192 ";
      builder.add(
        line.to,
        line.to,
        Decoration.widget({
          widget: new CalcResultWidget(result, prefix),
          side: 1,
        }),
      );
    }

    return builder.finish();
  },
);

export function lineUsesAssignmentGhostPrefix(lineText: string): boolean {
  const evalTarget = lineForCalcEvaluation(lineText).trim();
  if (evalTarget.length > 0) return ASSIGNMENT_RE.test(evalTarget);
  return ASSIGNMENT_RE.test(lineText);
}

export function containsVariableAssignment(lines: readonly string[]): boolean {
  for (const line of lines) {
    if (ASSIGNMENT_RE.test(line)) return true;
  }
  return false;
}

export function containsBuiltinFormula(lines: readonly string[]): boolean {
  for (const line of lines) {
    const evalTarget = lineForCalcEvaluation(line).trim();
    if (evalTarget.length === 0) continue;
    if (isBuiltinFormula(evalTarget)) return true;
  }
  return false;
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
  const changes: CalcRefreshChange[] = [];
  const prune: number[] = [];
  const syncedLines: number[] = [];

  for (const marker of markers) {
    const { docPos, lineIdx, offsetInLine, lastLiteral } = marker;
    const lineText = lines[lineIdx];
    if (lineText === undefined) {
      prune.push(docPos);
      continue;
    }

    // Trailer sanity: marker must still sit at the " = " prefix it was born
    // against. Any drift (partial delete, reflow) means the committed trailer
    // is gone and we should relinquish ownership.
    if (lineText.slice(offsetInLine, offsetInLine + 3) !== " = ") {
      prune.push(docPos);
      continue;
    }

    const currentLiteral = lineText.slice(offsetInLine + 3);

    // Hand-edit detection: the marker carries the literal we last wrote. If
    // the doc now holds anything else at that slot, the user has edited the
    // trailer (including appending trailing text). Relinquish ownership so we
    // don't clobber their edit.
    if (currentLiteral !== lastLiteral) {
      prune.push(docPos);
      continue;
    }

    const nextResult = nextResults.get(lineIdx);

    // Backend no longer yields a result (expression dropped, mid-edit). Keep
    // the marker so the trailer stays live when the expression comes back.
    if (nextResult === undefined) continue;

    // Already in sync — nothing to rewrite, but flag the line so the
    // caller suppresses the redundant ghost widget.
    if (currentLiteral === nextResult) {
      syncedLines.push(lineIdx);
      continue;
    }

    const lineStart = lineStarts[lineIdx];
    const trailerFrom = lineStart + offsetInLine;
    const trailerTo = lineStart + lineText.length;

    // Cursor guard: never yank the caret out from under the user when they
    // are editing inside the trailer span.
    if (selection.from <= trailerTo && selection.to >= trailerFrom) continue;

    changes.push({
      lineIdx,
      from: trailerFrom,
      to: trailerTo,
      insert: ` = ${nextResult}`,
      newLiteral: nextResult,
    });
    syncedLines.push(lineIdx);
  }

  return { changes, prune, syncedLines };
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
): number | null {
  const window = rangeSearchLineWindow(nextDoc, range);
  let closest: number | null = null;
  let bestDistance = Number.POSITIVE_INFINITY;

  for (let lineNo = window.startLine; lineNo <= window.endLine; lineNo++) {
    const lineIndex = lineNo - 1;
    if (claimedLineIndexes.has(lineIndex)) continue;
    const line = nextDoc.line(lineNo);
    if (lineEvalKey(line.text) !== key) continue;

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

  return ViewPlugin.define((view) => {
    let timer: number | null = null;
    let inFlight = false;
    let rerunRequested = false;
    let destroyed = false;
    let prevLines: string[] = [];
    let prevResults: Map<number, string> = new Map();
    let prevVariables: VariableIndexEntry[] = [];

    function scheduleEval() {
      if (timer !== null) clearTimeout(timer);
      timer = window.setTimeout(() => {
        void runEval(view);
      }, 150);
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
          const snapshot = doc.toString();
          const nextLines: string[] = [];
          const lineStarts: number[] = [];
          for (let i = 1; i <= doc.lines; i++) {
            const line = doc.line(i);
            nextLines.push(line.text);
            lineStarts.push(line.from);
          }

          const plan = planIncrementalCalc(prevLines, prevResults, nextLines);
          const hasPrev = prevLines.length > 0;
          // Prev-side changed slice mirrors the next-side plan:
          //   prevChangedTo = prevLen - suffix
          //                 = prevLen - (nextLen - plan.evalFrom - plan.evalLines.length)
          const prevChangedTo =
            prevLines.length - nextLines.length + plan.evalFrom + plan.evalLines.length;
          const prevChangedLines = prevLines.slice(plan.evalFrom, prevChangedTo);
          const touchesAnyAssignment =
            containsVariableAssignment(plan.evalLines) ||
            containsVariableAssignment(prevChangedLines);
          const hasBuiltinFormula =
            containsBuiltinFormula(nextLines) || containsBuiltinFormula(prevLines);
          const canUsePartial = hasPrev && !touchesAnyAssignment && !hasBuiltinFormula;

          let evaluated;
          let evalFrom = 0;
          let evalTo = nextLines.length;

          if (canUsePartial && plan.evalLines.length === 0) {
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
            evalFrom = plan.evalFrom;
            evalTo = plan.evalFrom + plan.evalLines.length;
            evaluated = await evaluateNoteContext(nextLines, variablesEnabled, {
              evalFrom,
              evalTo,
            });
          } else {
            evaluated = await evaluateNoteContext(nextLines, variablesEnabled);
          }

          if (destroyed) break;

          // If the document changed during async evaluation, drop stale results and rerun.
          if (view.state.doc.toString() !== snapshot) {
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

    // initial evaluation
    scheduleEval();

    return {
      update(update: ViewUpdate) {
        if (!update.docChanged) return;
        // Refresh-only transactions should not retrigger eval; the rewrites
        // they carry are purely cosmetic and the backend state is already
        // up to date.
        const isRefresh = update.transactions.some((t) =>
          t.annotation(calcRefreshAnnotation),
        );
        if (isRefresh) return;
        scheduleEval();
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
  if (segment && isBuiltinFormula(segment.expr)) return null;
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
        if (isBuiltinFormula(segment.expr)) return false;
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

export function calcExtensions(options: CalcExtensionOptions = {}) {
  return [
    calcResultsField,
    variableIndexField,
    commitMarksField,
    calcDecorations,
    buildCalcPlugin(options),
    calcTabKeymap,
  ];
}
