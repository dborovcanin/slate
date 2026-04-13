import { state, type NoteEntry } from "../state";
import { highlightPositions } from "../ui/escape.ts";
import { createListOverlay, type ListOverlay } from "../overlays/overlay.ts";
import { fuzzyFilter } from "./fuzzy";

type SwitcherItem = { item: NoteEntry; positions: number[] };

let overlay: ListOverlay | null = null;
let onSelectCallback: ((id: string) => void) | null = null;

function buildItems(query: string): SwitcherItem[] {
  const notes = state.notes.filter((n) => n.id !== state.activeNote?.id);
  const allNotes: NoteEntry[] = state.activeNote
    ? [
        {
          id: state.activeNote.id,
          title: state.notes.find((n) => n.id === state.activeNote!.id)?.title ?? "Untitled",
          updatedAt: state.activeNote.updated_at,
        },
        ...notes,
      ]
    : notes;
  return fuzzyFilter(query, allNotes, (n) => n.title);
}

function renderSwitcherItem(
  { item, positions }: SwitcherItem,
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

  if (item.id === state.activeNote?.id) {
    const badge = document.createElement("span");
    badge.className = "switcher-item-badge";
    badge.textContent = "current";
    row.appendChild(badge);
  }

  return row;
}

export function isSwitcherOpen(): boolean {
  return overlay?.isOpen() ?? false;
}

export function openSwitcher(selectCallback: (id: string) => void) {
  onSelectCallback = selectCallback;

  if (!overlay) {
    overlay = createListOverlay<SwitcherItem>({
      classPrefix: "switcher",
      placeholder: "Search notes...",
      backdrop: true,
      getItems: buildItems,
      renderItem: renderSwitcherItem,
      onSelect: ({ item }) => onSelectCallback?.(item.id),
      emptyMessage: "No notes found",
    });
  }

  overlay.open();
}

export function closeSwitcher() {
  overlay?.close();
}

export function refreshSwitcher() {
  if (!isSwitcherOpen()) return;
  overlay?.refresh();
}
