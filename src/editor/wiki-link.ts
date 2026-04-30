import {
  startCompletion,
  type CompletionSource,
} from "@codemirror/autocomplete";
import { EditorView } from "@codemirror/view";
import { listNotesMeta, resolveWikiLink } from "../api.ts";
import { markdownWikiLinkAtCursor } from "./wasm.ts";

// Regex: match [[ followed by anything that isn't ] up to cursor position.
const WIKI_LINK_OPEN_RE = /\[\[[^\]]*$/;

const wikiLinkCompletionSource: CompletionSource = async (context) => {
  const match = context.matchBefore(WIKI_LINK_OPEN_RE);
  if (!match) return null;

  const query = match.text.slice(2).toLowerCase();
  const notes = await listNotesMeta();
  const filtered = notes
    .filter((n) => {
      const title = n.title.toLowerCase();
      return query.length === 0 || title.includes(query);
    })
    .slice(0, 30);

  if (filtered.length === 0) return null;

  return {
    from: match.from,
    options: filtered.map((note) => {
      const shortId = note.id.slice(0, 8);
      const title = note.title || "Untitled";
      return {
        label: title,
        type: "wiki-link",
        apply: (view: EditorView, _completion: object, from: number, to: number) => {
          let actualTo = to;
          if (view.state.doc.sliceString(to, to + 2) === "]]") {
            actualTo = to + 2;
          }
          // Insert [[shortId]] — title is shown dynamically via async resolver.
          // User can append #heading or |alt-text manually if needed.
          const insertText = `[[${shortId}]]`;
          view.dispatch({
            changes: { from, to: actualTo, insert: insertText },
            selection: { anchor: from + insertText.length },
          });
        },
      };
    }),
    filter: false,
  };
};

// Input handler: when user types the second [, auto-insert ]] and open picker.
const wikiLinkInputHandler = EditorView.inputHandler.of((view, from, to, text) => {
  if (text !== "[") return false;
  const prevChar = from > 0 ? view.state.doc.sliceString(from - 1, from) : "";
  if (prevChar !== "[") return false;
  view.dispatch({
    changes: { from, to, insert: "[]]" },
    selection: { anchor: from + 1 },
  });
  startCompletion(view);
  return true;
});

// Click handler: navigate to the linked note when clicking a wiki-link-title span.
function wikiLinkClickHandler(
  onNavigate: (noteId: string, heading?: string) => void,
): (event: MouseEvent, view: EditorView) => boolean {
  return (event, view) => {
    const target = event.target as Element | null;
    const titleEl = target?.closest(".md-wiki-link-title");
    if (!titleEl || titleEl.classList.contains("md-wiki-link-broken")) return false;

    if (!event.ctrlKey && !event.metaKey) return false;

    const coordPos = view.posAtCoords({ x: event.clientX, y: event.clientY });
    const pos = coordPos ?? view.posAtDOM(titleEl);
    const line = view.state.doc.lineAt(pos);
    const link = markdownWikiLinkAtCursor(line.text, pos - line.from);
    if (!link) return false;
    resolveWikiLink(link.shortId)
      .then((result) => {
        if (result) onNavigate(result.id, link.heading ?? undefined);
      })
      .catch(() => {});
    event.preventDefault();
    return true;
  };
}

export { wikiLinkCompletionSource };

export function wikiLinkExtensions(
  onNavigate?: (noteId: string, heading?: string) => void,
) {
  const extensions: ReturnType<typeof EditorView.domEventHandlers>[] = [wikiLinkInputHandler];

  if (onNavigate) {
    extensions.push(
      EditorView.domEventHandlers({
        click: wikiLinkClickHandler(onNavigate),
      }),
    );
  }

  return extensions;
}
