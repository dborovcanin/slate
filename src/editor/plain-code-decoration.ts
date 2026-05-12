import { RangeSetBuilder, type Extension } from "@codemirror/state";
import { Decoration, EditorView, ViewPlugin, type ViewUpdate } from "@codemirror/view";
import { markdownTokenizeCodeLine, type MarkdownCodeToken } from "./wasm.ts";

const decCodeKeyword = Decoration.mark({ class: "md-code-token-keyword" });
const decCodeString = Decoration.mark({ class: "md-code-token-string" });
const decCodeNumber = Decoration.mark({ class: "md-code-token-number" });
const decCodeComment = Decoration.mark({ class: "md-code-token-comment" });
const decCodeFunction = Decoration.mark({ class: "md-code-token-function" });
const decCodeType = Decoration.mark({ class: "md-code-token-type" });

function tokenDecoration(kind: string): Decoration | null {
  switch (kind) {
    case "keyword":
      return decCodeKeyword;
    case "string":
      return decCodeString;
    case "number":
      return decCodeNumber;
    case "comment":
      return decCodeComment;
    case "function":
      return decCodeFunction;
    case "type":
      return decCodeType;
    default:
      return null;
  }
}

function buildDecorations(
  view: EditorView,
  language: string,
  tokenCache: Map<string, readonly MarkdownCodeToken[]>,
) {
  const builder = new RangeSetBuilder<Decoration>();
  for (const range of view.visibleRanges) {
    let line = view.state.doc.lineAt(range.from);
    while (true) {
      const cacheKey = `${language}\n${line.text}`;
      let tokens = tokenCache.get(cacheKey);
      if (!tokens) {
        tokens = language === "text" ? [] : markdownTokenizeCodeLine(line.text, language);
        tokenCache.set(cacheKey, tokens);
        if (tokenCache.size > 4096) {
          const oldest = tokenCache.keys().next().value;
          if (typeof oldest === "string") tokenCache.delete(oldest);
        }
      }
      for (const token of tokens) {
        const decoration = tokenDecoration(token.type);
        if (!decoration) continue;
        const from = line.from + token.from;
        const to = line.from + token.to;
        if (to > from) {
          builder.add(from, to, decoration);
        }
      }
      if (line.to >= range.to || line.number >= view.state.doc.lines) break;
      line = view.state.doc.line(line.number + 1);
    }
  }
  return builder.finish();
}

export function plainCodeSyntaxExtensions(language: string | null): Extension[] {
  if (!language) return [];
  const lang = language;
  const plugin = ViewPlugin.fromClass(class {
    decorations;
    tokenCache = new Map<string, readonly MarkdownCodeToken[]>();

    constructor(view: EditorView) {
      this.decorations = buildDecorations(view, lang, this.tokenCache);
    }

    update(update: ViewUpdate) {
      if (update.docChanged) this.tokenCache.clear();
      if (update.docChanged || update.viewportChanged) {
        this.decorations = buildDecorations(update.view, lang, this.tokenCache);
      }
    }
  }, {
    decorations: (v) => v.decorations,
  });

  return [plugin];
}
