export interface IncrementalCalcPlan {
  baseResults: Map<number, string>;
  evalFrom: number;
  evalLines: string[];
}

function sharedPrefixLen(a: string[], b: string[]): number {
  const max = Math.min(a.length, b.length);
  let i = 0;
  while (i < max && a[i] === b[i]) i++;
  return i;
}

function sharedSuffixLen(a: string[], b: string[], prefixLen: number): number {
  const max = Math.min(a.length, b.length) - prefixLen;
  let i = 0;
  while (i < max && a[a.length - 1 - i] === b[b.length - 1 - i]) i++;
  return i;
}

export function planIncrementalCalc(
  prevLines: string[],
  prevResults: Map<number, string>,
  nextLines: string[],
): IncrementalCalcPlan {
  if (prevLines.length === 0) {
    return {
      baseResults: new Map(),
      evalFrom: 0,
      evalLines: [...nextLines],
    };
  }

  const prefix = sharedPrefixLen(prevLines, nextLines);
  const suffix = sharedSuffixLen(prevLines, nextLines, prefix);
  const nextLen = nextLines.length;
  const prevLen = prevLines.length;
  const changedFrom = prefix;
  const changedTo = nextLen - suffix;

  const baseResults = new Map<number, string>();

  for (let i = 0; i < prefix; i++) {
    const result = prevResults.get(i);
    if (result !== undefined) baseResults.set(i, result);
  }

  for (let i = 0; i < suffix; i++) {
    const prevIdx = prevLen - suffix + i;
    const nextIdx = nextLen - suffix + i;
    const result = prevResults.get(prevIdx);
    if (result !== undefined) baseResults.set(nextIdx, result);
  }

  return {
    baseResults,
    evalFrom: changedFrom,
    evalLines: nextLines.slice(changedFrom, changedTo),
  };
}
