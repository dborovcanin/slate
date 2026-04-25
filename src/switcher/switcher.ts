import { state, type NoteEntry } from "../state";
import { searchNotesContent, type NoteSummary } from "../api";
import { highlightPositions } from "../ui/escape.ts";
import { createListOverlay, type ListOverlay } from "../overlays/overlay.ts";
import { fuzzyFilter, fuzzyMatch } from "./fuzzy";

type SwitcherItem = {
  item: NoteEntry;
  positions: number[];
  matchedByContent: boolean;
};
type DeleteCallback = (id: string) => void;

let overlay: ListOverlay | null = null;
let onSelectCallback: ((id: string) => void) | null = null;
let onDeleteCallback: DeleteCallback | null = null;
let activeSearchQuery = "";
let activeSearchItems: NoteEntry[] | null = null;
let activeSearchRequestId = 0;

function mapSummaryToEntry(summary: NoteSummary): NoteEntry {
  return {
    id: summary.id,
    title: summary.title,
    accessMode: summary.access_mode,
    isUnlocked: summary.is_unlocked,
    updatedAt: summary.updated_at,
  };
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
    .then((summaries) => {
      if (requestId !== activeSearchRequestId || query !== activeSearchQuery) return;
      activeSearchItems = summaries.map(mapSummaryToEntry);
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
  return activeSearchItems
    .map((entry) => {
      const item = itemsById.get(entry.id) ?? entry;
      const titleMatch = fuzzyMatch(trimmed, item.title);
      return {
        item,
        positions: titleMatch?.positions ?? [],
        matchedByContent: titleMatch === null,
      };
    });
}

function renderSwitcherItem(
  { item, positions, matchedByContent }: SwitcherItem,
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

export function openSwitcher(selectCallback: (id: string) => void, deleteCallback?: DeleteCallback) {
  onSelectCallback = selectCallback;
  onDeleteCallback = deleteCallback ?? null;

  if (!overlay) {
    overlay = createListOverlay<SwitcherItem>({
      classPrefix: "switcher",
      placeholder: "Search notes...",
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
