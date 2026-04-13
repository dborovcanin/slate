import {
  EditorView,
  Decoration,
  ViewPlugin,
  ViewUpdate,
  WidgetType,
  keymap,
} from "@codemirror/view";
import { StateField, StateEffect, RangeSetBuilder } from "@codemirror/state";
import { evaluateNoteContext, type VariableIndexEntry } from "../api.ts";
import { findCalcSegment } from "./calc-line-utils.ts";
import { planIncrementalCalc } from "./calc-incremental.ts";

export interface CalcExtensionOptions {
  variablesEnabled?: boolean;
}

// Effect to update calc results from backend
const setCalcResults = StateEffect.define<Map<number, string>>();
const setVariableIndex = StateEffect.define<VariableIndexEntry[]>();

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
          for (let i = 1; i <= doc.lines; i++) {
            nextLines.push(doc.line(i).text);
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

          view.dispatch({
            effects: [
              setCalcResults.of(nextMap),
              setVariableIndex.of(evaluated.variables ?? []),
            ],
          });

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
        if (update.docChanged) scheduleEval();
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
        effects: setCalcResults.of(nextResults),
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
    calcDecorations,
    buildCalcPlugin(options),
    calcTabKeymap,
  ];
}
