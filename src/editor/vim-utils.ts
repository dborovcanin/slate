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

// Word-motion character classes, matching `char_class` in
// `crates/editor-core/src/vim_actions.rs`: whitespace / word / other.
// The core classifier is Unicode-aware (`char::is_whitespace`,
// `char::is_alphanumeric`), so this one has to be too or `w`/`b`/`e` land on
// different columns in the UI than in the terminal on non-ASCII text.
const WORD_CHAR_RE = /[\p{Alphabetic}\p{N}_]/u;
const WHITESPACE_RE = /\s/u;

export function vimCharClass(char: string): number {
  if (!char) return 0;
  // ASCII covers the overwhelming majority of characters a motion walks over,
  // and these run per-character inside motion loops, so keep them off the
  // RegExp path.
  const code = char.charCodeAt(0);
  if (code < 0x80) {
    if (code === 0x20 || (code >= 0x09 && code <= 0x0d)) return 0;
    const isWord =
      (code >= 0x30 && code <= 0x39) ||
      (code >= 0x41 && code <= 0x5a) ||
      (code >= 0x61 && code <= 0x7a) ||
      code === 0x5f;
    return isWord ? 1 : 2;
  }
  if (WHITESPACE_RE.test(char)) return 0;
  return WORD_CHAR_RE.test(char) ? 1 : 2;
}

export function vimNormalLineEndPos(lineFrom: number, lineTo: number): number {
  return lineTo > lineFrom ? lineTo - 1 : lineFrom;
}

export function vimAppendInsertPos(head: number, lineTo: number): number {
  return head < lineTo ? head + 1 : lineTo;
}
