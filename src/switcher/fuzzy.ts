export interface FuzzyMatch {
  index: number;
  score: number;
  positions: number[];
}

export function fuzzyMatch(query: string, text: string): FuzzyMatch | null {
  if (query.length === 0) return { index: 0, score: 0, positions: [] };

  const lowerQuery = query.toLowerCase();
  const lowerText = text.toLowerCase();
  const positions: number[] = [];
  let qi = 0;
  let score = 0;
  let prevMatch = -2;

  for (let ti = 0; ti < lowerText.length && qi < lowerQuery.length; ti++) {
    if (lowerText[ti] === lowerQuery[qi]) {
      positions.push(ti);
      // consecutive matches score higher
      score += prevMatch === ti - 1 ? 2 : 1;
      // match at word boundary scores higher
      if (ti === 0 || /\s/.test(text[ti - 1])) score += 1;
      prevMatch = ti;
      qi++;
    }
  }

  if (qi < lowerQuery.length) return null;
  return { index: 0, score, positions };
}

export function fuzzyFilter<T>(
  query: string,
  items: T[],
  getText: (item: T) => string,
): { item: T; positions: number[] }[] {
  if (query.length === 0) return items.map((item) => ({ item, positions: [] }));

  const results: { item: T; score: number; positions: number[] }[] = [];
  for (const item of items) {
    const match = fuzzyMatch(query, getText(item));
    if (match) {
      results.push({ item, score: match.score, positions: match.positions });
    }
  }
  results.sort((a, b) => b.score - a.score);
  return results;
}
