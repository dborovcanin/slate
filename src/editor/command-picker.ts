import { EditorView } from "@codemirror/view";
import { executeCommand, listCommandSuggestions, type CommandMode, type CommandSuggestion } from "./command-engine";
import { CommandHistoryNavigator, rememberCommand } from "./command-history.ts";
import { insertAtSelection } from "./editor-utils";
import { createListOverlay, type ListOverlay, type ListOverlayState } from "../overlays/overlay.ts";
import type { NoteModules } from "../api.ts";

interface CommandPickerOptions {
  mode: CommandMode;
  dateFormat?: string;
  dateTimeFormat?: string;
  onWriteCommand?: (options?: { force?: boolean }) => Promise<void> | void;
  onExitCommand?: () => Promise<void> | void;
  onClipWatchStateChange?: (active: boolean) => void;
  onClipWatchPaste?: (text: string) => void;
  getNoteModules?: () => NoteModules | null;
  setNoteModules?: (modules: NoteModules) => Promise<void> | void;
  source?: "vim-colon" | "shortcut";
  onCancel?: () => void;
  selectionOverride?: {
    anchor: number;
    head: number;
  };
}

interface CommandModeExtensionOptions {
  vimMode?: boolean;
  dateFormat?: string;
  dateTimeFormat?: string;
  onWriteCommand?: (options?: { force?: boolean }) => Promise<void> | void;
  onExitCommand?: () => Promise<void> | void;
  onClipWatchStateChange?: (active: boolean) => void;
  onClipWatchPaste?: (text: string) => void;
  getNoteModules?: () => NoteModules | null;
  setNoteModules?: (modules: NoteModules) => Promise<void> | void;
}

const COMMAND_PICKER_SELECTOR = ".command-picker-bar";

interface CommandCompletionOption {
  token: string;
  hasMore: boolean;
}

interface CommandCompletionMenu {
  prefixTokens: string[];
  options: CommandCompletionOption[];
}

function sanitizeCommand(rawInput: string): string {
  return rawInput.trim().replace(/^:/, "");
}

function normalizeCommandInputForCompletion(rawInput: string): string {
  const sanitized = sanitizeCommand(rawInput).toLowerCase();
  if (sanitized.length === 0) return "";
  return sanitized.split(/\s+/).filter((token) => token.length > 0).join(" ");
}

function buildCommandCompletionMenu(
  rawInput: string,
  suggestions: CommandSuggestion[],
): CommandCompletionMenu | null {
  if (suggestions.length === 0) return null;

  const normalizedInput = normalizeCommandInputForCompletion(rawInput);
  const endsWithSpace = /\s$/.test(rawInput);
  const typedTokens = normalizedInput.length > 0 ? normalizedInput.split(" ") : [];

  const tokenPrefix = endsWithSpace
    ? ""
    : (typedTokens.length > 0 ? typedTokens[typedTokens.length - 1]! : "");
  const prefixTokens = endsWithSpace ? typedTokens : typedTokens.slice(0, -1);
  const tokenIndex = prefixTokens.length;

  const options: CommandCompletionOption[] = [];
  for (const suggestion of suggestions) {
    const suggestionTokens = suggestion.value.toLowerCase().split(/\s+/).filter((token) => token.length > 0);
    if (suggestionTokens.length <= tokenIndex) continue;
    if (!prefixTokens.every((token, idx) => suggestionTokens[idx] === token)) continue;

    const token = suggestionTokens[tokenIndex]!;
    if (!token.startsWith(tokenPrefix)) continue;

    const hasMore = suggestionTokens.length > tokenIndex + 1;
    const existing = options.find((entry) => entry.token === token);
    if (existing) {
      existing.hasMore = existing.hasMore || hasMore;
      continue;
    }
    options.push({ token, hasMore });
  }

  if (options.length === 0) return null;
  return { prefixTokens, options };
}

