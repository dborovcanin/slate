import {
  EditorView,
  Decoration,
  DecorationSet,
  ViewPlugin,
  ViewUpdate,
  WidgetType,
  keymap,
} from "@codemirror/view";
import { StateField, StateEffect, RangeSetBuilder } from "@codemirror/state";
import { evaluateLines } from "../api";
import { planIncrementalCalc } from "./calc-incremental";
import { findCalcSegment, lineForCalcEvaluation } from "./calc-line-utils";

// Effect to update calc results from backend
const setCalcResults = StateEffect.define<Map<number, string>>();

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
  constructor(readonly result: string) {
    super();
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

// ViewPlugin that debounces doc changes and fetches calc results
const calcPlugin = ViewPlugin.define((view) => {
  let timer: number | null = null;
  let inFlight = false;
  let rerunRequested = false;
  let destroyed = false;
  let prevEvalLines: string[] = [];
  let prevResults = new Map<number, string>();

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
        const evalLines: string[] = [];
        for (let i = 1; i <= doc.lines; i++) {
          evalLines.push(lineForCalcEvaluation(doc.line(i).text));
        }

        const plan = planIncrementalCalc(prevEvalLines, prevResults, evalLines);
        const nextMap = new Map(plan.baseResults);
        let evaluated: (string | null)[] = [];
        if (plan.evalLines.length > 0) {
          evaluated = await evaluateLines(plan.evalLines);
        }
        if (destroyed) break;

        // If the document changed during async evaluation, drop stale results and rerun.
        if (view.state.doc.toString() !== snapshot) {
          rerunRequested = true;
          continue;
        }

        evaluated.forEach((r, i) => {
          if (r !== null) nextMap.set(plan.evalFrom + i, r);
        });
        prevEvalLines = evalLines;
        prevResults = nextMap;
        view.dispatch({ effects: setCalcResults.of(nextMap) });
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

// Tab keymap: if cursor line has a calc result, apply it
const calcTabKeymap = keymap.of([
  {
    key: "Tab",
    run(view): boolean {
      const results = view.state.field(calcResultsField);
      const cursor = view.state.selection.main.head;
      const line = view.state.doc.lineAt(cursor);
      const lineIndex = line.number - 1; // 0-based
      const result = results.get(lineIndex);

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

export function calcExtensions() {
  return [calcResultsField, calcDecorations, calcPlugin, calcTabKeymap];
}
