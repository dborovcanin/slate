import { Annotation, RangeSetBuilder, type Text } from "@codemirror/state";
import { Decoration, EditorView, ViewPlugin, WidgetType } from "@codemirror/view";
import type { DecorationSet, ViewUpdate } from "@codemirror/view";
import { variableIndexField } from "./calc-decoration.ts";
import type { VariableIndexEntry } from "../api.ts";
import { resolveNoteImagePaths, resolveWikiLinks } from "../api.ts";
import { state } from "../state.ts";
import { convertFileSrc } from "@tauri-apps/api/core";
import {
  ensureWasmReady,
  markdownAnalyzeLines,
  markdownClassifyLine,
  markdownFindImageMatches,
  markdownFindInlineTokens,
  markdownInlineMarkerComponentRanges,
  markdownTokenizeCodeLine,
  type MarkdownCodeToken as SharedCodeToken,
  type MarkdownInlineMarkerComponentRange as SharedInlineMarkerComponentRange,
  type MarkdownInlineToken as SharedInlineToken,
  type MarkdownLineInfo as SharedMarkdownLineInfo,
} from "./wasm.ts";
import {
  editorProfilerNowMs,
  isEditorProfilerEnabled,
  recordEditorProfilerSample,
} from "../perf/editor-profiler.ts";

type InlineToken = SharedInlineToken;
type CodeToken = SharedCodeToken;
type InlineMarkerComponentRange = SharedInlineMarkerComponentRange;
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
const INLINE_MARKER_RANGE_CACHE_LIMIT = 1024;
const IMAGE_MATCH_CACHE_LIMIT = 1024;
const WIKI_LINK_CACHE_MAX_ENTRIES = 2048;
const WIKI_LINK_CACHE_TTL_MS = 5 * 60_000;
const WIKI_LINK_BROKEN_CACHE_TTL_MS = 1_500;
const IMAGE_PATH_CACHE_MAX_ENTRIES = 2048;
const IMAGE_PATH_CACHE_TTL_MS = 5 * 60_000;
const IMAGE_PATH_BROKEN_CACHE_TTL_MS = 30_000;
const IMAGE_RESOLVE_MAX_CONCURRENCY = 2;
const IMAGE_RESOLVE_BATCH_SIZE = 24;
const IMAGE_PREVIEW_MAX_WIDTH = 900;
const IMAGE_PREVIEW_MAX_HEIGHT = 700;
const IMAGE_RESIZE_MIN = 24;
const DATA_URL_PREFIX = "data:";

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
const decImageAlt = Decoration.mark({ class: "md-image-alt" });
const decImageSrc = Decoration.mark({ class: "md-image-src" });
const decImageMarker = Decoration.mark({ class: "md-token md-token-link" });
const decLinkText = Decoration.mark({ class: "md-link-text" });
const decLinkUrl = Decoration.mark({ class: "md-link-url" });
const decLinkMarker = Decoration.mark({ class: "md-token md-token-link" });
const decWikiLinkTitle = Decoration.mark({ class: "md-wiki-link-title" });
const decWikiLinkBroken = Decoration.mark({ class: "md-wiki-link-title md-wiki-link-broken" });
const decWikiLinkHidden = Decoration.mark({ class: "md-token md-token-wiki-link" });

class WikiLinkDisplayWidget extends WidgetType {
  constructor(
    private readonly displayText: string,
    private readonly broken: boolean,
  ) {
    super();
  }

  eq(other: WikiLinkDisplayWidget): boolean {
    return other.displayText === this.displayText && other.broken === this.broken;
  }

  toDOM(): HTMLElement {
    const span = document.createElement("span");
    span.className = this.broken
      ? "md-wiki-link-title md-wiki-link-broken"
      : "md-wiki-link-title";
    span.textContent = this.displayText;
    span.contentEditable = "false";
    return span;
  }
}

class MarkdownImageDisplayWidget extends WidgetType {
  constructor(
    private readonly srcUrl: string | null,
    private readonly fallbackSrcUrl: string | null,
    private readonly broken: boolean,
    private readonly alt: string,
    private readonly fallbackLabel: string,
    private readonly tokenLabel: string,
    private readonly sourceFrom: number,
    private readonly sourceTo: number,
    private readonly width: number | null,
    private readonly height: number | null,
  ) {
    super();
  }

  eq(other: MarkdownImageDisplayWidget): boolean {
    return (
      other.srcUrl === this.srcUrl
      && other.fallbackSrcUrl === this.fallbackSrcUrl
      && other.broken === this.broken
      && other.alt === this.alt
      && other.fallbackLabel === this.fallbackLabel
      && other.tokenLabel === this.tokenLabel
      && other.sourceFrom === this.sourceFrom
      && other.sourceTo === this.sourceTo
      && other.width === this.width
      && other.height === this.height
    );
  }

  private selectSourceRange(view: EditorView) {
    const from = Math.max(0, Math.min(this.sourceFrom, this.sourceTo));
    const to = Math.max(from, this.sourceTo);
    view.dispatch({
      selection: { anchor: from, head: to },
      scrollIntoView: true,
    });
    view.focus();
  }

  private bindEditModeClick(target: HTMLElement, view: EditorView) {
    target.addEventListener("mousedown", (event) => {
      if (event.button !== 0) return;
      if (event.ctrlKey || event.metaKey || event.altKey || event.shiftKey) return;
      event.preventDefault();
      event.stopPropagation();
      this.selectSourceRange(view);
    });
  }

  private imageTokenNode(href: string | null, broken = false): HTMLElement {
    const token = href
      ? document.createElement("a")
      : document.createElement("span");
    token.className = broken
      ? "md-image-display-token md-image-display-token-broken"
      : "md-image-display-token";
    token.textContent = this.tokenLabel;
    token.contentEditable = "false";
    if (token instanceof HTMLAnchorElement && href) {
      token.href = href;
      token.target = "_blank";
      token.rel = "noreferrer noopener";
      token.title = "Open image";
    }
    return token;
  }

