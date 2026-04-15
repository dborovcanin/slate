import { EditorView } from "@codemirror/view";
import { executeCommand, listCommandSuggestions, type CommandMode, type CommandSuggestion } from "./command-engine";
import { insertAtSelection } from "./editor-utils";
import { createListOverlay, type ListOverlay, type ListOverlayState } from "../overlays/overlay.ts";

interface CommandPickerOptions {
  mode: CommandMode;
  dateFormat?: string;
  dateTimeFormat?: string;
  onExitCommand?: () => Promise<void> | void;
  source?: "vim-colon" | "shortcut";
}

interface CommandModeExtensionOptions {
  vimMode?: boolean;
  dateFormat?: string;
  dateTimeFormat?: string;
  onExitCommand?: () => Promise<void> | void;
}

const COMMAND_PICKER_SELECTOR = ".command-picker-bar";

function normalizeCommand(rawInput: string): string {
  return rawInput.trim().replace(/^:/, "").toLowerCase();
}

function isInsideCommandPicker(target: EventTarget | null): boolean {
  return target instanceof Element && target.closest(COMMAND_PICKER_SELECTOR) !== null;
}

function isCtrlColon(event: KeyboardEvent): boolean {
  if (!event.ctrlKey || event.altKey || event.metaKey) return false;
  if (event.key === ":") return true;
  return event.code === "Semicolon" && event.shiftKey;
}

function shouldReinsertLiteralOnCancel(rawInput: string): boolean {
  return /^:+$/.test(rawInput.trim());
}

function showStatus(view: EditorView, message: string) {
  let statusEl = view.dom.querySelector(".command-picker-status") as HTMLDivElement | null;
  if (!statusEl) {
    statusEl = document.createElement("div");
    statusEl.className = "command-picker-status";
    view.dom.appendChild(statusEl);
  }
  statusEl.textContent = message;
  statusEl.classList.add("visible");
  const prev = Number(statusEl.dataset.hideTimer ?? "");
  if (Number.isFinite(prev)) window.clearTimeout(prev);
  const timer = window.setTimeout(() => statusEl?.classList.remove("visible"), 1800);
  statusEl.dataset.hideTimer = `${timer}`;
}

function renderCommandItem(suggestion: CommandSuggestion, selected: boolean): HTMLElement {
  const item = document.createElement("button");
  item.type = "button";
  item.className = "command-picker-item" + (selected ? " command-picker-item--active" : "");
  item.setAttribute("role", "option");
  item.setAttribute("aria-selected", selected ? "true" : "false");

  const title = document.createElement("span");
  title.className = "command-picker-item-title";
  title.textContent = suggestion.value;

  const hint = document.createElement("span");
  hint.className = "command-picker-item-hint";
  hint.textContent = suggestion.description;

  item.appendChild(title);
  item.appendChild(hint);
  return item;
}

export function isCommandPickerOpen(view: EditorView): boolean {
  return view.dom.querySelector(COMMAND_PICKER_SELECTOR) !== null;
}

export function openCommandPicker(view: EditorView, options: CommandPickerOptions) {
  if (isCommandPickerOpen(view)) return;

  // Prefix decoration (":") added to the bar after overlay creation
  const prefixEl = document.createElement("span");
  prefixEl.className = "command-picker-prefix";
  prefixEl.textContent = ":";

  let pickerOverlay: ListOverlay | null = null;

  const submit = async (command: string) => {
    pickerOverlay?.close();
    view.focus();
    if (!command) return;
    try {
      const message = await executeCommand(view, command, {
        mode: options.mode,
        dateFormat: options.dateFormat,
        dateTimeFormat: options.dateTimeFormat,
        onExitCommand: options.onExitCommand,
      });
      if (message) showStatus(view, message);
      view.focus();
    } catch (err) {
      console.error("Command failed:", err);
      showStatus(view, "command failed");
      view.focus();
    }
  };

  const pickCommand = (state: ListOverlayState<CommandSuggestion>): string => {
    const normalized = normalizeCommand(state.query);
    if (!normalized && state.items.length > 0) {
      return state.items[Math.max(state.selectedIndex, 0)]?.value ?? "";
    }
    if (!normalized) return "";
    if (state.items.some((s) => s.value === normalized)) return normalized;
    return state.items[Math.max(state.selectedIndex, 0)]?.value ?? normalized;
  };

  pickerOverlay = createListOverlay<CommandSuggestion>({
    container: view.dom,
    classPrefix: "command-picker",
    placeholder: "command",
    backdrop: false,
    getItems: (query) => listCommandSuggestions(options.mode, query),
    renderItem: renderCommandItem,
    onSelect: (suggestion) => { void submit(suggestion.value); },
    onClose: (query) => {
      view.focus();
      if (options.source === "vim-colon" && shouldReinsertLiteralOnCancel(query)) {
        insertAtSelection(view, `:${query}`);
      }
    },
    emptyMessage: "No commands",
    onKeydown: (event, state) => {
      if (event.key === "Tab") {
        if (state.items.length === 0) return false;
        event.preventDefault();
        event.stopPropagation();
        const best = state.items[Math.max(state.selectedIndex, 0)];
        if (best) state.inputEl.value = best.value;
        state.refresh();
        return true;
      }
      if (event.key === "Enter") {
        event.preventDefault();
        event.stopPropagation();
        void submit(pickCommand(state));
        return true;
      }
      return false;
    },
  });

  // Insert the `:` prefix before the input element
  pickerOverlay.open();
  const bar = view.dom.querySelector(".command-picker-bar");
  if (bar) bar.prepend(prefixEl);
}

export function commandModeExtension(options: CommandModeExtensionOptions = {}) {
  return EditorView.domEventHandlers({
    keydown: (event, view) => {
      if (options.vimMode) return false;
      if (isInsideCommandPicker(event.target)) return false;
      if (!isCtrlColon(event)) return false;
      event.preventDefault();
      openCommandPicker(view, {
        mode: "editor",
        dateFormat: options.dateFormat,
        dateTimeFormat: options.dateTimeFormat,
        onExitCommand: options.onExitCommand,
        source: "shortcut",
      });
      return true;
    },
  });
}
