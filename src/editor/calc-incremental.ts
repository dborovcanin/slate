// Pure-JS incremental calc planner — no WASM boundary crossing.
// Replaces the previous wasm_calc_plan_incremental call which copied all
// prevLines + nextLines (potentially 212k strings for a 106k-line doc)
// across the WASM boundary just to compute a prefix/suffix diff.
//
// The planner finds the minimal eval range by comparing line strings directly
// in JS: O(N) string comparisons, but no serialization or memory allocation
// beyond the result object itself.

export interface IncrementalCalcPlan {
  baseResults: Map<number, string>;
  evalFrom: number;
  evalLines: string[];
}

export function planIncrementalCalc(
  prevLines: readonly string[],
  prevResults: ReadonlyMap<number, string>,
  nextLines: readonly string[],
): IncrementalCalcPlan {
  const prevLen = prevLines.length;
  const nextLen = nextLines.length;

  if (prevLen === 0) {
    return { baseResults: new Map(), evalFrom: 0, evalLines: [...nextLines] };
  }

  // Scan from the start to find the first differing line.
  const minLen = Math.min(prevLen, nextLen);
  let prefixLen = 0;
  while (prefixLen < minLen && prevLines[prefixLen] === nextLines[prefixLen]) {
    prefixLen++;
  }

  // Nothing changed.
  if (prefixLen === prevLen && prevLen === nextLen) {
    return { baseResults: new Map(prevResults), evalFrom: 0, evalLines: [] };
  }

  // Scan from the end to find the common suffix, stopping before the prefix.
  let prevSuffixStart = prevLen;
  let nextSuffixStart = nextLen;
  while (
    prevSuffixStart > prefixLen &&
    nextSuffixStart > prefixLen &&
    prevLines[prevSuffixStart - 1] === nextLines[nextSuffixStart - 1]
  ) {
    prevSuffixStart--;
    nextSuffixStart--;
  }

  const evalFrom = prefixLen;
  const evalTo = nextSuffixStart;
  const evalLines = nextLines.slice(evalFrom, evalTo) as string[];

  // Build baseResults by carrying over results for unchanged lines.
  // Prefix lines (0..prefixLen) map 1-to-1.
  // Suffix lines (prevSuffixStart..prevLen) shift to (nextSuffixStart..nextLen).
  const baseResults = new Map<number, string>();
  for (const [idx, result] of prevResults) {
    if (idx < prefixLen) {
      baseResults.set(idx, result);
    } else if (idx >= prevSuffixStart) {
      // Remap: suffix shift from prev indices to next indices.
      baseResults.set(idx - prevSuffixStart + nextSuffixStart, result);
    }
    // Lines in the changed range are dropped — they will be re-evaluated.
  }

  return { baseResults, evalFrom, evalLines };
}