  toDOM(view: EditorView): HTMLElement {
    const figure = document.createElement("figure");
    figure.className = "md-image-display";
    figure.contentEditable = "false";
    figure.setAttribute("draggable", "false");

    const label = this.alt.trim().length > 0 ? this.alt.trim() : this.fallbackLabel;
    const maxWidth = Math.min(
      IMAGE_PREVIEW_MAX_WIDTH,
      Math.max(IMAGE_RESIZE_MIN, this.width ?? IMAGE_PREVIEW_MAX_WIDTH),
    );
    const maxHeight = Math.min(
      IMAGE_PREVIEW_MAX_HEIGHT,
      Math.max(IMAGE_RESIZE_MIN, this.height ?? IMAGE_PREVIEW_MAX_HEIGHT),
    );
    figure.style.setProperty("--img-max-width", `${maxWidth}px`);
    figure.style.setProperty("--img-max-height", `${maxHeight}px`);

    if (this.broken) {
      const token = this.imageTokenNode(
        this.srcUrl ?? this.fallbackSrcUrl ?? null,
        true,
      );
      this.bindEditModeClick(token, view);
      figure.appendChild(token);
      return figure;
    }

    if (!this.srcUrl) {
      const token = this.imageTokenNode(this.fallbackSrcUrl ?? null);
      this.bindEditModeClick(token, view);
      figure.appendChild(token);
      return figure;
    }

    const anchor = document.createElement("a");
    anchor.className = "md-image-display-link";
    anchor.href = this.srcUrl;
    anchor.target = "_blank";
    anchor.rel = "noreferrer noopener";
    anchor.title = "Open image";
    anchor.contentEditable = "false";
    this.bindEditModeClick(anchor, view);

    const img = document.createElement("img");
    img.className = "md-image-display-img";
    img.src = this.srcUrl;
    img.alt = label;
    img.loading = "lazy";
    img.decoding = "async";
    if (this.width && this.width > 0) img.width = this.width;
    if (this.height && this.height > 0) img.height = this.height;
    let fallbackTried = false;
    img.addEventListener("error", () => {
      if (!fallbackTried && this.fallbackSrcUrl) {
        fallbackTried = true;
        img.src = this.fallbackSrcUrl;
        return;
      }
      const token = this.imageTokenNode(this.srcUrl ?? this.fallbackSrcUrl ?? null, true);
      this.bindEditModeClick(token, view);
      anchor.replaceWith(token);
    });
    anchor.appendChild(img);
    figure.appendChild(anchor);

    const caption = this.imageTokenNode(this.srcUrl, false);
    caption.classList.add("md-image-display-caption");
    this.bindEditModeClick(caption, view);
    figure.appendChild(caption);
    return figure;
  }
}

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

const inlineMarkerRangeCache = new Map<string, InlineMarkerComponentRange[]>();

