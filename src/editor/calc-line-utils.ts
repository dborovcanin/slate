import {
  calcFindListSegment,
  calcFindSegment,
  calcFindSingleTableCell,
  calcIsBuiltinFormula,
  calcLineForEvaluation,
} from "./wasm.ts";

interface TableCalcCell {
  expr: string;
  fromCol: number;
  toCol: number;
}

function mapSegment(
  segment: { expr: string; fromCol: number; toCol: number } | null,
): TableCalcCell | null {
  if (!segment) return null;
  return {
    expr: segment.expr,
    fromCol: segment.fromCol,
    toCol: segment.toCol,
  };
}

export function isBuiltinFormula(text: string): boolean {
  return calcIsBuiltinFormula(text);
}

export function findSingleCalcTableCell(lineText: string): TableCalcCell | null {
  return mapSegment(calcFindSingleTableCell(lineText));
}

export function findListCalcSegment(lineText: string): TableCalcCell | null {
  return mapSegment(calcFindListSegment(lineText));
}

export function findCalcSegment(lineText: string): TableCalcCell | null {
  return mapSegment(calcFindSegment(lineText));
}

export function lineForCalcEvaluation(lineText: string): string {
  return calcLineForEvaluation(lineText);
}

export type { TableCalcCell };
