/**
 * Generic list overlay: input + filtered list + keyboard navigation.
 * Used by the note switcher and command picker.
 */

export interface ListOverlayOptions<T> {
  /** Parent element to attach to. Defaults to document.body. */
  container?: HTMLElement;
  /** CSS class prefix for generated elements. */
  classPrefix: string;
  /** Input placeholder text. */
  placeholder?: string;
  /** Whether to show a full-screen backdrop div that closes on click-outside. */
  backdrop?: boolean;
  /** Compute the item list from the current query string. */
  getItems: (query: string) => T[];
  /** Build a DOM element for a single item. */
  renderItem: (item: T, selected: boolean, index: number) => HTMLElement;
  /** Called when the user selects an item. */
  onSelect: (item: T, query: string) => void;
  /** Called when the overlay is closed without selecting. */
  onClose?: (query: string) => void;
  /** Text shown when the filtered list is empty. */
  emptyMessage?: string;
  /** Extra keydown handler on the input. Return true to prevent default navigation. */
  onKeydown?: (event: KeyboardEvent, state: ListOverlayState<T>) => boolean;
  /** Restore focus to the previously focused element when closing. Defaults to true. */
  restoreFocus?: boolean;
}

export interface ListOverlayState<T> {
  query: string;
  items: T[];
  selectedIndex: number;
  inputEl: HTMLInputElement;
  refresh: () => void;
}

export interface ListOverlay {
  /** Mount and show the overlay. */
  open: () => void;
  /** Hide and remove the overlay from DOM. */
  close: () => void;
  isOpen: () => boolean;
  /** Re-run getItems with the current query and re-render. */
  refresh: () => void;
}

let overlaySequence = 0;

