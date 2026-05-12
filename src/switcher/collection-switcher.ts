import { listCollections, type Collection } from "../api";
import { state } from "../state";
import { createListOverlay, type ListOverlay } from "../overlays/overlay.ts";
import { fuzzyFilter } from "./fuzzy";
import { highlightPositions } from "../ui/escape.ts";

type CollectionPickerItem = {
  id: string | null;
  name: string;
  description: string;
  isAll: boolean;
};

type CollectionPickerCallbacks = {
  onChoose: (collection: Collection | null) => void;
  onEdit: (collection: Collection) => void;
};

let overlay: ListOverlay | null = null;
let callbacks: CollectionPickerCallbacks | null = null;
let items: CollectionPickerItem[] = [];

function toCollectionResult(item: CollectionPickerItem): Collection | null {
  if (item.isAll || !item.id) return null;
  return {
    id: item.id,
    name: item.name,
    description: item.description,
    created_at: "",
    updated_at: "",
  };
}

function isCollectionEditShortcut(event: KeyboardEvent): boolean {
  return event.ctrlKey && !event.shiftKey && !event.altKey && event.key.toLowerCase() === "e";
}

function toItem(collection: Collection): CollectionPickerItem {
  return {
    id: collection.id,
    name: collection.name,
    description: collection.description,
    isAll: false,
  };
}

function buildItems(query: string): Array<{ item: CollectionPickerItem; positions: number[] }> {
  const trimmed = query.trim();
  if (!trimmed) {
    return items.map((item) => ({ item, positions: [] }));
  }
  return fuzzyFilter(trimmed, items, (entry) =>
    `${entry.name}${entry.description ? ` ${entry.description}` : ""}`
  );
}

function renderItem(
  payload: { item: CollectionPickerItem; positions: number[] },
  selected: boolean,
): HTMLElement {
  const { item, positions } = payload;
  const row = document.createElement("div");
  row.className = "switcher-item" + (selected ? " switcher-item--active" : "");
  row.setAttribute("role", "option");
  row.setAttribute("aria-selected", selected ? "true" : "false");

  const title = document.createElement("span");
  title.className = "switcher-item-title";
  title.appendChild(highlightPositions(item.name, positions));
  row.appendChild(title);

  if (item.isAll) {
    const badge = document.createElement("span");
    badge.className = "switcher-item-badge switcher-item-badge--current";
    badge.textContent = "clear";
    row.appendChild(badge);
  } else if (item.id === state.workingCollection?.id) {
    const badge = document.createElement("span");
    badge.className = "switcher-item-badge switcher-item-badge--current";
    badge.textContent = "active";
    row.appendChild(badge);
  }

  if (item.description.trim().length > 0) {
    const desc = document.createElement("span");
    desc.className = "switcher-item-snippet";
    desc.textContent = item.description;
    row.appendChild(desc);
  }

  return row;
}

async function refreshItems() {
  const collections = await listCollections();
  const all: CollectionPickerItem = {
    id: null,
    name: "All collections",
    description: "Clear active session collection",
    isAll: true,
  };
  items = [all, ...collections.map(toItem)];
  overlay?.refresh();
}

export function isCollectionSwitcherOpen(): boolean {
  return overlay?.isOpen() ?? false;
}

export function closeCollectionSwitcher() {
  overlay?.close();
}

export function openCollectionSwitcher(nextCallbacks: CollectionPickerCallbacks) {
  callbacks = nextCallbacks;
  if (!overlay) {
    overlay = createListOverlay<{ item: CollectionPickerItem; positions: number[] }>({
      classPrefix: "switcher",
      placeholder: "Search collections...",
      backdrop: true,
      getItems: buildItems,
      renderItem,
      onSelect: ({ item }) => {
        if (!callbacks) return;
        callbacks.onChoose(toCollectionResult(item));
      },
      onKeydown: (event, overlayState) => {
        if (!isCollectionEditShortcut(event)) {
          return false;
        }
        event.preventDefault();
        event.stopPropagation();
        const selected = overlayState.items[overlayState.selectedIndex];
        if (!selected || !callbacks) {
          return true;
        }
        const collection = toCollectionResult(selected.item);
        if (!collection) {
          return true;
        }
        overlay?.close();
        callbacks.onEdit(collection);
        return true;
      },
      emptyMessage: "No collections",
    });
  }
  overlay.open();
  void refreshItems().catch(() => {
    overlay?.refresh();
  });
}

export const __collectionSwitcherInternals = {
  setItemsForTest(nextItems: CollectionPickerItem[]) {
    items = nextItems;
  },
  buildItemsForTest(query: string) {
    return buildItems(query);
  },
  toCollectionResultForTest(item: CollectionPickerItem) {
    return toCollectionResult(item);
  },
  isCollectionEditShortcutForTest(event: KeyboardEvent) {
    return isCollectionEditShortcut(event);
  },
};
