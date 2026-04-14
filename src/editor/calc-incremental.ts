import { calcPlanIncremental } from "./wasm.ts";

export interface IncrementalCalcPlan {
  baseResults: Map<number, string>;
  evalFrom: number;
  evalLines: string[];
}

export function planIncrementalCalc(
  prevLines: string[],
  prevResults: Map<number, string>,
  nextLines: string[],
): IncrementalCalcPlan {
  const plan = calcPlanIncremental(prevLines, prevResults, nextLines);
  return {
    baseResults: plan.baseResults,
    evalFrom: plan.evalFrom,
    evalLines: plan.evalLines,
  };
}
