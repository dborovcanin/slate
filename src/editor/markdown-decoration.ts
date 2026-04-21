import { Annotation, RangeSetBuilder, type Text } from "@codemirror/state";
import { Decoration, EditorView, ViewPlugin, WidgetType } from "@codemirror/view";
import type { DecorationSet, ViewUpdate } from "@codemirror/view";
import { variableIndexField } from "./calc-decoration.ts";
import type { VariableIndexEntry } from "../api.ts";
import {
  ensureWasmReady,
  markdownAnalyzeLines,
  markdownClassifyLine,
  markdownFindInlineTokens,
  markdownTokenizeCodeLine,
  type MarkdownCodeToken as SharedCodeToken,
  type MarkdownInlineToken as SharedInlineToken,
  type MarkdownLineInfo as SharedMarkdownLineInfo,
} from "./wasm.ts";

type InlineToken = SharedInlineToken;
type CodeToken = SharedCodeToken;
export type MarkdownLineInfo = SharedMarkdownLineInfo;

interface FenceState {
  inCodeBlock: boolean;
  codeFenceLang: string | null;
}

const DEFAULT_FENCE_STATE: FenceState = {
  inCodeBlock: false,
  codeFenceLang: null,
};

const VIEWPORT_MARGIN_LINES = 24;
const HOTPATH_REBUILD_MARGIN_LINES = 8;
const FENCE_CHECKPOINT_INTERVAL = 256;

const decHeadingToken = Decoration.mark({ class: "md-token md-token-heading" });
const decQuoteToken = Decoration.mark({ class: "md-token md-token-quote" });
const decListToken = Decoration.mark({ class: "md-token md-token-list" });
const decRuleToken = Decoration.mark({ class: "md-token md-token-rule" });
const decFenceToken = Decoration.mark({ class: "md-token md-token-code-fence" });
const decHeadingContent = Decoration.mark({ class: "md-heading-content" });
const decChecklistDoneContent = Decoration.mark({ class: "md-checklist-content-done" });
const decStrong = Decoration.mark({ class: "md-strong" });
const decEmphasis = Decoration.mark({ class: "md-emphasis" });
const decStrike = Decoration.mark({ class: "md-strike" });
const decCode = Decoration.mark({ class: "md-inline-code-content" });
const decCodeMarker = Decoration.mark({ class: "md-token md-token-code" });
const decCodeKeyword = Decoration.mark({ class: "md-code-token-keyword" });
const decCodeString = Decoration.mark({ class: "md-code-token-string" });
const decCodeNumber = Decoration.mark({ class: "md-code-token-number" });
const decCodeComment = Decoration.mark({ class: "md-code-token-comment" });
const decCodeFunction = Decoration.mark({ class: "md-code-token-function" });
const decCodeType = Decoration.mark({ class: "md-code-token-type" });
const decLinkText = Decoration.mark({ class: "md-link-text" });
const decLinkUrl = Decoration.mark({ class: "md-link-url" });
const decLinkMarker = Decoration.mark({ class: "md-token md-token-link" });
const decVariable = Decoration.mark({ class: "md-variable" });

const lineClass = (className: string) => Decoration.line({ class: className });

const decHeadingLine = [
  lineClass("md-line md-heading md-heading-1"),
  lineClass("md-line md-heading md-heading-2"),
  lineClass("md-line md-heading md-heading-3"),
  lineClass("md-line md-heading md-heading-4"),
  lineClass("md-line md-heading md-heading-5"),
  lineClass("md-line md-heading md-heading-6"),
];
const decQuoteLine = lineClass("md-line md-quote");
const decListLine = lineClass("md-line md-list-item");
const decChecklistLine = lineClass("md-line md-checklist-item");
const decRuleLine = lineClass("md-line md-hr");
const decCodeFenceLine = lineClass("md-line md-code-fence");
const decCodeBlockLine = lineClass("md-line md-code-block-line");

class ChecklistMarkWidget extends WidgetType {
  private readonly checked: boolean;

  constructor(checked: boolean) {
    super();
    this.checked = checked;
  }

  eq(other: ChecklistMarkWidget): boolean {
    return other.checked === this.checked;
  }

  toDOM(): HTMLElement {
    const span = document.createElement("span");
    span.className = this.checked
      ? "md-checklist-mark md-checklist-mark-checked"
      : "md-checklist-mark";
    span.contentEditable = "false";
    span.setAttribute("draggable", "false");
    span.setAttribute("aria-hidden", "true");
    return span;
  }

  ignoreEvent(): boolean {
    return false;
  }
}

class HiddenChecklistPrefixWidget extends WidgetType {
  eq(): boolean {
    return true;
  }