function cachedInlineMarkerComponentRanges(
  lineText: string,
  profiling?: MarkdownBuildProfiling,
): InlineMarkerComponentRange[] {
  const startedAt = profiling ? editorProfilerNowMs() : 0;
  const cached = inlineMarkerRangeCache.get(lineText);
  if (cached) {
    if (profiling) {
      profiling.inlineMarkerRangeMs += editorProfilerNowMs() - startedAt;
    }
    return cached;
  }

  const computed = markdownInlineMarkerComponentRanges(lineText);
  inlineMarkerRangeCache.set(lineText, computed);
  if (inlineMarkerRangeCache.size > INLINE_MARKER_RANGE_CACHE_LIMIT) {
    const oldest = inlineMarkerRangeCache.keys().next().value;
    if (typeof oldest === "string") {
      inlineMarkerRangeCache.delete(oldest);
    }
  }
  if (profiling) {
    profiling.inlineMarkerRangeMs += editorProfilerNowMs() - startedAt;
  }
  return computed;
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

function markerRevealComponentRangeForToken(
  tokens: readonly InlineToken[],
  markerIndex: number,
  componentRanges: readonly InlineMarkerComponentRange[],
): TextRange | null {
  const marker = tokens[markerIndex];
  if (
    !marker
    || (marker.type !== "code-marker"
      && marker.type !== "image-marker"
      && marker.type !== "link-marker"
      && marker.type !== "wiki-link-marker")
  ) {
    return null;
  }
  const range = componentRanges.find((entry) =>
    marker.from >= entry.from && marker.to <= entry.to
  );
  return range ? { from: range.from, to: range.to } : null;
}

function shouldRevealInlineMarker(
  tokens: readonly InlineToken[],
  componentRanges: readonly InlineMarkerComponentRange[],
  markerIndex: number,
  lineFrom: number,
  activeSelection?: ActiveSelection,
): boolean {
  const range = markerRevealComponentRangeForToken(tokens, markerIndex, componentRanges);
  if (!range) return false;
  return selectionTouchesInlineRange(activeSelection, lineFrom + range.from, lineFrom + range.to);
}

function shouldRevealInlineMarkerAtBoundary(
  tokens: readonly InlineToken[],
  componentRanges: readonly InlineMarkerComponentRange[],
  markerIndex: number,
  lineFrom: number,
  activeSelection?: ActiveSelection,
): boolean {
  const range = markerRevealComponentRangeForToken(tokens, markerIndex, componentRanges);
  if (!range) return false;
  return selectionTouchesRange(activeSelection, lineFrom + range.from, lineFrom + range.to);
}

interface WLAccum {
  cursorInside: boolean;
  linkFrom: number;
  firstMarkerFrom: number;
  firstMarkerTo: number;
  shortId: string;
  idFrom: number;
  idTo: number;
  anchorText: string | null;
  anchorFrom: number | null;
  anchorTo: number | null;
  sepFrom: number | null;
  sepTo: number | null;
  titleFrom: number | null;
  titleTo: number | null;
  hasTitle: boolean;
}

interface ImageAccum {
  cursorInside: boolean;
  linkFrom: number;
  firstMarkerFrom: number;
  firstMarkerTo: number;
  midMarkerFrom: number | null;
  midMarkerTo: number | null;
  sourceFrom: number;
  sourceTo: number;
  altText: string;
  altFrom: number | null;
  altTo: number | null;
  srcText: string;
  srcFrom: number | null;
  srcTo: number | null;
  width: number | null;
  height: number | null;
  attrsFrom: number | null;
  attrsTo: number | null;
}

function imageDisplayLabel(srcText: string, altText: string): string {
  const alt = altText.trim();
  if (alt.length > 0) return `🖼 ${alt}`;
  const parts = srcText.split(/[\\/]/g).filter((entry) => entry.length > 0);
  const file = parts.length > 0 ? parts[parts.length - 1] : srcText.trim();
  return file.length > 0 ? `🖼 ${file}` : "🖼 image";
}

function imageDisplayTokenLabel(
  imageIndex: number,
  srcText: string,
  altText: string,
): string {
  const fallback = imageDisplayLabel(srcText, altText).replace(/^🖼\s*/, "").trim();
  if (fallback.length === 0) return `[Image #${imageIndex}]`;
  const clipped = fallback.length > 42 ? `${fallback.slice(0, 39)}...` : fallback;
  return `[Image #${imageIndex}: ${clipped}]`;
}

const imageMatchCache = new Map<string, ReturnType<typeof markdownFindImageMatches>>();

function cachedImageMatches(lineText: string): ReturnType<typeof markdownFindImageMatches> {
  const cached = imageMatchCache.get(lineText);
  if (cached) return cached;
  const computed = markdownFindImageMatches(lineText);
  imageMatchCache.set(lineText, computed);
  if (imageMatchCache.size > IMAGE_MATCH_CACHE_LIMIT) {
    const oldest = imageMatchCache.keys().next().value;
    if (typeof oldest === "string") {
      imageMatchCache.delete(oldest);
    }
  }
  return computed;
}

interface ResolvedImagePreview {
  srcUrl: string | null;
  fallbackSrcUrl?: string | null;
  broken?: boolean;
}

function absolutePathToFileUrl(path: string): string {
  const normalized = path.replace(/\\/g, "/");
  if (/^[A-Za-z]:\//.test(normalized)) {
    return `file:///${encodeURI(normalized)}`;
  }
  return `file://${encodeURI(normalized)}`;
}

function dataUrlToObjectUrl(dataUrl: string): string | null {
  if (!dataUrl.startsWith(DATA_URL_PREFIX)) return null;
  const splitAt = dataUrl.indexOf(",");
  if (splitAt <= 0 || splitAt >= dataUrl.length - 1) return null;
  const header = dataUrl.slice(0, splitAt);
  const payload = dataUrl.slice(splitAt + 1);
  const mimeMatch = /^data:([^;,]+)(;base64)?$/i.exec(header);
  const mimeType = mimeMatch?.[1] ?? "application/octet-stream";
  const isBase64 = header.toLowerCase().endsWith(";base64");
  try {
    if (isBase64) {
      const decoded = atob(payload);
      const bytes = new Uint8Array(decoded.length);
      for (let i = 0; i < decoded.length; i += 1) {
        bytes[i] = decoded.charCodeAt(i);
      }
      return URL.createObjectURL(new Blob([bytes], { type: mimeType }));
    }
    return URL.createObjectURL(
      new Blob([decodeURIComponent(payload)], { type: mimeType }),
    );
  } catch {
    return null;
  }
}

function emitImageDecorations(
  image: ImageAccum,
  closingMarkerFrom: number,
  closingMarkerTo: number,
  imageIndex: number,
  imagePreviewResolver: ((src: string) => ResolvedImagePreview | null) | undefined,
  pending: PendingDecoration[],
): void {
  if (image.cursorInside) {
    pending.push({
      from: image.firstMarkerFrom,
      to: image.firstMarkerTo,
      decoration: decImageMarker,
    });
    if (image.altFrom != null && image.altTo != null) {
      pending.push({ from: image.altFrom, to: image.altTo, decoration: decImageAlt });
    }
    if (image.midMarkerFrom != null && image.midMarkerTo != null) {
      pending.push({
        from: image.midMarkerFrom,
        to: image.midMarkerTo,
        decoration: decImageMarker,
      });
    }
    if (image.srcFrom != null && image.srcTo != null) {
      pending.push({ from: image.srcFrom, to: image.srcTo, decoration: decImageSrc });
    }
    pending.push({ from: closingMarkerFrom, to: closingMarkerTo, decoration: decImageMarker });
    if (image.attrsFrom != null && image.attrsTo != null) {
      pending.push({ from: image.attrsFrom, to: image.attrsTo, decoration: decImageMarker });
    }
    return;
  }

  pending.push({ from: image.firstMarkerFrom, to: image.firstMarkerTo, decoration: decHiddenMarkdownToken });
  if (image.altFrom != null && image.altTo != null) {
    pending.push({ from: image.altFrom, to: image.altTo, decoration: decHiddenMarkdownToken });
  }
  if (image.midMarkerFrom != null && image.midMarkerTo != null) {
    pending.push({ from: image.midMarkerFrom, to: image.midMarkerTo, decoration: decHiddenMarkdownToken });
  }
  if (image.srcFrom != null && image.srcTo != null) {
    pending.push({ from: image.srcFrom, to: image.srcTo, decoration: decHiddenMarkdownToken });
  }
  pending.push({ from: closingMarkerFrom, to: closingMarkerTo, decoration: decHiddenMarkdownToken });
  if (image.attrsFrom != null && image.attrsTo != null) {
    pending.push({ from: image.attrsFrom, to: image.attrsTo, decoration: decHiddenMarkdownToken });
  }
  const resolved = imagePreviewResolver ? imagePreviewResolver(image.srcText.trim()) : null;
  const tokenLabel = imageDisplayTokenLabel(imageIndex, image.srcText, image.altText);
  const sourceTo = image.attrsTo ?? closingMarkerTo;
  pending.push({
    from: image.firstMarkerFrom,
    to: image.firstMarkerFrom,
    decoration: Decoration.widget({
      widget: new MarkdownImageDisplayWidget(
        resolved?.srcUrl ?? null,
        resolved?.fallbackSrcUrl ?? null,
        resolved?.broken === true,
        image.altText,
        imageDisplayLabel(image.srcText, image.altText),
        tokenLabel,
        image.firstMarkerFrom,
        sourceTo,
        image.width,
        image.height,
      ),
      side: 1,
    }),
  });
}

function emitWikiLinkDecorations(
  wl: WLAccum,
  closingMarkerFrom: number,
  closingMarkerTo: number,
  lineText: string,
  lineFrom: number,
  wikiLinkResolver: ((shortId: string) => WikiLinkResolution | null) | undefined,
  pending: PendingDecoration[],
): void {
  if (wl.cursorInside) {
    // Cursor inside wiki-link: keep full source visible/editable.
    // Markers stay dimmed like other markdown syntax.
    pending.push({ from: wl.firstMarkerFrom, to: wl.firstMarkerTo, decoration: decWikiLinkHidden });
    if (wl.hasTitle) {
      const resolved = wikiLinkResolver ? wikiLinkResolver(wl.shortId) : null;
      pending.push({
        from: wl.titleFrom!,
        to: wl.titleTo!,
        decoration: resolved?.exists === false ? decWikiLinkBroken : decWikiLinkTitle,
      });
    }
    pending.push({ from: closingMarkerFrom, to: closingMarkerTo, decoration: decWikiLinkHidden });
    return;
  }

  if (wl.hasTitle) {
    // Cursor outside, explicit alt-text: hide everything, show styled title.
    pending.push({ from: wl.firstMarkerFrom, to: wl.firstMarkerTo, decoration: decHiddenMarkdownToken });
    pending.push({ from: wl.idFrom, to: wl.idTo, decoration: decHiddenMarkdownToken });
    if (wl.anchorFrom != null) {
      pending.push({ from: wl.anchorFrom, to: wl.anchorTo!, decoration: decHiddenMarkdownToken });
    }
    pending.push({ from: wl.sepFrom!, to: wl.sepTo!, decoration: decHiddenMarkdownToken });
    const resolved = wikiLinkResolver ? wikiLinkResolver(wl.shortId) : null;
    pending.push({
      from: wl.titleFrom!,
      to: wl.titleTo!,
      decoration: resolved?.exists === false ? decWikiLinkBroken : decWikiLinkTitle,
    });
    pending.push({ from: closingMarkerFrom, to: closingMarkerTo, decoration: decHiddenMarkdownToken });
    return;
  }

  // Cursor outside, no alt-text: hide source and show display widget.
  // Keep source text in the document so caret can enter the span and switch to edit mode.
  const resolved = wikiLinkResolver ? wikiLinkResolver(wl.shortId) : null;
  if (resolved === null) {
    // Pending — show raw text until resolved (no decorations).
    return;
  }
  const heading = wl.anchorText ? wl.anchorText.slice(1) : null;
  const displayText = resolved.exists
    ? resolved.title + (heading ? "#" + heading : "")
    : "?" + (heading ? "#" + heading : "");
  pending.push({ from: wl.firstMarkerFrom, to: wl.firstMarkerTo, decoration: decHiddenMarkdownToken });
  pending.push({ from: wl.idFrom, to: wl.idTo, decoration: decHiddenMarkdownToken });
  if (wl.anchorFrom != null) {
    pending.push({ from: wl.anchorFrom, to: wl.anchorTo!, decoration: decHiddenMarkdownToken });
  }
  pending.push({ from: closingMarkerFrom, to: closingMarkerTo, decoration: decHiddenMarkdownToken });
  pending.push({
    from: wl.firstMarkerFrom,
    to: wl.firstMarkerFrom,
    decoration: Decoration.widget({
      widget: new WikiLinkDisplayWidget(displayText, !resolved.exists),
      side: 1,
    }),
  });
}

function collectInlineDecorations(
  lineFrom: number,
  _lineNumber: number,
  lineText: string,
  tokens: readonly InlineToken[],
  componentRanges: readonly InlineMarkerComponentRange[],
  activeSelection?: ActiveSelection,
  wikiLinkResolver?: (shortId: string) => WikiLinkResolution | null,
  imagePreviewResolver?: (src: string) => ResolvedImagePreview | null,
): PendingDecoration[] {
  const pending: PendingDecoration[] = [];
  const imageMatches = cachedImageMatches(lineText);
  let wlAccum: WLAccum | null = null;
  let imageAccum: ImageAccum | null = null;
  let imageIndex = 0;

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
          decoration: shouldRevealInlineMarker(
            tokens,
            componentRanges,
            index,
            lineFrom,
            activeSelection,
          )
            ? decCodeMarker
            : decHiddenMarkdownToken,
        });
        break;
      case "image-marker":
        if (imageAccum === null) {
          imageAccum = {
            cursorInside: false,
            linkFrom: from,
            firstMarkerFrom: from,
            firstMarkerTo: to,
            midMarkerFrom: null,
            midMarkerTo: null,
            sourceFrom: token.from,
            sourceTo: token.to,
            altText: "",
            altFrom: null,
            altTo: null,
            srcText: "",
            srcFrom: null,
            srcTo: null,
            width: null,
            height: null,
            attrsFrom: null,
            attrsTo: null,
          };
        } else if (imageAccum.midMarkerFrom === null) {
          imageAccum.midMarkerFrom = from;
          imageAccum.midMarkerTo = to;
        } else {
          const currentImage = imageAccum;
          currentImage.sourceTo = token.to;
          imageIndex += 1;
          const matched = imageMatches.find((entry) => entry.from === currentImage.sourceFrom);
          if (matched) {
            currentImage.sourceTo = matched.to;
            currentImage.width = matched.width;
            currentImage.height = matched.height;
            if (matched.to > token.to) {
              currentImage.attrsFrom = lineFrom + token.to;
              currentImage.attrsTo = lineFrom + matched.to;
            }
          }
          const fullFrom = lineFrom + currentImage.sourceFrom;
          const fullTo = lineFrom + currentImage.sourceTo;
          currentImage.cursorInside = selectionTouchesInlineRange(activeSelection, fullFrom, fullTo);
          emitImageDecorations(currentImage, from, to, imageIndex, imagePreviewResolver, pending);
          imageAccum = null;
        }
        break;
      case "image-alt":
        if (imageAccum) {
          imageAccum.altText = lineText.slice(token.from, token.to);
          imageAccum.altFrom = from;
          imageAccum.altTo = to;
        }
        break;
      case "image-src":
        if (imageAccum) {
          imageAccum.srcText = lineText.slice(token.from, token.to);
          imageAccum.srcFrom = from;
          imageAccum.srcTo = to;
        }
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
          decoration: shouldRevealInlineMarker(
            tokens,
            componentRanges,
            index,
            lineFrom,
            activeSelection,
          )
            ? decLinkMarker
            : decHiddenMarkdownToken,
        });
        break;

      // Wiki-link tokens: accumulate then emit on closing marker.
      case "wiki-link-marker":
        if (wlAccum === null) {
          // Opening [[
          // Wiki-links reveal raw source when caret is inside the span or on
          // its right boundary, so keyboard navigation can enter edit mode.
          const cursorInside = shouldRevealInlineMarkerAtBoundary(
            tokens,
            componentRanges,
            index,
            lineFrom,
            activeSelection,
          );
          wlAccum = {
            cursorInside,
            linkFrom: from,
            firstMarkerFrom: from,
            firstMarkerTo: to,
            shortId: "",
            idFrom: 0,
            idTo: 0,
            anchorText: null,
            anchorFrom: null,
            anchorTo: null,
            sepFrom: null,
            sepTo: null,
            titleFrom: null,
            titleTo: null,
            hasTitle: false,
          };
        } else {
          // Closing ]]
          emitWikiLinkDecorations(wlAccum, from, to, lineText, lineFrom, wikiLinkResolver, pending);
          wlAccum = null;
        }
        break;
      case "wiki-link-id":
        if (wlAccum) {
          wlAccum.shortId = lineText.slice(token.from, token.to);
          wlAccum.idFrom = from;
          wlAccum.idTo = to;
        }
        break;
      case "wiki-link-anchor":
        if (wlAccum) {
          wlAccum.anchorText = lineText.slice(token.from, token.to);
          wlAccum.anchorFrom = from;
          wlAccum.anchorTo = to;
        }
        break;
      case "wiki-link-sep":
        if (wlAccum) {
          wlAccum.sepFrom = from;
          wlAccum.sepTo = to;
        }
        break;
      case "wiki-link-title":
        if (wlAccum) {
          wlAccum.titleFrom = from;
          wlAccum.titleTo = to;
          wlAccum.hasTitle = true;
        }
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

