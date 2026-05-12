import { state, type NoteEntry } from "../state";
import {
  listCollections,
  listNotesMetaFiltered,
  searchNotesContentFiltered,
  type Collection,
  type NoteSearchResult,
} from "../api";
import { highlightPositions } from "../ui/escape.ts";
import { createListOverlay, type ListOverlay } from "../overlays/overlay.ts";
import { fuzzyFilter, fuzzyMatch } from "./fuzzy";

type SwitcherItem = {
  item: NoteEntry;
  positions: number[];
  matchedByContent: boolean;
  snippet?: string;
  lineNumber?: number;
};
type DeleteCallback = (id: string) => void;
type SwitcherMode = "title" | "content";
const CONTENT_SEARCH_DEBOUNCE_MS = 120;

let overlay: ListOverlay | null = null;
let onSelectCallback: ((id: string, lineNumber?: number | null) => void) | null = null;
let onDeleteCallback: DeleteCallback | null = null;
let activeMode: SwitcherMode = "title";
let activeSearchQuery = "";
let activeSearchItems: NoteSearchResult[] | null = null;
let activeCollectionFilter: string | null = null;
let activeCollectionOptions: Collection[] = [];
let activeCollectionNotes: NoteEntry[] | null = null;
let activeSearchRequestId = 0;
let activeSearchTimer: number | null = null;
let activeSearchInFlight = false;
let queuedSearchQuery: string | null = null;
const ALL_COLLECTIONS_TOKEN = "__all_collections__";

function resetContentCollectionFilterFromWorkingCollection() {
  activeCollectionFilter = state.workingCollection?.id ?? null;
}

function toNoteEntriesFromSummaries(
  summaries: Awaited<ReturnType<typeof listNotesMetaFiltered>>,
): NoteEntry[] {
  return summaries.map((summary) => ({
    id: summary.id,
    title: summary.title,
    accessMode: summary.access_mode,
    isUnlocked: summary.is_unlocked,
    updatedAt: summary.updated_at,
  }));
}

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

function clearContentSearchTimer() {
  if (activeSearchTimer !== null) {
    window.clearTimeout(activeSearchTimer);
    activeSearchTimer = null;
  }
}

function resetContentSearchState() {
  clearContentSearchTimer();
  activeSearchRequestId += 1;
  activeSearchInFlight = false;
  queuedSearchQuery = null;
  activeSearchQuery = "";
  activeSearchItems = null;
}

function maybeDispatchQueuedContentSearch() {
  if (activeSearchInFlight) return;
  const query = queuedSearchQuery;
  if (!query || query !== activeSearchQuery) {
    queuedSearchQuery = null;
    return;
  }
  queuedSearchQuery = null;

  const requestId = ++activeSearchRequestId;
  activeSearchInFlight = true;
  void searchNotesContentFiltered(query, 60, activeCollectionFilter)
    .then((results) => {
      if (requestId !== activeSearchRequestId || query !== activeSearchQuery) return;
      activeSearchItems = results;
      refreshSwitcher();
    })
    .catch(() => {
      if (requestId !== activeSearchRequestId || query !== activeSearchQuery) return;
      activeSearchItems = [];
      refreshSwitcher();
    })
    .finally(() => {
      if (requestId === activeSearchRequestId) {
        activeSearchInFlight = false;
      }
      if (activeSearchTimer === null) {
        maybeDispatchQueuedContentSearch();
      }
    });
}

function queueContentSearch(query: string) {
  queuedSearchQuery = query;
  clearContentSearchTimer();
  activeSearchTimer = window.setTimeout(() => {
    activeSearchTimer = null;
    maybeDispatchQueuedContentSearch();
  }, CONTENT_SEARCH_DEBOUNCE_MS);
}

function notesForSwitcherSource(): NoteEntry[] {
  if (activeMode === "content" && activeCollectionNotes) {
    return activeCollectionNotes;
  }
  return allNotesForSwitcher();
}

async function refreshCollectionOptionsAndNotes() {
  if (activeMode !== "content") return;
  const collections = await listCollections();
  if (
    activeCollectionFilter &&
    !collections.some((entry) => entry.id === activeCollectionFilter)
  ) {
    if (state.workingCollection?.id === activeCollectionFilter) {
      state.setWorkingCollection(null);
    }
    activeCollectionFilter = null;
  }
  const summaries = await listNotesMetaFiltered(
    activeCollectionFilter,
    state.activeNote?.id ?? null,
  );
  activeCollectionOptions = collections;
  activeCollectionNotes = toNoteEntriesFromSummaries(summaries);
  ensureContentCollectionSelector();
  refreshSwitcher();
}

