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

type CodeTokenType = "keyword" | "string" | "number" | "comment" | "function" | "type";

interface InlineToken {
  from: number;
  to: number;
  type: InlineTokenType;
}

interface CodeToken {
  from: number;
  to: number;
  type: CodeTokenType;
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
  checklistMarkerStart: number | null;
  checklistMarkerEnd: number | null;
  checklistContentStart: number | null;
  checklistChecked: boolean;
  isHorizontalRule: boolean;
  isCodeFence: boolean;
}

const headingRe = /^(\s*)(#{1,6})\s+/;
const quoteRe = /^(\s*>+)\s*/;
const listRe = /^(\s*)(->|[-*+]|\d+\.|\d+(?:\.\d+)+)\s+/;
const checklistRe = /^(\s*(?:->|[-*+]|\d+\.|\d+(?:\.\d+)+)\s+)(\[(?: |x|X)\])(\s+)/;
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
  const checklistMatch = text.match(checklistRe);
  const checklistPrefix = checklistMatch?.[1] ?? "";
  const checklistBox = checklistMatch?.[2] ?? "";
  const checklistSpacer = checklistMatch?.[3] ?? "";
  const checklistMarkerStart = checklistMatch ? checklistPrefix.length : null;
  const checklistMarkerEnd = checklistMatch
    ? checklistPrefix.length + checklistBox.length
    : null;
  const checklistContentStart = checklistMatch
    ? checklistPrefix.length + checklistBox.length + checklistSpacer.length
    : null;
  return {
    headingLevel: headingMatch ? headingMatch[2].length : null,
    headingMarkerEnd: headingMatch ? headingMatch[0].length : null,
    quoteMarkerEnd: quoteMatch ? quoteMatch[0].length : null,
    listMarkerEnd: listMatch ? listMatch[0].length : null,
    checklistMarkerStart,
    checklistMarkerEnd,
    checklistContentStart,
    checklistChecked: checklistMatch ? checklistBox.toLowerCase() === "[x]" : false,
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

const jsKeywords = new Set([
  "const",
  "let",
  "var",
  "function",
  "return",
  "if",
  "else",
  "for",
  "while",
  "switch",
  "case",
  "break",
  "continue",
  "import",
  "export",
  "from",
  "class",
  "extends",
  "new",
  "async",
  "await",
  "try",
  "catch",
  "finally",
  "throw",
  "typeof",
  "instanceof",
  "in",
  "of",
  "true",
  "false",
  "null",
  "undefined",
]);

const rustKeywords = new Set([
  "fn",
  "let",
  "mut",
  "pub",
  "struct",
  "enum",
  "impl",
  "trait",
  "use",
  "mod",
  "match",
  "if",
  "else",
  "for",
  "while",
  "loop",
  "return",
  "self",
  "Self",
  "crate",
  "super",
  "as",
  "where",
  "const",
  "static",
  "true",
  "false",
]);

const pythonKeywords = new Set([
  "def",
  "class",
  "return",
  "if",
  "elif",
  "else",
  "for",
  "while",
  "try",
  "except",
  "finally",
  "with",
  "import",
  "from",
  "as",
  "pass",
  "break",
  "continue",
  "yield",
  "lambda",
  "True",
  "False",
  "None",
]);

const shellKeywords = new Set([
  "if",
  "then",
  "else",
  "fi",
  "for",
  "in",
  "do",
  "done",
  "case",
  "esac",
  "while",
  "function",
  "export",
  "local",
]);

const hashCommentLangs = new Set(["py", "python", "sh", "bash", "zsh", "yaml", "yml", "toml"]);
const noCommentLangs = new Set(["json"]);

function normalizeFenceLang(raw: string | null): string | null {
  if (!raw) return null;
  const value = raw.trim().toLowerCase();
  if (!value) return null;
  if (value === "typescript") return "ts";
  if (value === "javascript") return "js";
  if (value === "shell") return "sh";
  if (value === "py") return "python";
  if (value === "rs") return "rust";
  if (value === "tsx") return "ts";
  if (value === "jsx") return "js";
  return value;
}

function parseFenceLanguage(lineText: string): string | null {
  const match = lineText.match(/^\s*```([A-Za-z0-9_+-]+)/);
  return normalizeFenceLang(match?.[1] ?? null);
}

function keywordSetForLang(lang: string | null): Set<string> {
  if (!lang) return jsKeywords;
  if (lang === "ts" || lang === "js" || lang === "go" || lang === "java" || lang === "c") {
    return jsKeywords;
  }
  if (lang === "rust") return rustKeywords;
  if (lang === "python") return pythonKeywords;
  if (lang === "sh" || lang === "bash" || lang === "zsh") return shellKeywords;
  return jsKeywords;
}

function isIdentifierChar(ch: string): boolean {
  return /[A-Za-z0-9_]/.test(ch);
}

function nextNonWhitespaceChar(text: string, from: number): string | null {
  let i = from;
  while (i < text.length && /\s/.test(text[i] ?? "")) i++;
  return i < text.length ? (text[i] ?? null) : null;
}

function pushCodeToken(tokens: CodeToken[], from: number, to: number, type: CodeTokenType): void {
  if (to > from) {
    tokens.push({ from, to, type });
  }
}

function scanStringTokens(text: string, tokens: CodeToken[], protectedRanges: ProtectedRange[]) {
  const quoteSet = new Set(["'", '"', "`"]);
  let i = 0;
  while (i < text.length) {
    const ch = text[i] ?? "";
    if (!quoteSet.has(ch)) {
      i++;
      continue;
    }

    const quote = ch;
    const start = i;
    i++;
    let escaped = false;
    while (i < text.length) {
      const next = text[i] ?? "";
      if (escaped) {
        escaped = false;
        i++;
        continue;
      }
      if (next === "\\") {
        escaped = true;
        i++;
        continue;
      }
      if (next === quote) {
        i++;
        break;
      }
      i++;
    }

    pushCodeToken(tokens, start, i, "string");
    protect(protectedRanges, start, i);
  }
}

function findCommentStart(text: string, lang: string | null, protectedRanges: ProtectedRange[]): number {
  if (lang && noCommentLangs.has(lang)) return -1;
  if (lang && hashCommentLangs.has(lang)) {
    for (let i = 0; i < text.length; i++) {
      if (text[i] === "#" && !overlaps(protectedRanges, i, i + 1)) return i;
    }
    return -1;
  }

  for (let i = 0; i < text.length - 1; i++) {
    if (text[i] === "/" && text[i + 1] === "/" && !overlaps(protectedRanges, i, i + 2)) {
      return i;
    }
  }
  return -1;
}

export function tokenizeCodeLine(text: string, lang: string | null): CodeToken[] {
  const normalizedLang = normalizeFenceLang(lang);
  const keywords = keywordSetForLang(normalizedLang);
  const tokens: CodeToken[] = [];
  const protectedRanges: ProtectedRange[] = [];

  scanStringTokens(text, tokens, protectedRanges);

  const commentStart = findCommentStart(text, normalizedLang, protectedRanges);
  if (commentStart >= 0) {
    pushCodeToken(tokens, commentStart, text.length, "comment");
    protect(protectedRanges, commentStart, text.length);
  }

  let i = 0;
  while (i < text.length) {
    const ch = text[i] ?? "";
    if (overlaps(protectedRanges, i, i + 1)) {
      i++;
      continue;
    }

    if (/[A-Za-z_]/.test(ch)) {
      const start = i;
      i++;
      while (i < text.length && isIdentifierChar(text[i] ?? "")) i++;
      const word = text.slice(start, i);
      if (keywords.has(word)) {
        pushCodeToken(tokens, start, i, "keyword");
      } else {
        const next = nextNonWhitespaceChar(text, i);
        if (next === "(" || next === "!") {
          pushCodeToken(tokens, start, i, "function");
        } else if (/^[A-Z][A-Za-z0-9_]*$/.test(word)) {
          pushCodeToken(tokens, start, i, "type");
        }
      }
      continue;
    }

    if (/[0-9]/.test(ch)) {
      const start = i;
      i++;
      while (i < text.length && /[0-9_]/.test(text[i] ?? "")) i++;
      if (text[i] === "." && /[0-9]/.test(text[i + 1] ?? "")) {
        i++;
        while (i < text.length && /[0-9_]/.test(text[i] ?? "")) i++;
      }
      pushCodeToken(tokens, start, i, "number");
      continue;
    }

    i++;
  }

  tokens.sort((a, b) => (a.from === b.from ? a.to - b.to : a.from - b.from));
  return tokens;
}

function addCodeSyntaxDecorations(
  builder: RangeSetBuilder<Decoration>,
  lineFrom: number,
  text: string,
  lang: string | null,
) {
  for (const token of tokenizeCodeLine(text, lang)) {
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
  let codeFenceLang: string | null = null;

  for (let lineNo = 1; lineNo <= view.state.doc.lines; lineNo++) {
    const line = view.state.doc.line(lineNo);
    const info = classifyMarkdownLine(line.text);

    if (info.isCodeFence) {
      if (!inCodeBlock) {
        codeFenceLang = parseFenceLanguage(line.text);
      }
      builder.add(line.from, line.from, decCodeFenceLine);
      builder.add(line.from, line.to, decFenceToken);
      inCodeBlock = !inCodeBlock;
      if (!inCodeBlock) {
        codeFenceLang = null;
      }
      continue;
    }

    if (inCodeBlock) {
      builder.add(line.from, line.from, decCodeBlockLine);
      addCodeSyntaxDecorations(builder, line.from, line.text, codeFenceLang);
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

    if (info.listMarkerEnd && info.checklistMarkerStart === null) {
      builder.add(line.from, line.from, decListLine);
      builder.add(line.from, line.from + info.listMarkerEnd, decListToken);
    }

    if (info.checklistMarkerStart !== null && info.checklistMarkerEnd !== null) {
      try {
        builder.add(line.from, line.from, decChecklistLine);
        const markerFrom = line.from + info.checklistMarkerStart;
        const markerTo = line.from + info.checklistMarkerEnd;
        // Dim the brackets [ ] as tokens, style the inner mark
        builder.add(markerFrom, markerFrom + 1, decChecklistToken);   // [
        builder.add(markerFrom + 1, markerTo - 1, info.checklistChecked ? decChecklistMarkChecked : decChecklistMark); // x or space
        builder.add(markerTo - 1, markerTo, decChecklistToken);       // ]
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

    addInlineDecorations(builder, line.from, line.text);
  }

  return builder.finish();
}

const markdownRichPlugin = ViewPlugin.fromClass(
  class {
    decorations: DecorationSet;

    constructor(view: EditorView) {
      this.decorations = this.safeBuild(view);
    }

    update(update: ViewUpdate) {
      if (update.docChanged || update.viewportChanged) {
        this.decorations = this.safeBuild(update.view);
      }
    }

    private safeBuild(view: EditorView): DecorationSet {
      try {
        return buildMarkdownDecorations(view);
      } catch (error) {
        console.error("Markdown decoration build failed:", error);
        return Decoration.none;
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