export function createListOverlay<T>(options: ListOverlayOptions<T>): ListOverlay {
  const {
    container,
    classPrefix: p,
    placeholder = "",
    backdrop = false,
    getItems,
    renderItem,
    onSelect,
    onClose,
    emptyMessage = "No results",
    onKeydown,
    restoreFocus = true,
  } = options;

  let root: HTMLElement | null = null;
  let inputEl: HTMLInputElement | null = null;
  let listEl: HTMLElement | null = null;
  let restoreTarget: HTMLElement | null = null;
  let items: T[] = [];
  let selectedIndex = 0;
  const listboxId = `${p}-listbox-${overlaySequence++}`;

  function getQuery(): string {
    return inputEl?.value ?? "";
  }

  function renderList() {
    if (!listEl) return;
    listEl.replaceChildren();

    if (items.length === 0) {
      inputEl?.removeAttribute("aria-activedescendant");
      const empty = document.createElement("div");
      empty.className = `${p}-empty`;
      empty.textContent = emptyMessage;
      listEl.appendChild(empty);
      return;
    }

    items.forEach((item, i) => {
      const el = renderItem(item, i === selectedIndex, i);
      el.id = `${listboxId}-item-${i}`;
      if (!el.hasAttribute("role")) {
        el.setAttribute("role", "option");
      }
      el.setAttribute("aria-selected", i === selectedIndex ? "true" : "false");
      el.addEventListener("mouseenter", () => {
        selectedIndex = i;
        renderList();
      });
      el.addEventListener("mousedown", (e) => e.preventDefault());
      el.addEventListener("click", () => {
        const query = getQuery();
        close();
        onSelect(item, query);
      });
      listEl!.appendChild(el);
    });

    inputEl?.setAttribute("aria-activedescendant", `${listboxId}-item-${selectedIndex}`);
    listEl.querySelector(`.${p}-item--active`)?.scrollIntoView({ block: "nearest" });
  }

  function refresh() {
    const query = getQuery();
    items = getItems(query);
    selectedIndex = Math.min(selectedIndex, Math.max(items.length - 1, 0));
    renderList();
  }

  function handleKeydown(event: KeyboardEvent) {
    const state: ListOverlayState<T> = {
      query: getQuery(),
      items,
      selectedIndex,
      inputEl: inputEl!,
      refresh,
    };

    if (onKeydown?.(event, state)) {
      selectedIndex = state.selectedIndex;
      return;
    }

    switch (event.key) {
      case "Escape":
        event.preventDefault();
        event.stopPropagation();
        {
          const query = getQuery();
          close();
          onClose?.(query);
        }
        break;
      case "ArrowDown":
        if (items.length === 0) break;
        event.preventDefault();
        event.stopPropagation();
        selectedIndex = (selectedIndex + 1) % items.length;
        renderList();
        break;
      case "ArrowUp":
        if (items.length === 0) break;
        event.preventDefault();
        event.stopPropagation();
        selectedIndex = (selectedIndex - 1 + items.length) % items.length;
        renderList();
        break;
      case "Enter":
        event.preventDefault();
        event.stopPropagation();
        if (items[selectedIndex] !== undefined) {
          const query = getQuery();
          const item = items[selectedIndex]!;
          close();
          onSelect(item, query);
        }
        break;
    }
  }

  function open() {
    if (root) return;

    const parent = container ?? document.body;
    restoreTarget =
      restoreFocus && document.activeElement instanceof HTMLElement
        ? document.activeElement
        : null;

    if (backdrop) {
      root = document.createElement("div");
      root.className = `${p}-overlay`;
      root.setAttribute("role", "dialog");
      root.setAttribute("aria-modal", "true");
      root.addEventListener("mousedown", (e) => {
        if (e.target === root) {
          const query = getQuery();
          close();
          onClose?.(query);
        }
      });

      const panel = document.createElement("div");
      panel.className = `${p}-panel`;
      root.appendChild(panel);

      inputEl = document.createElement("input");
      inputEl.className = `${p}-input`;
      inputEl.type = "text";
      inputEl.placeholder = placeholder;
      inputEl.setAttribute("role", "combobox");
      inputEl.setAttribute("aria-autocomplete", "list");
      inputEl.setAttribute("aria-expanded", "true");
      inputEl.setAttribute("aria-controls", listboxId);

      listEl = document.createElement("div");
      listEl.className = `${p}-list`;
      listEl.id = listboxId;
      listEl.setAttribute("role", "listbox");

      panel.appendChild(inputEl);
      panel.appendChild(listEl);
      parent.appendChild(root);
    } else {
      root = document.createElement("div");
      root.className = `${p}-bar`;
      root.setAttribute("role", "dialog");
      root.setAttribute("aria-modal", "true");

      inputEl = document.createElement("input");
      inputEl.className = `${p}-input`;
      inputEl.type = "text";
      inputEl.placeholder = placeholder;
      inputEl.spellcheck = false;
      inputEl.autocomplete = "off";
      inputEl.setAttribute("autocorrect", "off");
      inputEl.setAttribute("autocapitalize", "off");
      inputEl.setAttribute("role", "combobox");
      inputEl.setAttribute("aria-autocomplete", "list");
      inputEl.setAttribute("aria-expanded", "true");
      inputEl.setAttribute("aria-controls", listboxId);

      listEl = document.createElement("div");
      listEl.className = `${p}-list`;
      listEl.id = listboxId;
      listEl.setAttribute("role", "listbox");

      root.appendChild(inputEl);
      root.appendChild(listEl);
      parent.appendChild(root);
    }

    inputEl.addEventListener("input", () => {
      selectedIndex = 0;
      refresh();
    });
    inputEl.addEventListener("keydown", handleKeydown);

    selectedIndex = 0;
    refresh();
    window.setTimeout(() => inputEl?.focus(), 0);
  }

  function close() {
    if (!root) return;
    root.remove();
    root = null;
    inputEl = null;
    listEl = null;
    if (restoreFocus && restoreTarget && restoreTarget.isConnected) {
      restoreTarget.focus();
    }
    restoreTarget = null;
  }

  return {
    open,
    close,
    isOpen: () => root !== null,
    refresh,
  };
}
