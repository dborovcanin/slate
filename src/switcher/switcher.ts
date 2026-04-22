import { state, type NoteEntry } from "../state";
import { highlightPositions } from "../ui/escape.ts";
import { createListOverlay, type ListOverlay } from "../overlays/overlay.ts";
import { fuzzyFilter } from "./fuzzy";

type SwitcherItem = { item: NoteEntry; positions: number[] };
type DeleteCallback = (id: string) => void;

let overlay: ListOverlay | null = null;
let onSelectCallback: ((id: string) => void) | null = null;
let onDeleteCallback: DeleteCallback | null = null;

function buildItems(query: string): SwitcherItem[] {
  const notes = state.notes.filter((n) => n.id !== state.activeNote?.id);
  const allNotes: NoteEntry[] = state.activeNote
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
}

export function refreshSwitcher() {
  if (!isSwitcherOpen()) return;
  overlay?.refresh();
}
