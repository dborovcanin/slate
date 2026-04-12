import { state, type NoteEntry } from "../state";
import { fuzzyFilter } from "./fuzzy";

let overlay: HTMLElement | null = null;
let input: HTMLInputElement | null = null;
let listEl: HTMLElement | null = null;
let selectedIndex = 0;
let filteredItems: { item: NoteEntry; positions: number[] }[] = [];
let onSelect: ((id: string) => void) | null = null;

export function isSwitcherOpen(): boolean {
  return overlay !== null && !overlay.hidden;
}

export function openSwitcher(selectCallback: (id: string) => void) {
  onSelect = selectCallback;

  if (!overlay) {
    createDOM();
  }

  overlay!.hidden = false;
  input!.value = "";
  selectedIndex = 0;
  updateList("");
  input!.focus();
}

export function closeSwitcher() {
  if (overlay) overlay.hidden = true;
}

function createDOM() {
  overlay = document.createElement("div");
  overlay.className = "switcher-overlay";
  overlay.addEventListener("mousedown", (e) => {
    if (e.target === overlay) closeSwitcher();
  });

  const panel = document.createElement("div");
  panel.className = "switcher-panel";

  input = document.createElement("input");
  input.className = "switcher-input";
  input.type = "text";
  input.placeholder = "Search notes...";
  input.addEventListener("input", () => {
    selectedIndex = 0;
    updateList(input!.value);
  });
  input.addEventListener("keydown", handleKeydown);

  listEl = document.createElement("div");
  listEl.className = "switcher-list";

  panel.appendChild(input);
  panel.appendChild(listEl);
  overlay.appendChild(panel);
  document.body.appendChild(overlay);
}

function handleKeydown(e: KeyboardEvent) {
  switch (e.key) {
    case "ArrowDown":
      e.preventDefault();
      selectedIndex = Math.min(selectedIndex + 1, filteredItems.length - 1);
      renderList();
      break;
    case "ArrowUp":
      e.preventDefault();
      selectedIndex = Math.max(selectedIndex - 1, 0);
      renderList();
      break;
    case "Enter":
      e.preventDefault();
      if (filteredItems[selectedIndex]) {
        const id = filteredItems[selectedIndex].item.id;
        closeSwitcher();
        onSelect?.(id);
      }
      break;
    case "Escape":
      e.preventDefault();
      closeSwitcher();
      break;
  }
}

function updateList(query: string) {
  const notes = state.notes.filter((n) => n.id !== state.activeNote?.id);
  const allNotes = state.activeNote
    ? [
        {
          id: state.activeNote.id,
          title:
            state.notes.find((n) => n.id === state.activeNote!.id)?.title ??
            "Untitled",
          updatedAt: state.activeNote.updated_at,
        },
        ...notes,
      ]
    : notes;

  filteredItems = fuzzyFilter(query, allNotes, (n) => n.title);
  renderList();
}

function renderList() {
  if (!listEl) return;
  listEl.innerHTML = "";

  if (filteredItems.length === 0) {
    const empty = document.createElement("div");
    empty.className = "switcher-empty";
    empty.textContent = "No notes found";
    listEl.appendChild(empty);
    return;
  }

  filteredItems.forEach(({ item, positions }, i) => {
    const row = document.createElement("div");
    row.className =
      "switcher-item" + (i === selectedIndex ? " switcher-item--active" : "");

    const title = document.createElement("span");
    title.className = "switcher-item-title";
    title.innerHTML = highlightPositions(item.title, positions);

    const isActive = item.id === state.activeNote?.id;
    if (isActive) {
      const badge = document.createElement("span");
      badge.className = "switcher-item-badge";
      badge.textContent = "current";
      row.appendChild(title);
      row.appendChild(badge);
    } else {
      row.appendChild(title);
    }

    row.addEventListener("click", () => {
      closeSwitcher();
      onSelect?.(item.id);
    });

    listEl!.appendChild(row);
  });

  const activeEl = listEl.querySelector(".switcher-item--active");
  activeEl?.scrollIntoView({ block: "nearest" });
}

function highlightPositions(text: string, positions: number[]): string {
  if (positions.length === 0) return escapeHtml(text);

  const posSet = new Set(positions);
  let result = "";
  let inHighlight = false;

  for (let i = 0; i < text.length; i++) {
    const shouldHighlight = posSet.has(i);
    if (shouldHighlight && !inHighlight) {
      result += "<b>";
      inHighlight = true;
    } else if (!shouldHighlight && inHighlight) {
      result += "</b>";
      inHighlight = false;
    }
    result += escapeHtml(text[i]);
  }
  if (inHighlight) result += "</b>";
  return result;
}

function escapeHtml(s: string): string {
  return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
}