  toDOM(): HTMLElement {
    const span = document.createElement("span");
    span.contentEditable = "false";
    span.setAttribute("draggable", "false");
    span.setAttribute("aria-hidden", "true");
    return span;
  }
}

class HiddenMarkdownTokenWidget extends WidgetType {
  eq(): boolean {
    return true;
  }

  toDOM(): HTMLElement {
    const span = document.createElement("span");
    span.className = "md-hidden-token";
    span.contentEditable = "false";
    span.setAttribute("draggable", "false");
    span.setAttribute("aria-hidden", "true");
    return span;
  }
}

class UnorderedListGlyphWidget extends WidgetType {
  private readonly glyph: string;

  constructor(glyph: string) {
    super();
    this.glyph = glyph;
  }

  eq(other: UnorderedListGlyphWidget): boolean {
    return other.glyph === this.glyph;
  }

  toDOM(): HTMLElement {
    const span = document.createElement("span");
    span.className = "md-unordered-list-glyph";
    span.textContent = this.glyph;
    span.contentEditable = "false";
    span.setAttribute("draggable", "false");
    span.setAttribute("aria-hidden", "true");
    return span;
  }
}

const decChecklistHiddenPrefix = Decoration.replace({
  widget: new HiddenChecklistPrefixWidget(),
  inclusive: false,
});

const decHiddenMarkdownToken = Decoration.replace({
  widget: new HiddenMarkdownTokenWidget(),
  inclusive: false,
});
const decUnorderedListBullet = Decoration.replace({
  widget: new UnorderedListGlyphWidget("•"),
  inclusive: false,
});
const decUnorderedListArrow = Decoration.replace({
  widget: new UnorderedListGlyphWidget("▸"),
  inclusive: false,
});

const decChecklistMark = Decoration.replace({
  widget: new ChecklistMarkWidget(false),
  inclusive: false,
});
const decChecklistMarkChecked = Decoration.replace({
  widget: new ChecklistMarkWidget(true),
  inclusive: false,
});

export function classifyMarkdownLine(text: string): MarkdownLineInfo {
  return markdownClassifyLine(text);
}

export function findInlineMarkdownTokens(text: string): InlineToken[] {
  return markdownFindInlineTokens(text);
}

function addCodeSyntaxDecorations(
  builder: RangeSetBuilder<Decoration>,
  lineFrom: number,
  tokens: readonly CodeToken[],
) {
  for (const token of tokens) {
    const from = lineFrom + token.from;
    const to = lineFrom + token.to;
    if (token.type === "keyword") builder.add(from, to, decCodeKeyword);
    if (token.type === "string") builder.add(from, to, decCodeString);
    if (token.type === "number") builder.add(from, to, decCodeNumber);
    if (token.type === "comment") builder.add(from, to, decCodeComment);
    if (token.type === "function") builder.add(from, to, decCodeFunction);
    if (token.type === "type") builder.add(from, to, decCodeType);
  }
}

export function tokenizeCodeLine(text: string, lang: string | null): CodeToken[] {
  return markdownTokenizeCodeLine(text, lang);
}

interface PendingDecoration {
  from: number;
  to: number;
  decoration: Decoration;
}

function isInlineMarkerToken(token: InlineToken): boolean {
  return token.type === "code-marker" || token.type === "link-marker";
}

function markerRevealComponentRange(
  tokens: readonly InlineToken[],
  markerIndex: number,
): TextRange | null {
  const marker = tokens[markerIndex];
  if (!marker || !isInlineMarkerToken(marker)) return null;

  let startIndex = markerIndex;
  while (startIndex > 0) {
    const prev = tokens[startIndex - 1];
    const current = tokens[startIndex];
    if (!prev || !current || prev.to !== current.from) break;
    startIndex -= 1;
  }

  let endIndex = markerIndex;
  while (endIndex + 1 < tokens.length) {
    const current = tokens[endIndex];
    const next = tokens[endIndex + 1];
    if (!current || !next || current.to !== next.from) break;
    endIndex += 1;
  }

  let hasNonMarker = false;
  for (let index = startIndex; index <= endIndex; index++) {
    const token = tokens[index];
    if (token && !isInlineMarkerToken(token)) {
      hasNonMarker = true;
      break;
    }
  }
  if (!hasNonMarker) return null;

  return {
    from: tokens[startIndex]!.from,
    to: tokens[endIndex]!.to,
  };
}

