import { RangeSetBuilder } from "@codemirror/state";
import { Decoration, EditorView, ViewPlugin } from "@codemirror/view";
import type { DecorationSet, ViewUpdate } from "@codemirror/view";

type InlineTokenType =
  | "strong"
  | "emphasis"
  | "strikethrough"
  | "code"
  | "code-marker"
  | "link-text"
  | "link-url"
  | "link-marker";

interface InlineToken {
  from: number;
  to: number;
  type: InlineTokenType;
}

interface ProtectedRange {
  from: number;
  to: number;
}

export interface MarkdownLineInfo {
  headingLevel: number | null;
  headingMarkerEnd: number | null;
  quoteMarkerEnd: number | null;
  listMarkerEnd: number | null;
  isHorizontalRule: boolean;
  isCodeFence: boolean;
}

const headingRe = /^(#{1,6})\s+/;
const quoteRe = /^(\s*>+)\s+/;
const listRe = /^(\s*)([-*+]|\d+\.)\s+/;
const hrRe = /^\s*(([-*_])\s*){3,}$/;
const fenceRe = /^\s*```/;

const codeSpanRe = /(`+)([^`]+?)\1/g;
const linkRe = /\[([^\]\n]+)\]\(([^)\n]+)\)/g;
const strongRe = /(\*\*|__)(?=\S)(.+?)(?<=\S)\1/g;
const strikeRe = /~~(?=\S)(.+?)(?<=\S)~~/g;
const emStarRe = /(^|[^*])\*(?=\S)([^*\n]+?)(?<=\S)\*/g;
const emUnderscoreRe = /(^|[^_])_(?=\S)([^_\n]+?)(?<=\S)_/g;

const decHeadingToken = Decoration.mark({ class: "md-token md-token-heading" });
const decQuoteToken = Decoration.mark({ class: "md-token md-token-quote" });
const decListToken = Decoration.mark({ class: "md-token md-token-list" });
const decRuleToken = Decoration.mark({ class: "md-token md-token-rule" });
const decFenceToken = Decoration.mark({ class: "md-token md-token-code-fence" });
const decHeadingContent = Decoration.mark({ class: "md-heading-content" });
const decStrong = Decoration.mark({ class: "md-strong" });
const decEmphasis = Decoration.mark({ class: "md-emphasis" });
const decStrike = Decoration.mark({ class: "md-strike" });
const decCode = Decoration.mark({ class: "md-inline-code-content" });
const decCodeMarker = Decoration.mark({ class: "md-token md-token-code" });
const decLinkText = Decoration.mark({ class: "md-link-text" });
const decLinkUrl = Decoration.mark({ class: "md-link-url" });
const decLinkMarker = Decoration.mark({ class: "md-token md-token-link" });

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
const decRuleLine = lineClass("md-line md-hr");
const decCodeFenceLine = lineClass("md-line md-code-fence");
const decCodeBlockLine = lineClass("md-line md-code-block-line");

function overlaps(ranges: ProtectedRange[], from: number, to: number): boolean {
  return ranges.some((range) => from < range.to && to > range.from);
}

function protect(ranges: ProtectedRange[], from: number, to: number) {
  if (to > from) {
    ranges.push({ from, to });
  }
}

function pushToken(tokens: InlineToken[], from: number, to: number, type: InlineTokenType) {
  if (to > from) {
    tokens.push({ from, to, type });
  }
}

export function classifyMarkdownLine(text: string): MarkdownLineInfo {
  const headingMatch = text.match(headingRe);
  const quoteMatch = text.match(quoteRe);
  const listMatch = text.match(listRe);
  return {
    headingLevel: headingMatch ? headingMatch[1].length : null,
    headingMarkerEnd: headingMatch ? headingMatch[0].length : null,
    quoteMarkerEnd: quoteMatch ? quoteMatch[0].length : null,
    listMarkerEnd: listMatch ? listMatch[0].length : null,
    isHorizontalRule: hrRe.test(text),
    isCodeFence: fenceRe.test(text),
  };
}

export function findInlineMarkdownTokens(text: string): InlineToken[] {
  const tokens: InlineToken[] = [];
  const protectedRanges: ProtectedRange[] = [];

  for (const match of text.matchAll(codeSpanRe)) {
    const start = match.index ?? 0;
    const markerLen = match[1].length;
    const end = start + match[0].length;
    pushToken(tokens, start, start + markerLen, "code-marker");
    pushToken(tokens, start + markerLen, end - markerLen, "code");
    pushToken(tokens, end - markerLen, end, "code-marker");
    protect(protectedRanges, start, end);
  }

  for (const match of text.matchAll(linkRe)) {
    const start = match.index ?? 0;
    const end = start + match[0].length;
    if (overlaps(protectedRanges, start, end)) continue;

    const textStart = start + 1;
    const textEnd = textStart + match[1].length;
    const urlStart = textEnd + 2;
    const urlEnd = urlStart + match[2].length;

    pushToken(tokens, start, start + 1, "link-marker");
    pushToken(tokens, textStart, textEnd, "link-text");
    pushToken(tokens, textEnd, textEnd + 2, "link-marker");
    pushToken(tokens, urlStart, urlEnd, "link-url");
    pushToken(tokens, urlEnd, end, "link-marker");
    protect(protectedRanges, start, end);
  }

  for (const match of text.matchAll(strongRe)) {
    const start = match.index ?? 0;
    const markerLen = match[1].length;
    const end = start + match[0].length;
    if (overlaps(protectedRanges, start, end)) continue;

    pushToken(tokens, start, start + markerLen, "code-marker");
    pushToken(tokens, start + markerLen, end - markerLen, "strong");
    pushToken(tokens, end - markerLen, end, "code-marker");
    protect(protectedRanges, start, end);
  }

  for (const match of text.matchAll(strikeRe)) {
    const start = match.index ?? 0;
    const end = start + match[0].length;
    if (overlaps(protectedRanges, start, end)) continue;

    pushToken(tokens, start, start + 2, "code-marker");
    pushToken(tokens, start + 2, end - 2, "strikethrough");
    pushToken(tokens, end - 2, end, "code-marker");
    protect(protectedRanges, start, end);
  }

  for (const match of text.matchAll(emStarRe)) {
    const prefixLen = match[1].length;
    const start = (match.index ?? 0) + prefixLen;
    const end = start + match[0].length - prefixLen;
    if (overlaps(protectedRanges, start, end)) continue;

    pushToken(tokens, start, start + 1, "code-marker");
    pushToken(tokens, start + 1, end - 1, "emphasis");
    pushToken(tokens, end - 1, end, "code-marker");
  }

  for (const match of text.matchAll(emUnderscoreRe)) {
    const prefixLen = match[1].length;
    const start = (match.index ?? 0) + prefixLen;
    const end = start + match[0].length - prefixLen;
    if (overlaps(protectedRanges, start, end)) continue;

    pushToken(tokens, start, start + 1, "code-marker");
    pushToken(tokens, start + 1, end - 1, "emphasis");
    pushToken(tokens, end - 1, end, "code-marker");
  }

  tokens.sort((a, b) => (a.from === b.from ? a.to - b.to : a.from - b.from));
  return tokens;
}

function addInlineDecorations(builder: RangeSetBuilder<Decoration>, lineFrom: number, text: string) {
  for (const token of findInlineMarkdownTokens(text)) {
    const from = lineFrom + token.from;
    const to = lineFrom + token.to;
    switch (token.type) {
      case "strong":
        builder.add(from, to, decStrong);
        break;
      case "emphasis":
        builder.add(from, to, decEmphasis);
        break;
      case "strikethrough":
        builder.add(from, to, decStrike);
        break;
      case "code":
        builder.add(from, to, decCode);
        break;
      case "code-marker":
        builder.add(from, to, decCodeMarker);
        break;
      case "link-text":
        builder.add(from, to, decLinkText);
        break;
      case "link-url":
        builder.add(from, to, decLinkUrl);
        break;
      case "link-marker":
        builder.add(from, to, decLinkMarker);
        break;
    }
  }
}

function buildMarkdownDecorations(view: EditorView): DecorationSet {
  const builder = new RangeSetBuilder<Decoration>();
  let inCodeBlock = false;

  for (let lineNo = 1; lineNo <= view.state.doc.lines; lineNo++) {
    const line = view.state.doc.line(lineNo);
    const info = classifyMarkdownLine(line.text);

    if (info.isCodeFence) {
      builder.add(line.from, line.from, decCodeFenceLine);
      builder.add(line.from, line.to, decFenceToken);
      inCodeBlock = !inCodeBlock;
      continue;
    }

    if (inCodeBlock) {
      builder.add(line.from, line.from, decCodeBlockLine);
      continue;
    }

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

    if (info.listMarkerEnd) {
      builder.add(line.from, line.from, decListLine);
      builder.add(line.from, line.from + info.listMarkerEnd, decListToken);
    }

    if (info.isHorizontalRule) {
      builder.add(line.from, line.from, decRuleLine);
      builder.add(line.from, line.to, decRuleToken);
    }

    addInlineDecorations(builder, line.from, line.text);
  }

  return builder.finish();
}

const markdownRichPlugin = ViewPlugin.fromClass(
  class {
    decorations: DecorationSet;

    constructor(view: EditorView) {
      this.decorations = buildMarkdownDecorations(view);
    }

    update(update: ViewUpdate) {
      if (update.docChanged || update.viewportChanged) {
        this.decorations = buildMarkdownDecorations(update.view);
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