interface MarkdownBuildProfiling {
  spanCount: number;
  lineCount: number;
  totalChars: number;
  maxLineLength: number;
  analyzeLinesMs: number;
  inlineMarkerRangeMs: number;
}

function createMarkdownBuildProfiling(): MarkdownBuildProfiling {
  return {
    spanCount: 0,
    lineCount: 0,
    totalChars: 0,
    maxLineLength: 0,
    analyzeLinesMs: 0,
    inlineMarkerRangeMs: 0,
  };
}

export type WikiLinkResolution = { exists: boolean; title: string };

export interface MarkdownDecorationBuildOptions {
  getFenceStateBeforeLine?: (lineNumber: number) => FenceState;
  variableMatcher?: VariableMatcher;
  profiling?: MarkdownBuildProfiling;
  // null = unknown/pending; { exists: false } = broken; { exists: true, title } = resolved
  wikiLinkResolver?: (shortId: string) => WikiLinkResolution | null;
  // Only lines returning true can enqueue wiki-link resolution work.
  resolveWikiLinksForLine?: (lineNumber: number) => boolean;
  imagePreviewResolver?: (lineNumber: number, src: string) => ResolvedImagePreview | null;
  resolveImagesForLine?: (lineNumber: number) => boolean;
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

  const profiling = options.profiling;
  const sortedSpans = [...spans].sort(
    (a, b) => a.fromLine - b.fromLine || a.toLine - b.toLine,
  );
  const builder = new RangeSetBuilder<Decoration>();
  const matcher = options.variableMatcher ?? createVariableMatcher(variableIndex);