function shouldRevealInlineMarker(
  tokens: readonly InlineToken[],
  markerIndex: number,
  lineFrom: number,
  activeSelection?: ActiveSelection,
): boolean {
  const range = markerRevealComponentRange(tokens, markerIndex);
  if (!range) return false;
  return selectionTouchesInlineRange(activeSelection, lineFrom + range.from, lineFrom + range.to);
}

function collectInlineDecorations(
  lineFrom: number,
  tokens: readonly InlineToken[],
  activeSelection?: ActiveSelection,
): PendingDecoration[] {
  const pending: PendingDecoration[] = [];
  for (let index = 0; index < tokens.length; index++) {
    const token = tokens[index]!;
    const from = lineFrom + token.from;
    const to = lineFrom + token.to;
    switch (token.type) {
      case "strong":
        pending.push({ from, to, decoration: decStrong });
        break;
      case "emphasis":
        pending.push({ from, to, decoration: decEmphasis });
        break;
      case "strikethrough":
        pending.push({ from, to, decoration: decStrike });
        break;
      case "code":
        pending.push({ from, to, decoration: decCode });
        break;
      case "code-marker":
        pending.push({
          from,
          to,
          decoration: shouldRevealInlineMarker(tokens, index, lineFrom, activeSelection)
            ? decCodeMarker
            : decHiddenMarkdownToken,
        });
        break;
      case "link-text":
        pending.push({ from, to, decoration: decLinkText });
        break;
      case "link-url":
        pending.push({ from, to, decoration: decLinkUrl });
        break;
      case "link-marker":
        pending.push({
          from,
          to,
          decoration: shouldRevealInlineMarker(tokens, index, lineFrom, activeSelection)
            ? decLinkMarker
            : decHiddenMarkdownToken,
        });
        break;
    }
  }
  return pending;
}

interface TextRange {
  from: number;
  to: number;
}

export interface VariableMatcher {
  findAll(text: string): TextRange[];
}

const EMPTY_MATCHER: VariableMatcher = { findAll: () => [] };