function ensureContentCollectionSelector() {
  if (activeMode !== "content" || !overlay?.isOpen()) return;
  const panel = document.querySelector(".switcher-panel");
  if (!panel) return;
  let filterRow = panel.querySelector(".switcher-filter-row") as HTMLDivElement | null;
  let selectEl: HTMLSelectElement | null = null;
  if (!filterRow) {
    filterRow = document.createElement("div");
    filterRow.className = "switcher-filter-row";
    const label = document.createElement("label");
    label.className = "switcher-filter-label";
    label.textContent = "Collection";
    selectEl = document.createElement("select");
    selectEl.className = "switcher-filter-select";
    label.appendChild(selectEl);
    filterRow.appendChild(label);
    const listEl = panel.querySelector(".switcher-list");
    panel.insertBefore(filterRow, listEl ?? null);
  } else {
    selectEl = filterRow.querySelector("select");
  }
  if (!selectEl) return;

  selectEl.replaceChildren();
  const allOption = document.createElement("option");
  allOption.value = ALL_COLLECTIONS_TOKEN;
  allOption.textContent = "All";
  selectEl.appendChild(allOption);
  for (const collection of activeCollectionOptions) {
    const option = document.createElement("option");
    option.value = collection.id;
    option.textContent = collection.name;
    selectEl.appendChild(option);
  }
  selectEl.value = activeCollectionFilter ?? ALL_COLLECTIONS_TOKEN;
  selectEl.onchange = () => {
    const value = selectEl!.value;
    activeCollectionFilter = value === ALL_COLLECTIONS_TOKEN ? null : value;
    activeSearchQuery = "";
    activeSearchItems = null;
    activeCollectionNotes = null;
    void refreshCollectionOptionsAndNotes().catch(() => {
      refreshSwitcher();
    });
  };
}

function buildItems(query: string): SwitcherItem[] {
  const allNotes = notesForSwitcherSource();
  const trimmed = query.trim();
  if (trimmed.length === 0) {
    resetContentSearchState();
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
      lineNumber: Number.isFinite(result.line_number) ? result.line_number : undefined,
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
  selectCallback: (id: string, lineNumber?: number | null) => void,
  deleteCallback?: DeleteCallback,
  mode: SwitcherMode = "title",
) {
  onSelectCallback = selectCallback;
  onDeleteCallback = deleteCallback ?? null;

  if (mode !== activeMode) {
    overlay?.close();
    overlay = null;
    activeMode = mode;
    resetContentSearchState();
  }
  if (mode === "content") {
    resetContentCollectionFilterFromWorkingCollection();
    activeCollectionOptions = [];
    activeCollectionNotes = null;
    void refreshCollectionOptionsAndNotes().catch(() => {
      refreshSwitcher();
    });
  } else {
    activeCollectionFilter = null;
    activeCollectionOptions = [];
    activeCollectionNotes = null;
  }

  if (!overlay) {
    const placeholder = mode === "content" ? "Search note content..." : "Search notes...";
    overlay = createListOverlay<SwitcherItem>({
      classPrefix: "switcher",
      placeholder,
      backdrop: true,
      getItems: buildItems,
      renderItem: renderSwitcherItem,
      onSelect: ({ item, lineNumber }) => onSelectCallback?.(item.id, lineNumber ?? null),
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
  if (mode === "content") {
    ensureContentCollectionSelector();
  }
}

export function closeSwitcher() {
  overlay?.close();
  onDeleteCallback = null;
  resetContentSearchState();
  activeCollectionFilter = null;
  activeCollectionOptions = [];
  activeCollectionNotes = null;
}

export function refreshSwitcher() {
  if (!isSwitcherOpen()) return;
  overlay?.refresh();
}

export const __switcherInternals = {
  resetContentCollectionFilterFromWorkingCollectionForTest() {
    resetContentCollectionFilterFromWorkingCollection();
  },
  setActiveCollectionFilterForTest(next: string | null) {
    activeCollectionFilter = next;
  },
  getActiveCollectionFilterForTest() {
    return activeCollectionFilter;
  },
};
