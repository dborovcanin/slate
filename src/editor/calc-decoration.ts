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
} from "@codemirror/state";
import { evaluateNoteContext, type VariableIndexEntry } from "../api.ts";
import { findCalcSegment } from "./calc-line-utils.ts";
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
    return value;
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
    return value;
  },
});

// Decoration set derived from the calc results field
const calcDecorations = EditorView.decorations.compute(
  [calcResultsField],
  (state) => {
    const results = state.field(calcResultsField);
    const builder = new RangeSetBuilder<Decoration>();

    for (let i = 0; i < state.doc.lines; i++) {
      const result = results.get(i);
      if (result) {
        const line = state.doc.line(i + 1); // 1-based
        builder.add(
          line.to,
          line.to,
          Decoration.widget({
            widget: new CalcResultWidget(result),
            side: 1,
          }),
        );
      }
    }

    return builder.finish();
  },
);

class CalcResultWidget extends WidgetType {
  readonly result: string;

  constructor(result: string) {
    super();
    this.result = result;
  }

  toDOM(): HTMLElement {
    const span = document.createElement("span");
    span.className = "calc-ghost";
    span.textContent = ` \u2192 ${this.result}`;
    return span;
  }

  eq(other: CalcResultWidget): boolean {
    return this.result === other.result;
  }
}

const ASSIGNMENT_RE = /(^|[^:!<>=])(:=)(?!=)/;

export function containsVariableAssignment(lines: readonly string[]): boolean {
  for (const line of lines) {
    if (ASSIGNMENT_RE.test(line)) return true;
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

function buildCalcPlugin(options: CalcExtensionOptions) {
  const variablesEnabled = options.variablesEnabled ?? true;

  return ViewPlugin.define((view) => {
    let timer: number | null = null;
    let inFlight = false;
    let rerunRequested = false;
    let destroyed = false;
    let prevLines: string[] = [];
    let prevResults: Map<number, string> = new Map();

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
          const canUsePartial = hasPrev && !touchesAnyAssignment;

          let evaluated;
          let evalFrom = 0;
          let evalTo = nextLines.length;

          if (canUsePartial && plan.evalLines.length === 0) {
            // No lines changed in the middle — prefix/suffix cover everything.
            const nextMap = new Map(plan.baseResults);
            view.dispatch({
              effects: [setCalcResults.of(nextMap)],
            });
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

          const effects: StateEffect<unknown>[] = [
            setCalcResults.of(nextMap),
            setVariableIndex.of(evaluated.variables ?? []),
          ];

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
          } else {
            view.dispatch({ effects });
          }

          prevLines = nextLines;
          prevResults = nextMap;
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

      // check if line already has " = <result>" at the end
      const lineText = line.text;
      const segment = findCalcSegment(lineText);
      if (segment) {
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
