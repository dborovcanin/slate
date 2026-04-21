import { Prec, type Extension } from "@codemirror/state";
import { EditorView, keymap } from "@codemirror/view";
import {
  findNext,
  findPrevious,
  getSearchQuery,
  highlightSelectionMatches,
  openSearchPanel,
  search,
  type SearchQuery,
} from "@codemirror/search";

function hasSearchText(query: SearchQuery): boolean {
  return query.search.trim().length > 0;
}

export function openEditorSearch(view: EditorView): boolean {
  return openSearchPanel(view);
}

export function editorSearchNext(view: EditorView): boolean {
  return findNext(view);
}

export function editorSearchPrev(view: EditorView): boolean {
  return findPrevious(view);
}

export function editorSearchHasMatches(view: EditorView): boolean {
  const query = getSearchQuery(view.state);
  if (!hasSearchText(query)) return false;
  const cursor = query.getCursor(view.state, 0, view.state.doc.length);
  return !cursor.next().done;
}

export function isEditorSearchOverlayTarget(_view: EditorView, target: EventTarget | null): boolean {
  return target instanceof Element && target.closest(".cm-search") !== null;
}

export function editorSearchExtensions(): Extension[] {
  return [
    search(),
    highlightSelectionMatches({
      highlightWordAroundCursor: false,
      minSelectionLength: 1,
      maxMatches: 200,
    }),
    Prec.highest(
      keymap.of([
        {
          key: "Ctrl-f",
          run: openEditorSearch,
          preventDefault: true,
        },
        {
          key: "Mod-f",
          run: openEditorSearch,
          preventDefault: true,
        },
      ]),
    ),
  ];
}
