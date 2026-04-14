import { Annotation, RangeSetBuilder, type Range, type Text } from "@codemirror/state";
import { Decoration, EditorView, ViewPlugin } from "@codemirror/view";
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

const decHeadingToken = Decoration.mark({ class: "md-token md-token-heading" });
const decQuoteToken = Decoration.mark({ class: "md-token md-token-quote" });
const decListToken = Decoration.mark({ class: "md-token md-token-list" });
const decRuleToken = Decoration.mark({ class: "md-token md-token-rule" });
const decFenceToken = Decoration.mark({ class: "md-token md-token-code-fence" });
const decHeadingContent = Decoration.mark({ class: "md-heading-content" });
const decChecklistToken = Decoration.mark({ class: "md-token md-checklist-token" });
const decChecklistMark = Decoration.mark({ class: "md-checklist-mark" });
const decChecklistMarkChecked = Decoration.mark({ class: "md-checklist-mark md-checklist-mark-checked" });
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

function collectInlineDecorations(
  lineFrom: number,
  tokens: readonly InlineToken[],
): PendingDecoration[] {
  const pending: PendingDecoration[] = [];
  for (const token of tokens) {
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
        pending.push({ from, to, decoration: decCodeMarker });
        break;
      case "link-text":
        pending.push({ from, to, decoration: decLinkText });
        break;
      case "link-url":
        pending.push({ from, to, decoration: decLinkUrl });
        break;
      case "link-marker":
        pending.push({ from, to, decoration: decLinkMarker });
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

export function buildMarkdownDecorationsForSpans(
  doc: Text,
  spans: readonly VisibleLineSpan[],
  variableIndex: readonly Pick<VariableIndexEntry, "normalized">[],
): DecorationSet {
  if (spans.length === 0) return Decoration.none;

  const sortedSpans = [...spans].sort(
    (a, b) => a.fromLine - b.fromLine || a.toLine - b.toLine,
  );
  const builder = new RangeSetBuilder<Decoration>();
  const fenceState = { inCodeBlock: false, codeFenceLang: null as string | null };
  const matcher = createVariableMatcher(variableIndex);
  let nextLineToProcess = 1;

  for (const span of sortedSpans) {
    if (nextLineToProcess > span.toLine) {
      nextLineToProcess = span.toLine + 1;
      continue;
    }

    const chunkLines: string[] = [];
    for (let lineNo = nextLineToProcess; lineNo <= span.toLine; lineNo++) {
      chunkLines.push(doc.line(lineNo).text);
    }
    const analysis = markdownAnalyzeLines(chunkLines, fenceState);

    const offset = span.fromLine - nextLineToProcess;
    for (let idx = Math.max(0, offset); idx < analysis.lines.length; idx++) {
      const lineNo = nextLineToProcess + idx;
      if (lineNo > span.toLine) break;
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

      decorateContentLine(builder, line, info, matcher, lineAnalysis.inlineTokens);
    }
    fenceState.inCodeBlock = analysis.finalInCodeBlock;
    fenceState.codeFenceLang = analysis.finalCodeFenceLang;
    nextLineToProcess = span.toLine + 1;
  }

  return builder.finish();
}

function buildMarkdownDecorations(view: EditorView): DecorationSet {
  const doc = view.state.doc;
  const variableIndex = view.state.field(variableIndexField, false) ?? [];
  const spans: VisibleLineSpan[] = view.visibleRanges.map(({ from, to }) => ({
    fromLine: doc.lineAt(from).number,
    toLine: doc.lineAt(to).number,
  }));
  return buildMarkdownDecorationsForSpans(doc, spans, variableIndex);
}

function decorateContentLine(
  builder: RangeSetBuilder<Decoration>,
  line: { from: number; to: number; text: string },
  info: MarkdownLineInfo,
  matcher: VariableMatcher,
  inlineTokens: readonly InlineToken[],
): void {
  if (info.headingLevel) {
    builder.add(line.from, line.from, decHeadingLine[info.headingLevel - 1]);
    if (info.headingMarkerEnd) {
      builder.add(line.from, line.from + info.headingMarkerEnd, decHeadingToken);
      builder.add(line.from + info.headingMarkerEnd, line.to, decHeadingContent);
    }
  }

  if (info.quoteMarkerEnd) {
    builder.add(line.from, line.from, decQuoteLine);
    builder.add(line.from, line.from + info.quoteMarkerEnd, decQuoteToken);
  }

  if (info.listMarkerEnd && info.checklistMarkerStart === null) {
    builder.add(line.from, line.from, decListLine);
    builder.add(line.from, line.from + info.listMarkerEnd, decListToken);
  }

  if (info.checklistMarkerStart !== null && info.checklistMarkerEnd !== null) {
    try {
      builder.add(line.from, line.from, decChecklistLine);
      const markerFrom = line.from + info.checklistMarkerStart;
      const markerTo = line.from + info.checklistMarkerEnd;
      builder.add(markerFrom, markerFrom + 1, decChecklistToken);
      builder.add(
        markerFrom + 1,
        markerTo - 1,
        info.checklistChecked ? decChecklistMarkChecked : decChecklistMark,
      );
      builder.add(markerTo - 1, markerTo, decChecklistToken);
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
  for (const inline of collectInlineDecorations(line.from, inlineTokens)) {
    pending.push(inline);
  }
  pending.sort((a, b) => a.from - b.from || a.to - b.to);
  for (const entry of pending) {
    builder.add(entry.from, entry.to, entry.decoration);
  }
}

function extractDecorationRanges(
  set: DecorationSet,
  fromPos: number,
  toPos: number,
): Range<Decoration>[] {
  const out: Range<Decoration>[] = [];
  const cursor = set.iter();
  while (cursor.value) {
    if (cursor.from >= fromPos && cursor.to <= toPos) {
      out.push(cursor.value.range(cursor.from, cursor.to));
    }
    cursor.next();
  }
  return out;
}

const markdownWasmReadyAnnotation = Annotation.define<boolean>();

const markdownRichPlugin = ViewPlugin.fromClass(
  class {
    decorations: DecorationSet;
    private destroyed = false;

    constructor(view: EditorView) {
      this.decorations = this.safeBuild(view, Decoration.none);
      void ensureWasmReady()
        .then(() => {
          if (this.destroyed) return;
          view.dispatch({ annotations: markdownWasmReadyAnnotation.of(true) });
        })
        .catch((error) => {
          console.error("Markdown wasm init failed:", error);
        });
    }

    update(update: ViewUpdate) {
      if (
        update.transactions.some((transaction) =>
          transaction.annotation(markdownWasmReadyAnnotation),
        )
      ) {
        this.decorations = this.safeBuild(update.view, this.decorations);
        return;
      }

      const prevVars = update.startState.field(variableIndexField, false) ?? [];
      const nextVars = update.state.field(variableIndexField, false) ?? [];
      const varsChanged = prevVars !== nextVars;

      if (varsChanged || update.viewportChanged) {
        this.decorations = this.safeBuild(update.view, this.decorations);
        return;
      }

      if (!update.docChanged) return;

      try {
        const incremental = this.incrementalRebuild(update);
        if (incremental) {
          this.decorations = incremental;
          return;
        }
      } catch (error) {
        console.error("Incremental markdown rebuild failed, falling back:", error);
      }
      this.decorations = this.safeBuild(update.view, this.decorations);
    }

    /**
     * Attempt a dirty-range-only rebuild. Returns null to signal the caller
     * should fall back to a full viewport rebuild.
     */
    private incrementalRebuild(update: ViewUpdate): DecorationSet | null {
      const view = update.view;
      const doc = view.state.doc;

      const visibleSpans: VisibleLineSpan[] = view.visibleRanges.map(({ from, to }) => ({
        fromLine: doc.lineAt(from).number,
        toLine: doc.lineAt(to).number,
      }));
      if (visibleSpans.length === 0) return Decoration.none;

      // Check if any pre-viewport fence marker was touched — that would flip
      // `inCodeBlock` at the viewport start and we can't recover cheaply.
      const firstVisibleLine = visibleSpans[0].fromLine;
      const lastVisibleLine = visibleSpans[visibleSpans.length - 1].toLine;
      let preFenceTouched = false;
      let fenceDirtied = false;
      const changedNewRanges: Array<{ fromB: number; toB: number }> = [];

      update.changes.iterChangedRanges((fromA, toA, fromB, toB) => {
        changedNewRanges.push({ fromB, toB });

        const prevText = update.startState.doc.sliceString(fromA, toA);
        const newText = doc.sliceString(fromB, toB);
        const changeHasFence = prevText.includes("```") || newText.includes("```");

        if (changeHasFence) {
          // Where does the change land in new coords?
          const startLineB = doc.lineAt(fromB).number;
          if (startLineB < firstVisibleLine) {
            preFenceTouched = true;
          } else {
            fenceDirtied = true;
          }
        }
      });

      if (preFenceTouched) return null;

      // Map the existing decoration set through the change so positions stay valid.
      let result = this.decorations.map(update.changes);
      const variableIndex = update.state.field(variableIndexField, false) ?? [];

      // Determine dirty line spans inside the viewport.
      const dirtyByVisibleSpan: Array<{ fromLine: number; toLine: number }> = [];
      for (const span of visibleSpans) {
        let minDirty = Infinity;
        let maxDirty = -Infinity;
        for (const { fromB, toB } of changedNewRanges) {
          const startLine = doc.lineAt(fromB).number;
          const endLine = doc.lineAt(toB).number;
          const clippedStart = Math.max(startLine, span.fromLine);
          const clippedEnd = Math.min(endLine, span.toLine);
          if (clippedStart > clippedEnd) continue;
          if (clippedStart < minDirty) minDirty = clippedStart;
          if (clippedEnd > maxDirty) maxDirty = clippedEnd;
        }
        if (minDirty === Infinity) continue;

        const toLine = fenceDirtied ? span.toLine : maxDirty;
        dirtyByVisibleSpan.push({ fromLine: minDirty, toLine });
      }

      // If a fence was dirtied somewhere in the viewport, all later visible
      // spans are also potentially affected — fall back for simplicity.
      if (fenceDirtied && dirtyByVisibleSpan.length < visibleSpans.length) {
        return null;
      }

      if (dirtyByVisibleSpan.length === 0) {
        // No changes landed inside the visible range; just keep the mapped set.
        return result;
      }

      for (const dirty of dirtyByVisibleSpan) {
        const fromPos = doc.line(dirty.fromLine).from;
        const nextLineStart =
          dirty.toLine < doc.lines ? doc.line(dirty.toLine + 1).from : doc.length + 1;
        const filterTo = nextLineStart - 1;

        const rebuilt = buildMarkdownDecorationsForSpans(doc, [dirty], variableIndex);
        const adds = extractDecorationRanges(rebuilt, fromPos, filterTo);

        result = result.update({
          filterFrom: fromPos,
          filterTo,
          filter: () => false,
          add: adds,
          sort: true,
        });
      }

      // Guard: we never touch decorations outside the current viewport, so
      // lines scrolled past lastVisibleLine keep whatever they had from the
      // mapped set. That's fine because #1 already scopes builds to viewport.
      void lastVisibleLine;
      return result;
    }

    private safeBuild(view: EditorView, fallback: DecorationSet): DecorationSet {
      try {
        return buildMarkdownDecorations(view);
      } catch (error) {
        console.error("Markdown decoration build failed:", error);
        return fallback;
      }
    }

    destroy() {
      this.destroyed = true;
    }
  },
  {
    decorations: (plugin) => plugin.decorations,
  },
);

export function markdownRichTextExtensions() {
  return [markdownRichPlugin];
}