function applySingleWordCompletion(state: ListOverlayState<CommandSuggestion>): boolean {
  const completion = buildCommandCompletionMenu(state.query, state.items);
  if (!completion || completion.options.length !== 1) return false;

  const option = completion.options[0]!;
  const nextTokens = [...completion.prefixTokens, option.token];
  state.inputEl.value = nextTokens.join(" ");
  if (option.hasMore) {
    state.inputEl.value += " ";
  }
  state.selectedIndex = 0;
  state.refresh();
  return true;
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
  let didSubmit = false;
  const historyNavigator = new CommandHistoryNavigator();

  const submit = async (command: string) => {
    didSubmit = true;
    pickerOverlay?.close();
    view.focus();
    if (!command) return;
    rememberCommand(command);
    try {
      const message = await executeCommand(view, command, {
        mode: options.mode,
        dateFormat: options.dateFormat,
        dateTimeFormat: options.dateTimeFormat,
        onWriteCommand: options.onWriteCommand,
        onExitCommand: options.onExitCommand,
        onClipWatchStateChange: options.onClipWatchStateChange,
        onClipWatchPaste: options.onClipWatchPaste,
        getNoteModules: options.getNoteModules,
        setNoteModules: options.setNoteModules,
        selectionOverride: options.selectionOverride,
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
    const command = sanitizeCommand(state.query);
    const normalized = command.toLowerCase();
    if (!command && state.items.length > 0) {
      return state.items[Math.max(state.selectedIndex, 0)]?.value ?? "";
    }
    if (!command) return "";
    if (state.items.some((s) => s.value === normalized)) return normalized;
    const selected = state.items[Math.max(state.selectedIndex, 0)]?.value;
    if (selected && selected.toLowerCase().startsWith(normalized)) {
      return selected;
    }
    return command;
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
      if (!didSubmit) {
        options.onCancel?.();
      }
      if (options.source === "vim-colon" && shouldReinsertLiteralOnCancel(query)) {
        insertAtSelection(view, `:${query}`);
      }
    },
    emptyMessage: "No commands",
    onKeydown: (event, state) => {
      if (event.key !== "ArrowUp" && event.key !== "ArrowDown") {
        historyNavigator.reset();
      }

      if (event.key === "ArrowUp") {
        event.preventDefault();
        event.stopPropagation();
        const previous = historyNavigator.previous();
        if (!previous) return true;
        state.inputEl.value = previous;
        state.selectedIndex = 0;
        state.refresh();
        return true;
      }

      if (event.key === "ArrowDown") {
        event.preventDefault();
        event.stopPropagation();
        const next = historyNavigator.next();
        if (!next) return true;
        state.inputEl.value = next;
        state.selectedIndex = 0;
        state.refresh();
        return true;
      }

      if (event.key === "ArrowLeft" || event.key === "ArrowRight") {
        const menu = buildCommandCompletionMenu(state.query, state.items);
        if (!menu || menu.options.length === 0) {
          return false;
        }
        event.preventDefault();
        event.stopPropagation();
        const len = state.items.length;
        const current = Math.max(0, Math.min(state.selectedIndex, len - 1));
        const delta = event.key === "ArrowLeft" ? -1 : 1;
        state.selectedIndex = (current + delta + len) % len;
        state.refresh();
        return true;
      }

      if (event.key === "Tab") {
        if (state.items.length === 0) return false;
        event.preventDefault();
        event.stopPropagation();
        if (applySingleWordCompletion(state)) {
          return true;
        }
        const len = state.items.length;
        const current = Math.max(0, Math.min(state.selectedIndex, len - 1));
        const delta = event.shiftKey ? -1 : 1;
        state.selectedIndex = (current + delta + len) % len;
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
        onWriteCommand: options.onWriteCommand,
        onExitCommand: options.onExitCommand,
        onClipWatchStateChange: options.onClipWatchStateChange,
        onClipWatchPaste: options.onClipWatchPaste,
        getNoteModules: options.getNoteModules,
        setNoteModules: options.setNoteModules,
        source: "shortcut",
      });
      return true;
    },
  });
}
