import { EditorView } from "@codemirror/view";
import { executeExCommand, suggestExCommands } from "./ex-commands";
import { openDatePicker } from "./date-picker";
import { insertAtSelection } from "./editor-utils";
import { forceQuit } from "../app";

interface CommandPickerOptions {
  dateFormat?: string;
}

interface CommandModeExtensionOptions extends CommandPickerOptions {
  vimMode?: boolean;
}

const COMMAND_PICKER_SELECTOR = ".command-picker-bar";

const commandHint: Record<string, string> = {
  sum: "sum current scope",
  "sum list": "sum current list",
  "sum table": "sum current table",
  "sum doc": "sum whole document",
  sum_all: "sum whole document",
  date: "insert picked date",
  "q!": "quit without saving",
};

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

export function shouldReinsertLiteralOnCancel(rawInput: string): boolean {
  const trimmed = rawInput.trim();
  return trimmed.length === 0 || /^:+$/.test(trimmed);
}

export function isCommandPickerOpen(view: EditorView): boolean {
  return view.dom.querySelector(COMMAND_PICKER_SELECTOR) !== null;
}

export function openCommandPicker(view: EditorView, options: CommandPickerOptions = {}) {
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

  let suggestions = suggestExCommands("");
  let selectedIndex = suggestions.length > 0 ? 0 : -1;

  const close = () => {
    bar.remove();
    view.focus();
  };

  const renderSuggestions = () => {
    list.replaceChildren();
    const query = input.value;
    suggestions = suggestExCommands(query);
    if (suggestions.length === 0) {
      selectedIndex = -1;
      const empty = document.createElement("div");
      empty.className = "command-picker-empty";
      empty.textContent = "No commands";
      list.appendChild(empty);
      return;
    }

    if (selectedIndex < 0 || selectedIndex >= suggestions.length) selectedIndex = 0;

    suggestions.forEach((command, idx) => {
      const item = document.createElement("button");
      item.type = "button";
      item.className = "command-picker-item";
      if (idx === selectedIndex) item.classList.add("command-picker-item--active");

      const title = document.createElement("span");
      title.className = "command-picker-item-title";
      title.textContent = command;

      const hint = document.createElement("span");
      hint.className = "command-picker-item-hint";
      hint.textContent = commandHint[command] ?? "";

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
        input.value = command;
        void submit();
      });
      list.appendChild(item);
    });
  };

  const pickCommand = (): string => {
    const normalized = normalizeCommand(input.value);
    if (!normalized && suggestions.length > 0) {
      return suggestions[Math.max(selectedIndex, 0)] ?? "";
    }
    if (!normalized) return "";
    if (suggestions.includes(normalized as (typeof suggestions)[number])) return normalized;
    if (suggestions.length > 0) return suggestions[Math.max(selectedIndex, 0)] ?? normalized;
    return normalized;
  };

  const submit = async () => {
    const command = pickCommand();
    close();
    if (!command) return;

    if (command === "q!") {
      forceQuit();
      return;
    }

    if (command === "date") {
      try {
        const value = await openDatePicker(options.dateFormat ?? "%Y-%m-%d");
        if (!value) {
          showStatus(view, "date cancelled");
          return;
        }
        insertAtSelection(view, value);
        showStatus(view, `inserted ${value}`);
      } catch (err) {
        console.error("Date command failed:", err);
        showStatus(view, "date command failed");
      }
      return;
    }

    try {
      const message = await executeExCommand(view, command);
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
      if (shouldReinsertLiteralOnCancel(rawInput)) {
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
      input.value = suggestions[Math.max(selectedIndex, 0)] ?? input.value;
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
      if (isInsideCommandPicker(event.target)) return false;
      if (event.key !== ":" || event.ctrlKey || event.altKey || event.metaKey) return false;
      if (options.vimMode && view.dom.dataset.vimMode !== "insert") return false;
      event.preventDefault();
      openCommandPicker(view, options);
      return true;
    },
  });
}
