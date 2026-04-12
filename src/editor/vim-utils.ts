export interface BlockSpan {
  lineIndex: number;
  fromCol: number;
  toCol: number;
}

function clamp(value: number, min: number, max: number) {
  return Math.min(max, Math.max(min, value));
}

export function computeBlockSpans(
  lineLengths: number[],
  anchorLine: number,
  anchorCol: number,
  headLine: number,
  headCol: number,
): BlockSpan[] {
  if (lineLengths.length === 0) return [];

  const lineMax = lineLengths.length - 1;
  const aLine = clamp(anchorLine, 0, lineMax);
  const hLine = clamp(headLine, 0, lineMax);
  const startLine = Math.min(aLine, hLine);
  const endLine = Math.max(aLine, hLine);

  const startCol = Math.max(0, Math.min(anchorCol, headCol));
  const endColInclusive = Math.max(0, Math.max(anchorCol, headCol));
  const spans: BlockSpan[] = [];

  for (let lineIndex = startLine; lineIndex <= endLine; lineIndex++) {
    const len = Math.max(0, lineLengths[lineIndex] ?? 0);
    const fromCol = Math.min(startCol, len);
    const toCol = Math.min(endColInclusive + 1, len);
    spans.push({ lineIndex, fromCol, toCol });
  }

  return spans;
}
