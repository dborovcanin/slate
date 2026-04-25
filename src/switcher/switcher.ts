import { state, type NoteEntry } from "../state";
import { searchNotesContent, type NoteSearchResult } from "../api";
import { highlightPositions } from "../ui/escape.ts";
import { createListOverlay, type ListOverlay } from "../overlays/overlay.ts";
import { fuzzyFilter, fuzzyMatch } from "./fuzzy";

type SwitcherItem = {
  item: NoteEntry;
  positions: number[];
  matchedByContent: boolean;
  snippet?: string;
};
type DeleteCallback = (id: string) => void;
type SwitcherMode = "title" | "content";

let overlay: ListOverlay | null = null;
let onSelectCallback: ((id: string) => void) | null = null;
let onDeleteCallback: DeleteCallback | null = null;
let activeMode: SwitcherMode = "title";
let activeSearchQuery = "";
let activeSearchItems: NoteSearchResult[] | null = null;
let activeSearchRequestId = 0;

function renderSnippet(text: string): DocumentFragment {
  const fragment = document.createDocumentFragment();
  const parts = text.split(/(\[\[|\]\])/);
  let inMatch = false;
  for (const part of parts) {
    if (part === "[[") { inMatch = true; continue; }
    if (part === "]]") { inMatch = false; continue; }
    if (part === "") continue;
    if (inMatch) {
      const mark = document.createElement("mark");
      mark.textContent = part;
      fragment.appendChild(mark);
    } else {
      fragment.appendChild(document.createTextNode(part));
    }
  }
  return fragment;
}

function allNotesForSwitcher(): NoteEntry[] {
  const notes = state.notes.filter((n) => n.id !== state.activeNote?.id);
  return state.activeNote
    ? [
        {
          id: state.activeNote.id,
          title: state.notes.find((n) => n.id === state.activeNote!.id)?.title ?? "Untitled",
          accessMode: state.activeNote.access_mode,
          isUnlocked: state.activeNote.is_unlocked,
          updatedAt: state.activeNote.updated_at,
        },
        ...notes,
      ]
    : notes;
}

function queueContentSearch(query: string) {
  const requestId = ++activeSearchRequestId;
  void searchNotesContent(query, 60)
    .then((results) => {
      if (requestId !== activeSearchRequestId || query !== activeSearchQuery) return;
      activeSearchItems = results;
      refreshSwitcher();
    })
    .catch(() => {
      if (requestId !== activeSearchRequestId || query !== activeSearchQuery) return;
      activeSearchItems = [];
      refreshSwitcher();
    });
}

function buildItems(query: string): SwitcherItem[] {
  const allNotes = allNotesForSwitcher();
  const trimmed = query.trim();
  if (trimmed.length === 0) {
    activeSearchQuery = "";
    activeSearchItems = null;
    return fuzzyFilter("", allNotes, (n) => n.title).map(({ item, positions }) => ({
      item,
      positions,
      matchedByContent: false,
    }));
  }

  if (activeMode === "title") {
    return fuzzyFilter(trimmed, allNotes, (n) => n.title).map(({ item, positions }) => ({
      item,
      positions,
      matchedByContent: false,
    }));
  }

  if (activeSearchQuery !== trimmed) {
    activeSearchQuery = trimmed;
    activeSearchItems = null;
    queueContentSearch(trimmed);
  }

  if (!activeSearchItems) {
    return fuzzyFilter(trimmed, allNotes, (n) => n.title).map(({ item, positions }) => ({
      item,
      positions,
      matchedByContent: false,
    }));
  }

  const itemsById = new Map(allNotes.map((note) => [note.id, note]));
  return activeSearchItems.map((result) => {
    const item = itemsById.get(result.id) ?? {
      id: result.id,
      title: result.title,
      accessMode: "none" as const,
      isUnlocked: false,
      updatedAt: result.updated_at,
    };
    const titleMatch = fuzzyMatch(trimmed, item.title);
    return {
      item,
      positions: titleMatch?.positions ?? [],
      matchedByContent: titleMatch === null,
      snippet: result.snippet || undefined,
    };
  });
}

function renderSwitcherItem(
  { item, positions, matchedByContent, snippet }: SwitcherItem,
  selected: boolean,
): HTMLElement {
  const row = document.createElement("div");
  row.className = "switcher-item" + (selected ? " switcher-item--active" : "");
  row.setAttribute("role", "option");
  row.setAttribute("aria-selected", selected ? "true" : "false");

  const title = document.createElement("span");
  title.className = "switcher-item-title";
  title.appendChild(highlightPositions(item.title, positions));
  row.appendChild(title);

  if (item.accessMode === "locked") {
    const badge = document.createElement("span");
    badge.className = "switcher-item-badge switcher-item-badge--locked";
    badge.textContent = "locked (app)";
    row.appendChild(badge);
  } else if (item.accessMode === "encrypted") {
    const badge = document.createElement("span");
    badge.className = "switcher-item-badge switcher-item-badge--encrypted";
    badge.textContent = "encrypted";
    row.appendChild(badge);
  }

  if (matchedByContent) {
    const badge = document.createElement("span");
    badge.className = "switcher-item-badge switcher-item-badge--content";
    badge.textContent = "content";
    row.appendChild(badge);
  }

  if (matchedByContent && snippet) {
    const snippetEl = document.createElement("span");
    snippetEl.className = "switcher-item-snippet";
    snippetEl.appendChild(renderSnippet(snippet));
    row.appendChild(snippetEl);
  }

  if (item.id === state.activeNote?.id) {
    const badge = document.createElement("span");
    badge.className = "switcher-item-badge switcher-item-badge--current";
    badge.textContent = "current";
    row.appendChild(badge);
  }

  return row;
}

export function isSwitcherOpen(): boolean {
  return overlay?.isOpen() ?? false;
}

export function openSwitcher(
  selectCallback: (id: string) => void,
  deleteCallback?: DeleteCallback,
  mode: SwitcherMode = "title",
) {
  onSelectCallback = selectCallback;
  onDeleteCallback = deleteCallback ?? null;

  if (mode !== activeMode) {
    overlay?.close();
    overlay = null;
    activeMode = mode;
    activeSearchQuery = "";
    activeSearchItems = null;
  }

  if (!overlay) {
    const placeholder = mode === "content" ? "Search note content..." : "Search notes...";
    overlay = createListOverlay<SwitcherItem>({
      classPrefix: "switcher",
      placeholder,
      backdrop: true,
      getItems: buildItems,
      renderItem: renderSwitcherItem,
      onSelect: ({ item }) => onSelectCallback?.(item.id),
      onKeydown: (event, state) => {
        const wantsDelete =
          event.key === "Delete" || (event.ctrlKey && !event.shiftKey && event.key === "Backspace");
        if (!wantsDelete || !onDeleteCallback) return false;
        event.preventDefault();
        event.stopPropagation();
        const selected = state.items[state.selectedIndex];
        if (!selected) return true;
        onDeleteCallback(selected.item.id);
        return true;
      },
      emptyMessage: "No notes found",
    });
  }

  overlay.open();
}

export function closeSwitcher() {
  overlay?.close();
  onDeleteCallback = null;
  activeSearchQuery = "";
  activeSearchItems = null;
}

export function refreshSwitcher() {
  if (!isSwitcherOpen()) return;
  overlay?.refresh();
}