function escapeRegExp(s: string): string {
  return s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

export function createVariableMatcher(
  variables: readonly Pick<VariableIndexEntry, "normalized">[],
): VariableMatcher {
  if (variables.length === 0) return EMPTY_MATCHER;

  const names = variables
    .map((v) => v.normalized.trim().toLowerCase())
    .filter((n) => n.length > 0);
  if (names.length === 0) return EMPTY_MATCHER;

  // Longest-first so the regex engine's leftmost-first alternation picks the
  // longest match at each position (mirrors the Rust backend).
  names.sort((a, b) => b.length - a.length || (a < b ? -1 : a > b ? 1 : 0));

  const pattern = `\\b(?:${names.map(escapeRegExp).join("|")})\\b`;
  const regex = new RegExp(pattern, "gi");

  return {
    findAll(text: string): TextRange[] {
      if (text.length === 0) return [];
      const ranges: TextRange[] = [];
      regex.lastIndex = 0;
      let m: RegExpExecArray | null;
      while ((m = regex.exec(text)) !== null) {
        if (m[0].length === 0) {
          regex.lastIndex++;
          continue;
        }
        ranges.push({ from: m.index, to: m.index + m[0].length });
      }
      return ranges;
    },
  };
}

export function findVariableNameRanges(
  text: string,
  variables: readonly Pick<VariableIndexEntry, "normalized">[],
): TextRange[] {
  return createVariableMatcher(variables).findAll(text);
}

export interface VisibleLineSpan {
  fromLine: number;
  toLine: number;
}

interface ActiveSelection {
  from: number;
  to: number;
  empty: boolean;
}

export interface MarkdownDecorationBuildOptions {
  getFenceStateBeforeLine?: (lineNumber: number) => FenceState;
  variableMatcher?: VariableMatcher;
}

class VariableMatcherCache {
  private source: readonly Pick<VariableIndexEntry, "normalized">[] | null = null;
  private matcher: VariableMatcher = EMPTY_MATCHER;

  get(variables: readonly Pick<VariableIndexEntry, "normalized">[]): VariableMatcher {
    if (this.source === variables) return this.matcher;
    this.source = variables;
    this.matcher = createVariableMatcher(variables);
    return this.matcher;
  }
}

function cloneFenceState(state: FenceState): FenceState {
  return {
    inCodeBlock: state.inCodeBlock,
    codeFenceLang: state.codeFenceLang,
  };
}

function clampedLineNumber(doc: Text, lineNumber: number): number {
  const maxLine = doc.lines + 1;
  return Math.min(maxLine, Math.max(1, Math.floor(lineNumber)));
}

function fallbackFenceStateBeforeLine(doc: Text, lineNumber: number): FenceState {
  const targetLine = clampedLineNumber(doc, lineNumber);
  if (targetLine <= 1) return cloneFenceState(DEFAULT_FENCE_STATE);
  const lines: string[] = [];
  for (let lineNo = 1; lineNo < targetLine; lineNo++) {
    lines.push(doc.line(lineNo).text);
  }
  if (lines.length === 0) return cloneFenceState(DEFAULT_FENCE_STATE);
  const analysis = markdownAnalyzeLines(lines, DEFAULT_FENCE_STATE);
  return {
    inCodeBlock: analysis.finalInCodeBlock,
    codeFenceLang: analysis.finalCodeFenceLang,
  };
}

export class FenceCheckpointCache {
  private readonly interval: number;
  private checkpoints = new Map<number, FenceState>();

  constructor(interval = FENCE_CHECKPOINT_INTERVAL) {
    this.interval = Math.max(16, Math.floor(interval));
    this.reset();
  }

  reset() {
    this.checkpoints = new Map<number, FenceState>([
      [1, cloneFenceState(DEFAULT_FENCE_STATE)],
    ]);
  }

  invalidateFromLine(lineNumber: number) {
    const minLine = Math.max(1, Math.floor(lineNumber));
    for (const line of [...this.checkpoints.keys()]) {
      if (line >= minLine && line !== 1) this.checkpoints.delete(line);
    }
  }

  getStateBeforeLine(doc: Text, lineNumber: number): FenceState {
    const targetLine = clampedLineNumber(doc, lineNumber);
    if (targetLine <= 1) return cloneFenceState(DEFAULT_FENCE_STATE);

    let checkpointLine = 1;
    for (const line of this.checkpoints.keys()) {
      if (line <= targetLine && line > checkpointLine) checkpointLine = line;
    }
    let state = cloneFenceState(
      this.checkpoints.get(checkpointLine) ?? DEFAULT_FENCE_STATE,
    );

    while (checkpointLine < targetLine) {
      const nextCheckpointLine = Math.min(targetLine, checkpointLine + this.interval);
      const chunkLines: string[] = [];
      for (let lineNo = checkpointLine; lineNo < nextCheckpointLine; lineNo++) {
        if (lineNo < 1 || lineNo > doc.lines) break;
        chunkLines.push(doc.line(lineNo).text);
      }

      if (chunkLines.length > 0) {
        const analysis = markdownAnalyzeLines(chunkLines, state);
        state = {
          inCodeBlock: analysis.finalInCodeBlock,
          codeFenceLang: analysis.finalCodeFenceLang,
        };
      }
      this.checkpoints.set(nextCheckpointLine, cloneFenceState(state));
      checkpointLine = nextCheckpointLine;
    }

    return state;
  }
}

export function buildMarkdownDecorationsForSpans(
  doc: Text,
  spans: readonly VisibleLineSpan[],
  variableIndex: readonly Pick<VariableIndexEntry, "normalized">[],
  activeSelection?: ActiveSelection,
  options: MarkdownDecorationBuildOptions = {},
): DecorationSet {
  if (spans.length === 0) return Decoration.none;

  const sortedSpans = [...spans].sort(
    (a, b) => a.fromLine - b.fromLine || a.toLine - b.toLine,
  );
  const builder = new RangeSetBuilder<Decoration>();
  const matcher = options.variableMatcher ?? createVariableMatcher(variableIndex);

  for (const span of sortedSpans) {
    const fromLine = Math.max(1, span.fromLine);
    const toLine = Math.min(doc.lines, span.toLine);
    if (fromLine > toLine) continue;

    const chunkLines: string[] = [];
    for (let lineNo = fromLine; lineNo <= toLine; lineNo++) {
      chunkLines.push(doc.line(lineNo).text);
    }
    const fenceState = options.getFenceStateBeforeLine
      ? cloneFenceState(options.getFenceStateBeforeLine(fromLine))
      : fallbackFenceStateBeforeLine(doc, fromLine);
    const analysis = markdownAnalyzeLines(chunkLines, fenceState);

    for (let idx = 0; idx < analysis.lines.length; idx++) {
      const lineNo = fromLine + idx;
      if (lineNo > toLine) break;
      const line = doc.line(lineNo);
      const lineAnalysis = analysis.lines[idx];
      if (!lineAnalysis) continue;
      const info = lineAnalysis.info;

      if (info.isCodeFence) {
        builder.add(line.from, line.from, decCodeFenceLine);
        builder.add(line.from, line.to, decFenceToken);
        continue;
      }

      if (lineAnalysis.inCodeBlock) {
        builder.add(line.from, line.from, decCodeBlockLine);
        addCodeSyntaxDecorations(builder, line.from, lineAnalysis.codeTokens);
        continue;
      }

      decorateContentLine(
        builder,
        line,
        info,
        matcher,
        lineAnalysis.inlineTokens,
        activeSelection,
      );
    }
  }

  return builder.finish();
}

function mergeLineSpans(spans: readonly VisibleLineSpan[]): VisibleLineSpan[] {
  if (spans.length <= 1) return [...spans];
  const sorted = [...spans].sort((a, b) => a.fromLine - b.fromLine || a.toLine - b.toLine);
  const merged: VisibleLineSpan[] = [];
  for (const span of sorted) {
    const last = merged[merged.length - 1];
    if (!last || span.fromLine > last.toLine + 1) {
      merged.push({ fromLine: span.fromLine, toLine: span.toLine });
      continue;
    }
    last.toLine = Math.max(last.toLine, span.toLine);
  }
  return merged;
}

function expandedVisibleSpans(
  view: EditorView,
  marginLines = VIEWPORT_MARGIN_LINES,
): VisibleLineSpan[] {
  const doc = view.state.doc;
  if (view.visibleRanges.length === 0) return [];
  const expanded = view.visibleRanges.map(({ from, to }) => ({
    fromLine: Math.max(1, doc.lineAt(from).number - marginLines),
    toLine: Math.min(doc.lines, doc.lineAt(to).number + marginLines),
  }));
  return mergeLineSpans(expanded);
}

function buildMarkdownDecorations(
  view: EditorView,
  fenceCache: FenceCheckpointCache,
  matcherCache: VariableMatcherCache,
  marginLines = VIEWPORT_MARGIN_LINES,
): DecorationSet {
  const doc = view.state.doc;
  const variableIndex = view.state.field(variableIndexField, false) ?? [];
  const selection = view.state.selection.main;
  const spans = expandedVisibleSpans(view, marginLines);
  const matcher = matcherCache.get(variableIndex);
  return buildMarkdownDecorationsForSpans(doc, spans, variableIndex, {
    from: selection.from,
    to: selection.to,
    empty: selection.empty,
  }, {
    getFenceStateBeforeLine: (lineNumber) =>
      fenceCache.getStateBeforeLine(doc, lineNumber),
    variableMatcher: matcher,
  });
}

function lineChecklistRevealRanges(
  line: { from: number; text: string },
): Array<{ from: number; to: number }> {
  const info = markdownClassifyLine(line.text);
  if (info.checklistMarkerStart === null || info.checklistMarkerEnd === null) return [];

  const ranges: Array<{ from: number; to: number }> = [
    {
      from: line.from + info.checklistMarkerStart,
      to: line.from + info.checklistMarkerEnd,
    },
  ];

  if (info.listMarkerEnd !== null && info.listMarkerEnd === info.checklistMarkerStart) {
    const beforeChecklist = line.text.slice(0, info.checklistMarkerStart);
    const markerMatch = beforeChecklist.match(/^(\s*)(->|[-*+])\s+$/);
    if (markerMatch) {
      const indentLen = (markerMatch[1] ?? "").length;
      const prefixFrom = line.from + indentLen;
      const prefixTo = line.from + info.checklistMarkerStart;
      if (prefixFrom < prefixTo) {
        ranges.push({ from: prefixFrom, to: prefixTo });
      }
    }
  }
  return ranges;
}

function selectionIntersectsChecklistReveal(
  doc: Text,
  selection: ActiveSelection,
): boolean {
  if (!selection.empty) return true;
  const line = doc.lineAt(selection.from);
  const ranges = lineChecklistRevealRanges(line);
  for (const range of ranges) {
    if (selection.from >= range.from && selection.from <= range.to) return true;
  }
  return false;
}

function selectionTouchesRange(
  selection: ActiveSelection | undefined,
  from: number,
  to: number,
): boolean {
  if (!selection) return false;
  if (selection.empty) {
    return selection.from >= from && selection.from <= to;
  }
  return selection.from < to && from < selection.to;
}

function selectionTouchesInlineRange(
  selection: ActiveSelection | undefined,
  from: number,
  to: number,
): boolean {
  if (!selection) return false;
  if (selection.empty) {
    return selection.from >= from && selection.from < to;
  }
  return selection.from < to && from < selection.to;
}

interface UnorderedListMarkerSymbolRange extends TextRange {
  arrow: boolean;
}

function unorderedListMarkerSymbolRange(
  text: string,
): UnorderedListMarkerSymbolRange | null {
  const match = text.match(/^(\s*)(->|[-*+])(\s+)/);
  if (!match) return null;
  const indentLen = (match[1] ?? "").length;
  const marker = match[2] ?? "";
  if (marker.length === 0) return null;
  return {
    from: indentLen,
    to: indentLen + marker.length,
    arrow: marker === "->",
  };
}

function lineHasTransparentMarkdownSyntax(text: string): boolean {
  if (/^\s*#{1,6}\s/.test(text)) return true;
  if (/^\s*>/.test(text)) return true;
  if (text.includes("`")) return true;
  if (text.includes("~~")) return true;
  if (text.includes("**") || text.includes("__")) return true;
  if (text.includes("[") && text.includes("](") && text.includes(")")) return true;
  if (/\*[^*\s][^*]*\*/.test(text)) return true;
  return /_[^_\s][^_]*_/.test(text);
}

function selectionIntersectsTransparentMarkdownReveal(
  doc: Text,
  selection: ActiveSelection,
): boolean {
  if (!selection.empty) return true;
  const line = doc.lineAt(selection.from);
  return lineHasTransparentMarkdownSyntax(line.text);
}

function inlineRevealComponentSignatureForCursor(
  lineText: string,
  cursorOffsetInLine: number,
): string {
  const tokens = markdownFindInlineTokens(lineText);
  const seen = new Set<string>();
  for (let index = 0; index < tokens.length; index++) {
    const token = tokens[index];
    if (!token || !isInlineMarkerToken(token)) continue;
    const range = markerRevealComponentRange(tokens, index);
    if (!range) continue;
    const signature = `${range.from}-${range.to}`;
    if (seen.has(signature)) continue;
    seen.add(signature);
    if (cursorOffsetInLine >= range.from && cursorOffsetInLine < range.to) {
      return signature;
    }
  }
  return "";
}

function emptySelectionRevealSignature(
  doc: Text,
  selection: ActiveSelection,
): string | null {
  if (!selection.empty) return null;
  const line = doc.lineAt(selection.from);
  const checklistReveal = lineChecklistRevealRanges(line).some((range) =>
    selection.from >= range.from && selection.from <= range.to
  );
  const cursorOffsetInLine = selection.from - line.from;
  const inlineSignature = inlineRevealComponentSignatureForCursor(
    line.text,
    cursorOffsetInLine,
  );
  return `${line.number}:${checklistReveal ? "1" : "0"}:${inlineSignature}`;
}

function decorateContentLine(
  builder: RangeSetBuilder<Decoration>,
  line: { from: number; to: number; text: string },
  info: MarkdownLineInfo,
  matcher: VariableMatcher,
  inlineTokens: readonly InlineToken[],
  activeSelection?: ActiveSelection,
): void {
  const revealLinePrefixSyntax = selectionTouchesRange(activeSelection, line.from, line.to);

  if (info.headingLevel) {
    builder.add(line.from, line.from, decHeadingLine[info.headingLevel - 1]);
    if (info.headingMarkerEnd) {
      const markerTo = line.from + info.headingMarkerEnd;
      builder.add(
        line.from,
        markerTo,
        revealLinePrefixSyntax ? decHeadingToken : decHiddenMarkdownToken,
      );
      builder.add(line.from + info.headingMarkerEnd, line.to, decHeadingContent);
    }
  }

  if (info.quoteMarkerEnd) {
    builder.add(line.from, line.from, decQuoteLine);
    const markerTo = line.from + info.quoteMarkerEnd;
    builder.add(
      line.from,
      markerTo,
      revealLinePrefixSyntax ? decQuoteToken : decHiddenMarkdownToken,
    );
  }

  if (info.listMarkerEnd && info.checklistMarkerStart === null) {
    builder.add(line.from, line.from, decListLine);
    const markerPrefixTo = line.from + info.listMarkerEnd;
    const unorderedMarker = unorderedListMarkerSymbolRange(line.text);
    if (unorderedMarker) {
      const markerFrom = line.from + unorderedMarker.from;
      const markerTo = line.from + unorderedMarker.to;
      const revealUnorderedSource = selectionTouchesRange(
        activeSelection,
        markerFrom,
        markerTo,
      );
      if (line.from < markerFrom) {
        builder.add(line.from, markerFrom, decListToken);
      }
      if (revealUnorderedSource) {
        builder.add(markerFrom, markerTo, decListToken);
      } else {
        builder.add(
          markerFrom,
          markerTo,
          unorderedMarker.arrow ? decUnorderedListArrow : decUnorderedListBullet,
        );
      }
      if (markerTo < markerPrefixTo) {
        builder.add(markerTo, markerPrefixTo, decListToken);
      }
    } else {
      builder.add(line.from, markerPrefixTo, decListToken);
    }
  }

  if (info.checklistMarkerStart !== null && info.checklistMarkerEnd !== null) {
    try {
      builder.add(line.from, line.from, decChecklistLine);
      let hiddenPrefixRange: TextRange | null = null;

      // Hide unordered checklist list markers ("- ", "* ", "+ ", "-> ")
      // while keeping indentation and source text intact.
      if (info.listMarkerEnd !== null && info.listMarkerEnd === info.checklistMarkerStart) {
        const beforeChecklist = line.text.slice(0, info.checklistMarkerStart);
        const markerMatch = beforeChecklist.match(/^(\s*)(->|[-*+])\s+$/);
        if (markerMatch) {
          const indentLen = (markerMatch[1] ?? "").length;
          const prefixFrom = line.from + indentLen;
          const prefixTo = line.from + info.checklistMarkerStart;
          if (prefixFrom < prefixTo) {
            hiddenPrefixRange = { from: prefixFrom, to: prefixTo };
          }
        }
      }

      const markerFrom = line.from + info.checklistMarkerStart;
      const markerTo = line.from + info.checklistMarkerEnd;
      const revealChecklistSyntax =
        selectionTouchesRange(activeSelection, markerFrom, markerTo) ||
        (hiddenPrefixRange !== null &&
          selectionTouchesRange(activeSelection, hiddenPrefixRange.from, hiddenPrefixRange.to));

      if (!revealChecklistSyntax && hiddenPrefixRange !== null) {
        builder.add(hiddenPrefixRange.from, hiddenPrefixRange.to, decChecklistHiddenPrefix);
      }

      if (!revealChecklistSyntax && markerFrom < markerTo) {
        builder.add(
          markerFrom,
          markerTo,
          info.checklistChecked ? decChecklistMarkChecked : decChecklistMark,
        );
      }
      if (info.checklistChecked && info.checklistContentStart !== null) {
        const contentFrom = line.from + info.checklistContentStart;
        if (contentFrom < line.to) {
          builder.add(contentFrom, line.to, decChecklistDoneContent);
        }
      }
    } catch (error) {
      console.error("Checklist decoration failed, skipping checklist render for line:", error);
    }
  }

  if (info.isHorizontalRule) {
    builder.add(line.from, line.from, decRuleLine);
    builder.add(line.from, line.to, decRuleToken);
  }

  const pending: PendingDecoration[] = [];
  for (const range of matcher.findAll(line.text)) {
    pending.push({
      from: line.from + range.from,
      to: line.from + range.to,
      decoration: decVariable,
    });
  }
  for (const inline of collectInlineDecorations(line.from, inlineTokens, activeSelection)) {
    pending.push(inline);
  }
  pending.sort((a, b) => a.from - b.from || a.to - b.to);
  for (const entry of pending) {
    builder.add(entry.from, entry.to, entry.decoration);
  }
}

const markdownWasmReadyAnnotation = Annotation.define<boolean>();
const markdownDeferredRefreshAnnotation = Annotation.define<boolean>();

function earliestChangedLine(update: ViewUpdate): number {
  let earliest = Number.POSITIVE_INFINITY;
  update.changes.iterChangedRanges((_fromA, _toA, fromB) => {
    earliest = Math.min(earliest, fromB);
  });
  if (!Number.isFinite(earliest)) return 1;
  return update.state.doc.lineAt(earliest).number;
}

// Min of the pre-change affected positions + min of the post-change affected
// positions. Used to decide whether every edit sits strictly below the
// current viewport.
function earliestChangedPos(update: ViewUpdate): number {
  let earliest = Number.POSITIVE_INFINITY;
  update.changes.iterChangedRanges((fromA, _toA, fromB) => {
    earliest = Math.min(earliest, fromA, fromB);
  });
  return Number.isFinite(earliest) ? earliest : 0;
}

function viewportEndPos(view: EditorView): number {
  const ranges = view.visibleRanges;
  if (ranges.length === 0) return 0;
  let max = 0;
  for (const range of ranges) {
    if (range.to > max) max = range.to;
  }
  return max;
}

function allChangesBelowViewport(update: ViewUpdate): boolean {
  const endPos = viewportEndPos(update.view);
  if (endPos === 0) return false;
  return earliestChangedPos(update) > endPos;
}

const markdownRichPlugin = ViewPlugin.fromClass(
  class {
    decorations: DecorationSet;
    private destroyed = false;
    private readonly fenceCache = new FenceCheckpointCache();
    private readonly matcherCache = new VariableMatcherCache();
    private pendingRefreshTimer: ReturnType<typeof setTimeout> | null = null;

    constructor(view: EditorView) {
      this.decorations = this.safeBuild(view, Decoration.none, VIEWPORT_MARGIN_LINES);
      void ensureWasmReady()
        .then(() => {
          if (this.destroyed) return;
          view.dispatch({ annotations: markdownWasmReadyAnnotation.of(true) });
        })
        .catch((error) => {
          console.error("Markdown wasm init failed:", error);
        });
    }

    private scheduleDeferredRefresh(view: EditorView) {
      if (this.pendingRefreshTimer !== null) {
        clearTimeout(this.pendingRefreshTimer);
      }
      this.pendingRefreshTimer = setTimeout(() => {
        this.pendingRefreshTimer = null;
        if (this.destroyed) return;
        view.dispatch({ annotations: markdownDeferredRefreshAnnotation.of(true) });
      }, 90);
    }

    update(update: ViewUpdate) {
      if (
        update.transactions.some((transaction) =>
          transaction.annotation(markdownWasmReadyAnnotation),
        )
      ) {
        this.decorations = this.safeBuild(
          update.view,
          this.decorations,
          VIEWPORT_MARGIN_LINES,
        );
        return;
      }

      if (
        update.transactions.some((transaction) =>
          transaction.annotation(markdownDeferredRefreshAnnotation),
        )
      ) {
        this.decorations = this.safeBuild(
          update.view,
          this.decorations,
          HOTPATH_REBUILD_MARGIN_LINES,
        );
        return;
      }

      if (update.docChanged) {
        this.fenceCache.invalidateFromLine(earliestChangedLine(update));
      }

      const prevVars = update.startState.field(variableIndexField, false) ?? [];
      const nextVars = update.state.field(variableIndexField, false) ?? [];
      const varsChanged = prevVars !== nextVars;

      if (varsChanged || update.viewportChanged) {
        this.decorations = this.safeBuild(
          update.view,
          this.decorations,
          VIEWPORT_MARGIN_LINES,
        );
        return;
      }

      if (update.selectionSet && !update.docChanged) {
        const prevSelection: ActiveSelection = {
          from: update.startState.selection.main.from,
          to: update.startState.selection.main.to,
          empty: update.startState.selection.main.empty,
        };
        const nextSelection: ActiveSelection = {
          from: update.state.selection.main.from,
          to: update.state.selection.main.to,
          empty: update.state.selection.main.empty,
        };
        const prevLineNo = update.startState.doc.lineAt(prevSelection.from).number;
        const nextLineNo = update.state.doc.lineAt(nextSelection.from).number;
        if (prevLineNo === nextLineNo) {
          const prevSig = emptySelectionRevealSignature(update.startState.doc, prevSelection);
          const nextSig = emptySelectionRevealSignature(update.state.doc, nextSelection);
          if (prevSig !== null && prevSig === nextSig) return;
        }
        const affectsChecklist =
          selectionIntersectsChecklistReveal(update.startState.doc, prevSelection)
          || selectionIntersectsChecklistReveal(update.state.doc, nextSelection);
        const affectsTransparentMarkdown =
          selectionIntersectsTransparentMarkdownReveal(update.startState.doc, prevSelection)
          || selectionIntersectsTransparentMarkdownReveal(update.state.doc, nextSelection);
        if (!affectsChecklist && !affectsTransparentMarkdown) return;
        this.decorations = this.safeBuild(
          update.view,
          this.decorations,
          HOTPATH_REBUILD_MARGIN_LINES,
        );
        return;
      }

      if (!update.docChanged) return;
      // If every edit lands strictly below the expanded viewport, the
      // viewport's text and fence state are both unchanged and CodeMirror
      // auto-maps the existing decorations through the transaction.
      if (allChangesBelowViewport(update)) return;
      // Keep typing responsive by mapping existing decorations immediately
      // and coalescing an actual rebuild after input settles.
      this.decorations = this.decorations.map(update.changes);
      this.scheduleDeferredRefresh(update.view);
    }

    private safeBuild(
      view: EditorView,
      fallback: DecorationSet,
      marginLines: number,
    ): DecorationSet {
      try {
        return buildMarkdownDecorations(
          view,
          this.fenceCache,
          this.matcherCache,
          marginLines,
        );
      } catch (error) {
        console.error("Markdown decoration build failed:", error);
        return fallback;
      }
    }

    destroy() {
      this.destroyed = true;
      if (this.pendingRefreshTimer !== null) {
        clearTimeout(this.pendingRefreshTimer);
        this.pendingRefreshTimer = null;
      }
    }
  },
  {
    decorations: (plugin) => plugin.decorations,
  },
);

export function markdownRichTextExtensions() {
  return [markdownRichPlugin];
}