  for (const span of sortedSpans) {
    const fromLine = Math.max(1, span.fromLine);
    const toLine = Math.min(doc.lines, span.toLine);
    if (fromLine > toLine) continue;
    if (profiling) profiling.spanCount += 1;

    const chunkLines: string[] = [];
    for (let lineNo = fromLine; lineNo <= toLine; lineNo++) {
      const text = doc.line(lineNo).text;
      chunkLines.push(text);
      if (profiling) {
        profiling.lineCount += 1;
        profiling.totalChars += text.length;
        if (text.length > profiling.maxLineLength) {
          profiling.maxLineLength = text.length;
        }
      }
    }
    const fenceState = options.getFenceStateBeforeLine
      ? cloneFenceState(options.getFenceStateBeforeLine(fromLine))
      : fallbackFenceStateBeforeLine(doc, fromLine);
    const analyzeStartedAt = profiling ? editorProfilerNowMs() : 0;
    const analysis = markdownAnalyzeLines(chunkLines, fenceState);
    if (profiling) {
      profiling.analyzeLinesMs += editorProfilerNowMs() - analyzeStartedAt;
    }

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
        profiling,
        options.resolveWikiLinksForLine?.(lineNo) === false
          ? undefined
          : options.wikiLinkResolver,
        options.resolveImagesForLine?.(lineNo) === false
          ? undefined
          : options.imagePreviewResolver
            ? (src: string) => options.imagePreviewResolver!(lineNo, src)
            : undefined,
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

function visibleSpans(view: EditorView): VisibleLineSpan[] {
  const doc = view.state.doc;
  if (view.visibleRanges.length === 0) return [];
  const spans = view.visibleRanges.map(({ from, to }) => ({
    fromLine: Math.max(1, doc.lineAt(from).number),
    toLine: Math.min(doc.lines, doc.lineAt(to).number),
  }));
  return mergeLineSpans(spans);
}

function lineInVisibleSpans(lineNumber: number, spans: readonly VisibleLineSpan[]): boolean {
  for (const span of spans) {
    if (lineNumber >= span.fromLine && lineNumber <= span.toLine) return true;
  }
  return false;
}

function buildMarkdownDecorations(
  view: EditorView,
  fenceCache: FenceCheckpointCache,
  matcherCache: VariableMatcherCache,
  marginLines = VIEWPORT_MARGIN_LINES,
  profiling?: MarkdownBuildProfiling,
  wikiLinkResolver?: (shortId: string) => WikiLinkResolution | null,
  imagePreviewResolver?: (lineNumber: number, src: string) => ResolvedImagePreview | null,
): DecorationSet {
  const doc = view.state.doc;
  const variableIndex = view.state.field(variableIndexField, false) ?? [];
  const selection = view.state.selection.main;
  const spans = expandedVisibleSpans(view, marginLines);
  const strictVisibleSpans = visibleSpans(view);
  const matcher = matcherCache.get(variableIndex);
  return buildMarkdownDecorationsForSpans(doc, spans, variableIndex, {
    from: selection.from,
    to: selection.to,
    empty: selection.empty,
  }, {
    getFenceStateBeforeLine: (lineNumber) =>
      fenceCache.getStateBeforeLine(doc, lineNumber),
    variableMatcher: matcher,
    profiling,
    wikiLinkResolver,
    resolveWikiLinksForLine: (lineNumber) =>
      lineInVisibleSpans(lineNumber, strictVisibleSpans),
    imagePreviewResolver,
    resolveImagesForLine: (lineNumber) =>
      lineInVisibleSpans(lineNumber, strictVisibleSpans),
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
  if (text.includes("[[") && text.includes("]]")) return true;
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
  if (
    !lineText.includes("*")
    && !lineText.includes("_")
    && !lineText.includes("`")
    && !lineText.includes("[")
    && !lineText.includes("~")
  ) {
    return "";
  }
  const ranges = cachedInlineMarkerComponentRanges(lineText);
  for (const range of ranges) {
    const signature = `${range.from}-${range.to}`;
    if (cursorOffsetInLine >= range.from && cursorOffsetInLine <= range.to) {
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
  line: { from: number; to: number; text: string; number: number },
  info: MarkdownLineInfo,
  matcher: VariableMatcher,
  inlineTokens: readonly InlineToken[],
  activeSelection?: ActiveSelection,
  profiling?: MarkdownBuildProfiling,
  wikiLinkResolver?: (shortId: string) => WikiLinkResolution | null,
  imagePreviewResolver?: (src: string) => ResolvedImagePreview | null,
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
  const inlineMarkerComponentRanges = cachedInlineMarkerComponentRanges(line.text, profiling);
  for (const inline of collectInlineDecorations(
    line.from,
    line.number,
    line.text,
    inlineTokens,
    inlineMarkerComponentRanges,
    activeSelection,
    wikiLinkResolver,
    imagePreviewResolver,
  )) {
    pending.push(inline);
  }
  pending.sort((a, b) => a.from - b.from || a.to - b.to);
  for (const entry of pending) {
    builder.add(entry.from, entry.to, entry.decoration);
  }
}

const markdownWasmReadyAnnotation = Annotation.define<boolean>();
const markdownDeferredRefreshAnnotation = Annotation.define<boolean>();
const markdownManualRefreshAnnotation = Annotation.define<{
  invalidateWikiLinkCache?: boolean;
  invalidatedShortIds?: string[];
  invalidatedImageSources?: string[];
  noteId?: string | null;
}>();

export function requestMarkdownDecorationRefresh(
  view: EditorView,
  options?: {
    invalidateWikiLinkCache?: boolean;
    invalidatedShortIds?: string[];
    invalidatedImageSources?: string[];
    noteId?: string | null;
  },
) {
  view.dispatch({
    annotations: markdownManualRefreshAnnotation.of({
      invalidateWikiLinkCache: !!options?.invalidateWikiLinkCache,
      invalidatedShortIds: options?.invalidatedShortIds ?? [],
      invalidatedImageSources: options?.invalidatedImageSources ?? [],
      noteId: options?.noteId ?? null,
    }),
  });
}

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

function changedRangeCount(update: ViewUpdate): number {
  let count = 0;
  update.changes.iterChangedRanges(() => {
    count += 1;
  });
  return count;
}

const markdownRichPlugin = ViewPlugin.fromClass(
  class {
    decorations: DecorationSet;
    private destroyed = false;
    private readonly fenceCache = new FenceCheckpointCache();
    private readonly matcherCache = new VariableMatcherCache();
    private pendingRefreshTimer: ReturnType<typeof setTimeout> | null = null;
    private readonly wikiLinkCache = new Map<string, WikiLinkResolution | false>();
    private readonly wikiLinkCacheResolvedAt = new Map<string, number>();
    private readonly resolvingIds = new Set<string>();
    private readonly pendingWikiLinkBatchIds = new Set<string>();
    private pendingWikiLinkBatchTimer: ReturnType<typeof setTimeout> | null = null;
    private readonly imagePathCache = new Map<string, string | false>();
    private readonly imagePathCacheResolvedAt = new Map<string, number>();
    private readonly imageObjectUrlCache = new Map<string, string>();
    private readonly resolvingImageKeys = new Set<string>();
    private readonly pendingImageResolveSources = new Set<string>();
    private pendingImageResolveTimer: ReturnType<typeof setTimeout> | null = null;
    private imageResolveInFlight = 0;
    private imageResolveNoteId: string | null = null;

    constructor(view: EditorView) {
      this.decorations = this.safeBuild(view, Decoration.none, VIEWPORT_MARGIN_LINES, "init");
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

    private getCachedWikiLink(shortId: string): WikiLinkResolution | false | undefined {
      const cached = this.wikiLinkCache.get(shortId);
      if (cached === undefined) return undefined;
      const resolvedAt = this.wikiLinkCacheResolvedAt.get(shortId) ?? 0;
      const ttlMs = cached === false ? WIKI_LINK_BROKEN_CACHE_TTL_MS : WIKI_LINK_CACHE_TTL_MS;
      if (Date.now() - resolvedAt > ttlMs) {
        this.wikiLinkCache.delete(shortId);
        this.wikiLinkCacheResolvedAt.delete(shortId);
        return undefined;
      }
      return cached;
    }

    private setCachedWikiLink(shortId: string, value: WikiLinkResolution | false) {
      // Refresh insertion order to keep simple LRU-ish trimming by oldest key.
      this.wikiLinkCache.delete(shortId);
      this.wikiLinkCacheResolvedAt.delete(shortId);
      this.wikiLinkCache.set(shortId, value);
      this.wikiLinkCacheResolvedAt.set(shortId, Date.now());
      while (this.wikiLinkCache.size > WIKI_LINK_CACHE_MAX_ENTRIES) {
        const oldest = this.wikiLinkCache.keys().next().value;
        if (typeof oldest !== "string") break;
        this.wikiLinkCache.delete(oldest);
        this.wikiLinkCacheResolvedAt.delete(oldest);
      }
    }

    private clearCachedWikiLink(shortId: string) {
      this.wikiLinkCache.delete(shortId);
      this.wikiLinkCacheResolvedAt.delete(shortId);
      this.pendingWikiLinkBatchIds.delete(shortId);
      this.resolvingIds.delete(shortId);
    }

    private clearImageCache() {
      for (const objectUrl of this.imageObjectUrlCache.values()) {
        URL.revokeObjectURL(objectUrl);
      }
      this.imagePathCache.clear();
      this.imagePathCacheResolvedAt.clear();
      this.imageObjectUrlCache.clear();
      this.resolvingImageKeys.clear();
      this.pendingImageResolveSources.clear();
      this.imageResolveInFlight = 0;
      this.imageResolveNoteId = null;
    }

    private imageCacheKey(noteId: string, source: string): string {
      return `${noteId}::${source}`;
    }

    private getCachedImagePath(cacheKey: string): string | false | undefined {
      const cached = this.imagePathCache.get(cacheKey);
      if (cached === undefined) return undefined;
      const resolvedAt = this.imagePathCacheResolvedAt.get(cacheKey) ?? 0;
      const ttlMs = cached === false ? IMAGE_PATH_BROKEN_CACHE_TTL_MS : IMAGE_PATH_CACHE_TTL_MS;
      if (Date.now() - resolvedAt > ttlMs) {
        this.clearCachedImagePathByKey(cacheKey);
        return undefined;
      }
      return cached;
    }

    private clearCachedImagePathByKey(cacheKey: string) {
      this.imagePathCache.delete(cacheKey);
      this.imagePathCacheResolvedAt.delete(cacheKey);
      const objectUrl = this.imageObjectUrlCache.get(cacheKey);
      if (objectUrl) {
        URL.revokeObjectURL(objectUrl);
        this.imageObjectUrlCache.delete(cacheKey);
      }
      this.resolvingImageKeys.delete(cacheKey);
    }

    private clearCachedImageSource(noteId: string, source: string) {
      const cacheKey = this.imageCacheKey(noteId, source);
      this.clearCachedImagePathByKey(cacheKey);
      this.pendingImageResolveSources.delete(source);
    }

    private setCachedImagePath(cacheKey: string, value: string | false) {
      this.imagePathCache.delete(cacheKey);
      this.imagePathCacheResolvedAt.delete(cacheKey);
      const objectUrl = this.imageObjectUrlCache.get(cacheKey);
      if (objectUrl) {
        URL.revokeObjectURL(objectUrl);
        this.imageObjectUrlCache.delete(cacheKey);
      }
      this.imagePathCache.set(cacheKey, value);
      this.imagePathCacheResolvedAt.set(cacheKey, Date.now());
      while (this.imagePathCache.size > IMAGE_PATH_CACHE_MAX_ENTRIES) {
        const oldest = this.imagePathCache.keys().next().value;
        if (typeof oldest !== "string") break;
        this.clearCachedImagePathByKey(oldest);
      }
    }

    private imageObjectUrlForCacheKey(cacheKey: string, dataUrl: string): string | null {
      const cached = this.imageObjectUrlCache.get(cacheKey);
      if (cached) return cached;
      const objectUrl = dataUrlToObjectUrl(dataUrl);
      if (!objectUrl) return null;
      this.imageObjectUrlCache.set(cacheKey, objectUrl);
      return objectUrl;
    }

    private scheduleWikiLinkBatchResolve(view: EditorView) {
      if (this.pendingWikiLinkBatchTimer !== null) return;
      this.pendingWikiLinkBatchTimer = setTimeout(() => {
        this.pendingWikiLinkBatchTimer = null;
        if (this.destroyed || this.pendingWikiLinkBatchIds.size === 0) return;
        const shortIds = [...this.pendingWikiLinkBatchIds];
        this.pendingWikiLinkBatchIds.clear();
        resolveWikiLinks(shortIds)
          .then((results) => {
            const resolvedIds = new Set<string>();
            for (const entry of results) {
              const shortId = entry.shortId;
              resolvedIds.add(shortId);
              this.setCachedWikiLink(
                shortId,
                entry.summary !== null
                  ? { exists: true, title: entry.summary.title }
                  : false,
              );
              this.resolvingIds.delete(shortId);
            }
            for (const shortId of shortIds) {
              if (!resolvedIds.has(shortId)) {
                this.resolvingIds.delete(shortId);
              }
            }
            if (!this.destroyed) this.scheduleDeferredRefresh(view);
          })
          .catch(() => {
            for (const shortId of shortIds) {
              this.resolvingIds.delete(shortId);
            }
          });
      }, 24);
    }

    private scheduleImagePathBatchResolve(view: EditorView, noteId: string) {
      if (this.pendingImageResolveTimer !== null) return;
      this.pendingImageResolveTimer = setTimeout(() => {
        this.pendingImageResolveTimer = null;
        if (this.destroyed || this.pendingImageResolveSources.size === 0) return;
        if (this.imageResolveInFlight >= IMAGE_RESOLVE_MAX_CONCURRENCY) {
          this.scheduleImagePathBatchResolve(view, noteId);
          return;
        }
        const activeNoteId = state.activeNote?.id ?? null;
        if (!activeNoteId || activeNoteId !== noteId) {
          this.pendingImageResolveSources.clear();
          this.resolvingImageKeys.clear();
          return;
        }
        const sources = [...this.pendingImageResolveSources].slice(0, IMAGE_RESOLVE_BATCH_SIZE);
        for (const source of sources) {
          this.pendingImageResolveSources.delete(source);
        }
        this.imageResolveInFlight += 1;
        resolveNoteImagePaths(noteId, sources)
          .then((results) => {
            const activeNoteId = state.activeNote?.id ?? null;
            if (this.destroyed || activeNoteId !== noteId) {
              for (const source of sources) {
                const cacheKey = this.imageCacheKey(noteId, source);
                this.resolvingImageKeys.delete(cacheKey);
              }
              return;
            }
            const seen = new Set<string>();
            for (const entry of results) {
              const source = entry.source;
              seen.add(source);
              const cacheKey = this.imageCacheKey(noteId, source);
              this.resolvingImageKeys.delete(cacheKey);
              this.setCachedImagePath(cacheKey, entry.path ? entry.path : false);
            }
            for (const source of sources) {
              if (seen.has(source)) continue;
              const cacheKey = this.imageCacheKey(noteId, source);
              this.resolvingImageKeys.delete(cacheKey);
            }
            if (!this.destroyed) this.scheduleDeferredRefresh(view);
          })
          .catch(() => {
            for (const source of sources) {
              const cacheKey = this.imageCacheKey(noteId, source);
              this.resolvingImageKeys.delete(cacheKey);
            }
          })
          .finally(() => {
            this.imageResolveInFlight = Math.max(0, this.imageResolveInFlight - 1);
            if (!this.destroyed && this.pendingImageResolveSources.size > 0) {
              this.scheduleImagePathBatchResolve(view, noteId);
            }
          });
      }, 24);
    }

    update(update: ViewUpdate) {
      const manualRefresh = update.transactions
        .map((transaction) => transaction.annotation(markdownManualRefreshAnnotation))
        .find((annotation) => annotation !== undefined);
      if (manualRefresh) {
        let manualReason = "manualRefresh";
        if (manualRefresh.invalidateWikiLinkCache) {
          this.wikiLinkCache.clear();
          this.wikiLinkCacheResolvedAt.clear();
          this.pendingWikiLinkBatchIds.clear();
          this.resolvingIds.clear();
          this.clearImageCache();
          manualReason = "manualRefresh_invalidateWikiLinks";
        } else if (manualRefresh.invalidatedShortIds?.length) {
          for (const shortId of manualRefresh.invalidatedShortIds) {
            this.clearCachedWikiLink(shortId);
          }
          manualReason = "manualRefresh_wikiSubset";
        }
        if (manualRefresh.invalidatedImageSources?.length) {
          const imageNoteId = manualRefresh.noteId ?? state.activeNote?.id ?? null;
          if (imageNoteId) {
            for (const source of manualRefresh.invalidatedImageSources) {
              this.clearCachedImageSource(imageNoteId, source);
            }
            manualReason = "manualRefresh_images";
          }
        }
        this.decorations = this.safeBuild(
          update.view,
          this.decorations,
          VIEWPORT_MARGIN_LINES,
          manualReason,
        );
        return;
      }

      if (
        update.transactions.some((transaction) =>
          transaction.annotation(markdownWasmReadyAnnotation),
        )
      ) {
        this.decorations = this.safeBuild(
          update.view,
          this.decorations,
          VIEWPORT_MARGIN_LINES,
          "init",
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
          "docChanged_deferred",
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
        const reason = varsChanged ? "varsChanged" : "viewportChanged";
        this.decorations = this.safeBuild(
          update.view,
          this.decorations,
          VIEWPORT_MARGIN_LINES,
          reason,
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
          "selectionSet",
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
      const profiling = isEditorProfilerEnabled();
      const startedAt = profiling ? editorProfilerNowMs() : 0;
      this.decorations = this.decorations.map(update.changes);
      this.scheduleDeferredRefresh(update.view);
      if (profiling) {
        recordEditorProfilerSample(
          "markdown.decorations.rebuildTrigger",
          editorProfilerNowMs() - startedAt,
          {
            reason: "docChanged_immediate",
            metrics: {
              changedRanges: changedRangeCount(update),
            },
          },
        );
      }
    }

    private safeBuild(
      view: EditorView,
      fallback: DecorationSet,
      marginLines: number,
      reason: string,
    ): DecorationSet {
      const profilingEnabled = isEditorProfilerEnabled();
      const startedAt = profilingEnabled ? editorProfilerNowMs() : 0;
      const profiling = profilingEnabled ? createMarkdownBuildProfiling() : undefined;
      try {
        const newIds = new Set<string>();
        const newImageSources = new Set<string>();
        const activeNoteId = state.activeNote?.id ?? null;
        if (this.imageResolveNoteId !== activeNoteId) {
          this.clearImageCache();
          this.imageResolveNoteId = activeNoteId;
        }
        const wikiLinkResolver = (shortId: string): WikiLinkResolution | null => {
          const cached = this.getCachedWikiLink(shortId);
          if (cached === false) return { exists: false, title: "" };
          if (cached !== undefined) return cached;
          newIds.add(shortId);
          return null;
        };
        const imagePreviewResolver = (
          _lineNumber: number,
          src: string,
        ): ResolvedImagePreview | null => {
          const noteId = activeNoteId;
          const source = src.trim();
          if (!noteId || source.length === 0) return null;
          const cacheKey = this.imageCacheKey(noteId, source);
          const cached = this.getCachedImagePath(cacheKey);
          if (cached === false) return { srcUrl: null, broken: true };
          if (typeof cached === "string") {
            if (cached.startsWith(DATA_URL_PREFIX)) {
              const objectUrl = this.imageObjectUrlForCacheKey(cacheKey, cached);
              return {
                srcUrl: objectUrl ?? cached,
                fallbackSrcUrl: null,
              };
            }
            return {
              srcUrl: convertFileSrc(cached),
              fallbackSrcUrl: absolutePathToFileUrl(cached),
            };
          }
          newImageSources.add(source);
          return null;
        };
        const next = buildMarkdownDecorations(
          view,
          this.fenceCache,
          this.matcherCache,
          marginLines,
          profiling,
          wikiLinkResolver,
          imagePreviewResolver,
        );
        for (const shortId of newIds) {
          if (this.resolvingIds.has(shortId)) continue;
          this.resolvingIds.add(shortId);
          this.pendingWikiLinkBatchIds.add(shortId);
        }
        if (this.pendingWikiLinkBatchIds.size > 0) {
          this.scheduleWikiLinkBatchResolve(view);
        }
        if (activeNoteId) {
          for (const source of newImageSources) {
            const cacheKey = this.imageCacheKey(activeNoteId, source);
            if (this.resolvingImageKeys.has(cacheKey)) continue;
            this.resolvingImageKeys.add(cacheKey);
            this.pendingImageResolveSources.add(source);
          }
          if (this.pendingImageResolveSources.size > 0) {
            this.scheduleImagePathBatchResolve(view, activeNoteId);
          }
        }
        if (profilingEnabled) {
          const durationMs = editorProfilerNowMs() - startedAt;
          const analyzeMs = profiling?.analyzeLinesMs ?? 0;
          const inlineMarkerRangeMs = profiling?.inlineMarkerRangeMs ?? 0;
          const decorationAssemblyMs = Math.max(
            0,
            durationMs - analyzeMs - inlineMarkerRangeMs,
          );
          recordEditorProfilerSample("markdown.decorations.safeBuild", durationMs, {
            reason,
            metrics: {
              marginLines,
              spanCount: profiling?.spanCount ?? 0,
              lineCount: profiling?.lineCount ?? 0,
              totalChars: profiling?.totalChars ?? 0,
              maxLineLength: profiling?.maxLineLength ?? 0,
              analyzeLinesMs: analyzeMs,
              inlineMarkerRangeMs,
              decorationAssemblyMs,
            },
          });
        }
        return next;
      } catch (error) {
        console.error("Markdown decoration build failed:", error);
        if (profilingEnabled) {
          recordEditorProfilerSample(
            "markdown.decorations.safeBuild",
            editorProfilerNowMs() - startedAt,
            { reason: `${reason}_error` },
          );
        }
        return fallback;
      }
    }

    destroy() {
      this.destroyed = true;
      if (this.pendingRefreshTimer !== null) {
        clearTimeout(this.pendingRefreshTimer);
        this.pendingRefreshTimer = null;
      }
      if (this.pendingWikiLinkBatchTimer !== null) {
        clearTimeout(this.pendingWikiLinkBatchTimer);
        this.pendingWikiLinkBatchTimer = null;
      }
      if (this.pendingImageResolveTimer !== null) {
        clearTimeout(this.pendingImageResolveTimer);
        this.pendingImageResolveTimer = null;
      }
      this.clearImageCache();
    }
  },
  {
    decorations: (plugin) => plugin.decorations,
  },
);

export function markdownRichTextExtensions() {
  return [markdownRichPlugin];
}
