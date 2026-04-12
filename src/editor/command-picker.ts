import { EditorView } from "@codemirror/view";
import { executeCommand, listCommandSuggestions, type CommandMode } from "./command-engine";
import { insertAtSelection } from "./editor-utils";

interface CommandPickerOptions {
  mode: CommandMode;
  dateFormat?: string;
  onExitCommand?: () => Promise<void> | void;
  source?: "vim-colon" | "shortcut";
}

interface CommandModeExtensionOptions {
  vimMode?: boolean;
  dateFormat?: string;
  onExitCommand?: () => Promise<void> | void;
}

const COMMAND_PICKER_SELECTOR = ".command-picker-bar";

function normalizeCommand(rawInput: string): string {
  return rawInput.trim().replace(/^:/, "").toLowerCase();
}

function isInsideCommandPicker(target: EventTarget | null): boolean {
  return target instanceof Element && target.closest(COMMAND_PICKER_SELECTOR) !== null;
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
  const timer = window.setTimeout(() => {
    statusEl?.classList.remove("visible");
  }, 1800);
  statusEl.dataset.hideTimer = `${timer}`;
}

function shouldReinsertLiteralOnCancel(rawInput: string): boolean {
  return /^:+$/.test(rawInput.trim());
}

function isCtrlColon(event: KeyboardEvent): boolean {
  if (!event.ctrlKey || event.altKey || event.metaKey) return false;
  if (event.key === ":") return true;
  return event.code === "Semicolon" && event.shiftKey;
}

export function isCommandPickerOpen(view: EditorView): boolean {
  return view.dom.querySelector(COMMAND_PICKER_SELECTOR) !== null;
}

export function openCommandPicker(view: EditorView, options: CommandPickerOptions) {
  if (isCommandPickerOpen(view)) return;

  const bar = document.createElement("div");
  bar.className = "command-picker-bar";

  const prefix = document.createElement("span");
  prefix.className = "command-picker-prefix";
  prefix.textContent = ":";

  const input = document.createElement("input");
  input.className = "command-picker-input";
  input.type = "text";
  input.spellcheck = false;
  input.autocapitalize = "off";
  input.autocomplete = "off";
  input.setAttribute("autocorrect", "off");
  input.placeholder = "command";

  const list = document.createElement("div");
  list.className = "command-picker-list";

  bar.appendChild(prefix);
  bar.appendChild(input);
  bar.appendChild(list);
  view.dom.appendChild(bar);

  let suggestions = listCommandSuggestions(options.mode, "");
  let selectedIndex = suggestions.length > 0 ? 0 : -1;

  const close = () => {
    bar.remove();
    view.focus();
  };

  const renderSuggestions = () => {
    list.replaceChildren();
    suggestions = listCommandSuggestions(options.mode, input.value);
    if (suggestions.length === 0) {
      selectedIndex = -1;
      const empty = document.createElement("div");
      empty.className = "command-picker-empty";
      empty.textContent = "No commands";
      list.appendChild(empty);
      return;
    }

    if (selectedIndex < 0 || selectedIndex >= suggestions.length) selectedIndex = 0;
    suggestions.forEach((suggestion, idx) => {
      const item = document.createElement("button");
      item.type = "button";
      item.className = "command-picker-item";
      if (idx === selectedIndex) item.classList.add("command-picker-item--active");

      const title = document.createElement("span");
      title.className = "command-picker-item-title";
      title.textContent = suggestion.value;

      const hint = document.createElement("span");
      hint.className = "command-picker-item-hint";
      hint.textContent = suggestion.description;

      item.appendChild(title);
      item.appendChild(hint);
      item.addEventListener("mouseenter", () => {
        selectedIndex = idx;
        renderSuggestions();
      });
      item.addEventListener("mousedown", (event) => {
        event.preventDefault();
      });
      item.addEventListener("click", () => {
        input.value = suggestion.value;
        void submit();
      });
      list.appendChild(item);
    });
  };

  const pickCommand = (): string => {
    const normalized = normalizeCommand(input.value);
    if (!normalized && suggestions.length > 0) {
      return suggestions[Math.max(selectedIndex, 0)]?.value ?? "";
    }
    if (!normalized) return "";
    if (suggestions.some((suggestion) => suggestion.value === normalized)) return normalized;
    return suggestions[Math.max(selectedIndex, 0)]?.value ?? normalized;
  };

  const submit = async () => {
    const command = pickCommand();
    close();
    if (!command) return;

    try {
      const message = await executeCommand(view, command, {
        mode: options.mode,
        dateFormat: options.dateFormat,
        onExitCommand: options.onExitCommand,
      });
      if (message) showStatus(view, message);
    } catch (err) {
      console.error("Command failed:", err);
      showStatus(view, "command failed");
    }
  };

  input.addEventListener("input", () => {
    renderSuggestions();
  });

  input.addEventListener("keydown", (event) => {
    if (event.key === "Escape") {
      event.preventDefault();
      event.stopPropagation();
      const rawInput = input.value;
      close();
      if (options.source === "vim-colon" && shouldReinsertLiteralOnCancel(rawInput)) {
        insertAtSelection(view, `:${rawInput}`);
      }
      return;
    }

    if (event.key === "Enter") {
      event.preventDefault();
      event.stopPropagation();
      void submit();
      return;
    }

    if (event.key === "Tab") {
      if (suggestions.length === 0) return;
      event.preventDefault();
      event.stopPropagation();
      input.value = suggestions[Math.max(selectedIndex, 0)]?.value ?? input.value;
      renderSuggestions();
      return;
    }

    if (event.key === "ArrowDown") {
      if (suggestions.length === 0) return;
      event.preventDefault();
      event.stopPropagation();
      selectedIndex = (selectedIndex + 1 + suggestions.length) % suggestions.length;
      renderSuggestions();
      return;
    }

    if (event.key === "ArrowUp") {
      if (suggestions.length === 0) return;
      event.preventDefault();
      event.stopPropagation();
      selectedIndex = (selectedIndex - 1 + suggestions.length) % suggestions.length;
      renderSuggestions();
    }
  });

  renderSuggestions();
  window.setTimeout(() => input.focus(), 0);
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
        onExitCommand: options.onExitCommand,
        source: "shortcut",
      });
      return true;
    },
  });
}
