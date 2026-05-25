import { type Extension } from "@codemirror/state";
import {
  EditorView,
  ViewPlugin,
  type PluginValue,
  type ViewUpdate,
} from "@codemirror/view";
import {
  convertLineToList,
  convertLineToTitle,
  type ListKind,
} from "./wasm.ts";

type InlineMarker = "*" | "**" | "~~" | "`";
type InsertBlockKind =
  | "heading"
  | "bullet"
  | "numbered"
  | "checklist"
  | "quote"
  | "code"
  | "table";

type InsertMenuKeyEvent = Pick<
  KeyboardEvent,
  "altKey" | "code" | "ctrlKey" | "key" | "metaKey" | "shiftKey"
>;

interface BlockMenuItem {
  label: string;
  icon: string;
  kind: InsertBlockKind;
}

const INLINE_MARKERS: readonly InlineMarker[] = ["**", "~~", "`", "*"];
const BLOCK_MENU_ITEMS: readonly BlockMenuItem[] = [
  { label: "Heading", icon: "H1", kind: "heading" },
  { label: "Bullet list", icon: "-", kind: "bullet" },
  { label: "Numbered list", icon: "1.", kind: "numbered" },
  { label: "Checklist", icon: "[]", kind: "checklist" },
  { label: "Quote", icon: ">", kind: "quote" },
  { label: "Code block", icon: "{}", kind: "code" },
  { label: "Table", icon: "| |", kind: "table" },
];
const FLOATING_MARGIN = 8;

function clamp(value: number, min: number, max: number): number {
  if (max < min) return min;
  return Math.max(min, Math.min(value, max));
}

function leadingIndent(lineText: string): string {
  return lineText.match(/^[\t ]*/)?.[0] ?? "";
}

function stripBlockPrefix(lineText: string): string {
  let next = lineText.trim();
  next = next.replace(/^#{1,6}\s*/, "");
  next = next.replace(/^(?:[-+*]|\d+[.)])\s+/, "");
  next = next.replace(/^\[[ xX]\]\s+/, "");
  next = next.replace(/^#{1,6}\s*/, "");
  return next.trim();
}

export function clearInlineFormattingText(text: string): string {
  let next = text;
  for (const marker of INLINE_MARKERS) {
    next = next.split(marker).join("");
  }
  return next;
}

export function fallbackTitleLineText(lineText: string): string {
  const indent = leadingIndent(lineText);
  const content = stripBlockPrefix(lineText);
  return content.length === 0 ? `${indent}# ` : `${indent}# ${content}`;
}

export function fallbackListLineText(
  lineText: string,
  kind: ListKind,
  orderedIndex = 1,
): string {
  const indent = leadingIndent(lineText);
  const content = stripBlockPrefix(lineText);
  const prefix =
    kind === "ordered"
      ? `${Math.max(1, orderedIndex)}. `
      : kind === "checklist"
        ? "- [ ] "
        : "- ";
  return `${indent}${prefix}${content}`;
}

export function toggleQuoteLineText(lineText: string): string {
  const unquoted = lineText.replace(/^(\s*)>\s?/, "$1");
  if (unquoted !== lineText) return unquoted;
  const indentLength = lineText.length - lineText.trimStart().length;
  return `${lineText.slice(0, indentLength)}> ${lineText.slice(indentLength)}`;
}

export function snippetForInsertBlock(
  kind: Extract<InsertBlockKind, "code" | "table">,
): { text: string; cursorOffset: number } {
  if (kind === "code") {
    return { text: "```\n\n```", cursorOffset: 4 };
  }
  const text = "| Column 1 | Column 2 |\n| --- | --- |\n|  |  |";
  const cursorOffset = text.indexOf("|  |") + 2;
  return { text, cursorOffset };
}

export function isInsertMenuShortcut(event: InsertMenuKeyEvent): boolean {
  const hasPrimaryModifier = event.ctrlKey || event.metaKey;
  if (!hasPrimaryModifier || event.altKey || event.shiftKey) return false;
  return event.key === "/" || event.code === "Slash";
}

export function nextBlockMenuIndex(
  current: number,
  delta: number,
  itemCount = BLOCK_MENU_ITEMS.length,
): number {
  if (itemCount <= 0) return 0;
  const wrapped = (current + delta) % itemCount;
  return wrapped < 0 ? wrapped + itemCount : wrapped;
}

function titleLineText(lineText: string): string {
  const converted = convertLineToTitle(lineText);
  if (converted.changed) return converted.text;
  return fallbackTitleLineText(lineText);
}

function listLineText(
  lineText: string,
  kind: ListKind,
  orderedIndex = 1,
): string {
  const converted = convertLineToList(lineText, kind, orderedIndex);
  if (converted.changed) return converted.text;
  return fallbackListLineText(lineText, kind, orderedIndex);
}

class SelectionToolbarController implements PluginValue {
  private readonly toolbarEl: HTMLDivElement;
  private readonly blockButtonEl: HTMLButtonElement;
  private readonly blockMenuEl: HTMLDivElement;
  private readonly blockMenuButtons: HTMLButtonElement[] = [];
  private blockMenuOpen = false;
  private activeBlockMenuIndex = 0;
  private layoutFrame: number | null = null;

  private readonly onDocumentPointerDown = (event: PointerEvent) => {
    const target = event.target as Node | null;
    if (!target) return;
    if (
      this.toolbarEl.contains(target) ||
      this.blockButtonEl.contains(target) ||
      this.blockMenuEl.contains(target)
    ) {
      return;
    }
    if (this.view.dom.contains(target)) {
      this.closeBlockMenu();
      return;
    }
    this.hideAll();
  };

  private readonly onWindowResize = () => this.scheduleLayout();
  private readonly onWindowBlur = () => this.hideAll();
  private readonly onWindowKeydown = (event: KeyboardEvent) => {
    if (this.handleOpenBlockMenuKeydown(event)) return;
    if (isInsertMenuShortcut(event) && this.openBlockMenuFromKeyboard()) {
      event.preventDefault();
      event.stopPropagation();
      return;
    }
    if (event.key !== "Escape") return;
    if (this.toolbarEl.hidden && !this.blockMenuOpen && this.blockButtonEl.hidden) return;
    event.preventDefault();
    this.hideAll();
    this.view.focus();
  };

  private readonly onEditorScroll = () => {
    this.closeBlockMenu();
    this.scheduleLayout();
  };

  constructor(private readonly view: EditorView) {
    this.toolbarEl = this.createToolbarElement();
    this.blockButtonEl = this.createBlockButtonElement();
    this.blockMenuEl = this.createBlockMenuElement();

    document.body.append(this.toolbarEl, this.blockButtonEl, this.blockMenuEl);
    document.addEventListener("pointerdown", this.onDocumentPointerDown, true);
    window.addEventListener("resize", this.onWindowResize);
    window.addEventListener("blur", this.onWindowBlur);
    window.addEventListener("keydown", this.onWindowKeydown, true);
    this.view.scrollDOM.addEventListener("scroll", this.onEditorScroll, {
      passive: true,
    });
    this.scheduleLayout();
  }

  update(update: ViewUpdate): void {
    if (
      update.selectionSet ||
      update.viewportChanged ||
      update.heightChanged ||
      update.geometryChanged ||
      update.focusChanged
    ) {
      this.scheduleLayout();
    }
  }

  destroy(): void {
    if (this.layoutFrame !== null) {
      cancelAnimationFrame(this.layoutFrame);
      this.layoutFrame = null;
    }
    document.removeEventListener("pointerdown", this.onDocumentPointerDown, true);
    window.removeEventListener("resize", this.onWindowResize);
    window.removeEventListener("blur", this.onWindowBlur);
    window.removeEventListener("keydown", this.onWindowKeydown, true);
    this.view.scrollDOM.removeEventListener("scroll", this.onEditorScroll);
    this.toolbarEl.remove();
    this.blockButtonEl.remove();
    this.blockMenuEl.remove();
  }

  private scheduleLayout() {
    if (this.layoutFrame !== null) return;
    this.layoutFrame = requestAnimationFrame(() => {
      this.layoutFrame = null;
      this.layout();
    });
  }

  private layout() {
    if (this.isSuppressed() || this.hasCompetingOverlay()) {
      this.hideAll();
      return;
    }

    const main = this.view.state.selection.main;
    if (!main.empty) {
      this.closeBlockMenu();
      this.blockButtonEl.hidden = true;
      this.positionSelectionToolbar();
      return;
    }

    this.toolbarEl.hidden = true;
    this.toolbarEl.classList.remove("editor-selection-toolbar--open");
    this.positionBlockButton();
    if (this.blockMenuOpen) {
      this.positionBlockMenu();
    }
  }

  private isSuppressed(): boolean {
    if (!this.view.hasFocus) return true;
    return !!this.view.dom.dataset.vimMode;
  }

  private hasCompetingOverlay(): boolean {
    return !!document.querySelector(
      ".command-picker-bar, .editor-context-menu--open, .cm-search, .cm-tooltip-autocomplete, .variable-autocomplete",
    );
  }

  private hideAll() {
    this.toolbarEl.hidden = true;
    this.toolbarEl.classList.remove("editor-selection-toolbar--open");
    this.blockButtonEl.hidden = true;
    this.closeBlockMenu();
  }

  private positionSelectionToolbar() {
    const main = this.view.state.selection.main;
    const start = this.view.coordsAtPos(main.from);
    const end = this.view.coordsAtPos(main.to);
    const head = this.view.coordsAtPos(main.head);
    const first = start ?? head ?? end;
    const last = end ?? head ?? start;
    if (!first || !last) {
      this.toolbarEl.hidden = true;
      return;
    }

    this.toolbarEl.hidden = false;
    this.toolbarEl.classList.add("editor-selection-toolbar--open");
    this.toolbarEl.style.left = "0px";
    this.toolbarEl.style.top = "0px";

    const rect = this.toolbarEl.getBoundingClientRect();
    const centerX =
      (Math.min(first.left, last.left) + Math.max(first.right, last.right)) / 2;
    const belowTop = Math.max(first.bottom, last.bottom) + FLOATING_MARGIN;
    const preferredTop = Math.min(first.top, last.top) - rect.height - FLOATING_MARGIN;
    const top =
      preferredTop >= FLOATING_MARGIN
        ? preferredTop
        : Math.min(belowTop, window.innerHeight - rect.height - FLOATING_MARGIN);
    const left = clamp(
      centerX - rect.width / 2,
      FLOATING_MARGIN,
      window.innerWidth - rect.width - FLOATING_MARGIN,
    );

    this.toolbarEl.style.left = `${left}px`;
    this.toolbarEl.style.top = `${clamp(top, FLOATING_MARGIN, window.innerHeight - rect.height - FLOATING_MARGIN)}px`;
  }

  private positionBlockButton() {
    const main = this.view.state.selection.main;
    const line = this.view.state.doc.lineAt(main.head);
    const coords = this.view.coordsAtPos(line.from) ?? this.view.coordsAtPos(main.head);
    if (!coords) {
      this.blockButtonEl.hidden = true;
      this.closeBlockMenu();
      return;
    }

    this.blockButtonEl.hidden = false;
    this.blockButtonEl.classList.add("editor-block-affordance--visible");
    this.blockButtonEl.style.left = "0px";
    this.blockButtonEl.style.top = "0px";

    const rect = this.blockButtonEl.getBoundingClientRect();
    const scrollerRect = this.view.scrollDOM.getBoundingClientRect();
    const left = clamp(
      coords.left - rect.width - 9,
      scrollerRect.left + 6,
      window.innerWidth - rect.width - FLOATING_MARGIN,
    );
    const top = clamp(
      coords.top + (coords.bottom - coords.top - rect.height) / 2,
      FLOATING_MARGIN,
      window.innerHeight - rect.height - FLOATING_MARGIN,
    );

    this.blockButtonEl.style.left = `${left}px`;
    this.blockButtonEl.style.top = `${top}px`;
  }

  private openBlockMenu(activeIndex = 0) {
    this.blockMenuOpen = true;
    this.blockMenuEl.hidden = false;
    this.blockMenuEl.classList.add("editor-block-menu--open");
    this.blockButtonEl.setAttribute("aria-expanded", "true");
    this.setActiveBlockMenuIndex(activeIndex);
    this.positionBlockMenu();
  }

  private closeBlockMenu() {
    this.blockMenuOpen = false;
    this.blockMenuEl.hidden = true;
    this.blockMenuEl.classList.remove("editor-block-menu--open");
    this.blockButtonEl.setAttribute("aria-expanded", "false");
    this.blockMenuEl.removeAttribute("aria-activedescendant");
  }

  private positionBlockMenu() {
    if (!this.blockMenuOpen) return;
    const buttonRect = this.blockButtonEl.getBoundingClientRect();
    this.blockMenuEl.hidden = false;
    this.blockMenuEl.style.left = "0px";
    this.blockMenuEl.style.top = "0px";

    const rect = this.blockMenuEl.getBoundingClientRect();
    let left = buttonRect.right + 7;
    if (left + rect.width > window.innerWidth - FLOATING_MARGIN) {
      left = buttonRect.left - rect.width - 7;
    }
    const top = clamp(
      buttonRect.top - 2,
      FLOATING_MARGIN,
      window.innerHeight - rect.height - FLOATING_MARGIN,
    );

    this.blockMenuEl.style.left = `${clamp(left, FLOATING_MARGIN, window.innerWidth - rect.width - FLOATING_MARGIN)}px`;
    this.blockMenuEl.style.top = `${top}px`;
  }

  private createToolbarElement(): HTMLDivElement {
    const toolbar = document.createElement("div");
    toolbar.className = "editor-selection-toolbar";
    toolbar.hidden = true;
    toolbar.setAttribute("role", "toolbar");
    toolbar.setAttribute("aria-label", "Selection formatting");

    toolbar.append(
      this.createToolbarButton("B", "Bold", () => this.toggleWrap("**"), "bold"),
      this.createToolbarButton("I", "Italic", () => this.toggleWrap("*"), "italic"),
      this.createToolbarButton("S", "Strikethrough", () => this.toggleWrap("~~"), "strike"),
      this.createToolbarButton("{}", "Inline code", () => this.toggleWrap("`"), "code"),
      this.createToolbarButton("Tx", "Clear formatting", () => this.clearFormatting(), "clear"),
    );
    return toolbar;
  }

  private createBlockButtonElement(): HTMLButtonElement {
    const button = document.createElement("button");
    button.type = "button";
    button.className = "editor-block-affordance";
    button.hidden = true;
    button.textContent = "+";
    button.title = "Insert block (Ctrl+/)";
    button.setAttribute("aria-label", "Insert block");
    button.setAttribute("aria-haspopup", "menu");
    button.setAttribute("aria-expanded", "false");
    button.addEventListener("pointerdown", this.preventEditorBlur);
    button.addEventListener("click", (event) => {
      event.preventDefault();
      if (this.blockMenuOpen) {
        this.closeBlockMenu();
      } else {
        this.openBlockMenu();
      }
    });
    return button;
  }

  private createBlockMenuElement(): HTMLDivElement {
    const menu = document.createElement("div");
    menu.className = "editor-block-menu";
    menu.hidden = true;
    menu.setAttribute("role", "menu");
    menu.setAttribute("aria-label", "Insert block");
    menu.addEventListener("pointerdown", this.preventEditorBlur);

    BLOCK_MENU_ITEMS.forEach((item, index) => {
      const button = this.createBlockMenuButton(item.label, item.icon, item.kind, index);
      this.blockMenuButtons.push(button);
      menu.appendChild(button);
    });
    return menu;
  }

  private openBlockMenuFromKeyboard(): boolean {
    if (this.isSuppressed() || this.hasCompetingOverlay()) return false;
    const main = this.view.state.selection.main;
    if (!main.empty) return false;
    this.toolbarEl.hidden = true;
    this.positionBlockButton();
    if (this.blockButtonEl.hidden) return false;
    this.openBlockMenu(this.activeBlockMenuIndex);
    return true;
  }

  private handleOpenBlockMenuKeydown(event: KeyboardEvent): boolean {
    if (!this.blockMenuOpen) return false;

    switch (event.key) {
      case "ArrowDown":
      case "Down":
        event.preventDefault();
        event.stopPropagation();
        this.setActiveBlockMenuIndex(nextBlockMenuIndex(this.activeBlockMenuIndex, 1));
        return true;
      case "ArrowUp":
      case "Up":
        event.preventDefault();
        event.stopPropagation();
        this.setActiveBlockMenuIndex(nextBlockMenuIndex(this.activeBlockMenuIndex, -1));
        return true;
      case "Home":
        event.preventDefault();
        event.stopPropagation();
        this.setActiveBlockMenuIndex(0);
        return true;
      case "End":
        event.preventDefault();
        event.stopPropagation();
        this.setActiveBlockMenuIndex(BLOCK_MENU_ITEMS.length - 1);
        return true;
      case "Enter":
      case " ":
        event.preventDefault();
        event.stopPropagation();
        this.runAction(() => this.insertBlock(BLOCK_MENU_ITEMS[this.activeBlockMenuIndex].kind));
        return true;
      case "Escape":
        event.preventDefault();
        event.stopPropagation();
        this.closeBlockMenu();
        this.view.focus();
        return true;
      case "Tab":
        this.closeBlockMenu();
        return false;
      default:
        if (event.key.length === 1 || event.key === "Backspace" || event.key === "Delete") {
          this.closeBlockMenu();
        }
        return false;
    }
  }

  private setActiveBlockMenuIndex(index: number) {
    const count = this.blockMenuButtons.length;
    if (count === 0) return;
    this.activeBlockMenuIndex = ((index % count) + count) % count;
    this.blockMenuButtons.forEach((button, buttonIndex) => {
      const active = buttonIndex === this.activeBlockMenuIndex;
      button.classList.toggle("editor-block-menu-item--active", active);
      if (active) {
        this.blockMenuEl.setAttribute("aria-activedescendant", button.id);
      }
    });
  }

  private createToolbarButton(
    label: string,
    title: string,
    action: () => void,
    style: string,
  ): HTMLButtonElement {
    const button = document.createElement("button");
    button.type = "button";
    button.className = `editor-selection-toolbar-btn editor-selection-toolbar-btn--${style}`;
    button.textContent = label;
    button.title = title;
    button.setAttribute("aria-label", title);
    button.addEventListener("pointerdown", this.preventEditorBlur);
    button.addEventListener("click", (event) => {
      event.preventDefault();
      this.runAction(action);
    });
    return button;
  }

  private createBlockMenuButton(
    label: string,
    icon: string,
    kind: InsertBlockKind,
    index: number,
  ): HTMLButtonElement {
    const button = document.createElement("button");
    button.type = "button";
    button.className = "editor-block-menu-item";
    button.id = `editor-block-menu-item-${index}`;
    button.setAttribute("role", "menuitem");
    const iconEl = document.createElement("span");
    iconEl.className = "editor-block-menu-icon";
    iconEl.textContent = icon;
    const labelEl = document.createElement("span");
    labelEl.className = "editor-block-menu-label";
    labelEl.textContent = label;
    button.append(iconEl, labelEl);
    button.addEventListener("pointerdown", this.preventEditorBlur);
    button.addEventListener("mouseenter", () => this.setActiveBlockMenuIndex(index));
    button.addEventListener("focus", () => this.setActiveBlockMenuIndex(index));
    button.addEventListener("click", (event) => {
      event.preventDefault();
      this.runAction(() => this.insertBlock(kind));
    });
    return button;
  }

  private readonly preventEditorBlur = (event: PointerEvent) => {
    event.preventDefault();
    event.stopPropagation();
  };

  private runAction(action: () => void) {
    try {
      action();
    } catch (error) {
      console.error("Selection toolbar action failed:", error);
    } finally {
      this.view.focus();
      this.scheduleLayout();
    }
  }

  private toggleWrap(left: InlineMarker, right = left) {
    const main = this.view.state.selection.main;
    const from = main.from;
    const to = main.to;

    if (main.empty) {
      const insert = left + right;
      this.view.dispatch({
        changes: { from, to, insert },
        selection: { anchor: from + left.length },
        scrollIntoView: true,
      });
      return;
    }

    const before =
      from >= left.length
        ? this.view.state.sliceDoc(from - left.length, from)
        : "";
    const after =
      to + right.length <= this.view.state.doc.length
        ? this.view.state.sliceDoc(to, to + right.length)
        : "";
    const isWrapped = before === left && after === right;

    if (isWrapped) {
      this.view.dispatch({
        changes: [
          { from: to, to: to + right.length, insert: "" },
          { from: from - left.length, to: from, insert: "" },
        ],
        selection: { anchor: from - left.length, head: to - left.length },
        scrollIntoView: true,
      });
      return;
    }

    this.view.dispatch({
      changes: [
        { from, to: from, insert: left },
        { from: to, to, insert: right },
      ],
      selection: { anchor: from + left.length, head: to + left.length },
      scrollIntoView: true,
    });
  }

  private clearFormatting() {
    const main = this.view.state.selection.main;
    if (main.empty) return;
    const text = this.view.state.sliceDoc(main.from, main.to);
    const clean = clearInlineFormattingText(text);
    this.view.dispatch({
      changes: { from: main.from, to: main.to, insert: clean },
      selection: { anchor: main.from, head: main.from + clean.length },
      scrollIntoView: true,
    });
  }

  private insertBlock(kind: InsertBlockKind) {
    this.closeBlockMenu();
    if (kind === "heading") {
      this.replaceCurrentLine(titleLineText(this.currentLineText()));
      return;
    }
    if (kind === "bullet") {
      this.replaceCurrentLine(listLineText(this.currentLineText(), "unordered", 1));
      return;
    }
    if (kind === "numbered") {
      this.replaceCurrentLine(listLineText(this.currentLineText(), "ordered", 1));
      return;
    }
    if (kind === "checklist") {
      this.replaceCurrentLine(listLineText(this.currentLineText(), "checklist", 1));
      return;
    }
    if (kind === "quote") {
      this.replaceCurrentLine(toggleQuoteLineText(this.currentLineText()));
      return;
    }

    this.insertSnippet(kind);
  }

  private currentLineText(): string {
    const main = this.view.state.selection.main;
    return this.view.state.doc.lineAt(main.head).text;
  }

  private replaceCurrentLine(text: string) {
    const main = this.view.state.selection.main;
    const line = this.view.state.doc.lineAt(main.head);
    this.view.dispatch({
      changes: { from: line.from, to: line.to, insert: text },
      selection: { anchor: line.from + text.length },
      scrollIntoView: true,
    });
  }

  private insertSnippet(kind: Extract<InsertBlockKind, "code" | "table">) {
    const snippet = snippetForInsertBlock(kind);
    const main = this.view.state.selection.main;
    const line = this.view.state.doc.lineAt(main.head);
    const replaceLine = line.text.trim().length === 0;
    const insert = replaceLine ? snippet.text : `\n${snippet.text}`;
    const from = replaceLine ? line.from : line.to;
    const to = replaceLine ? line.to : line.to;
    const anchor = from + (replaceLine ? snippet.cursorOffset : 1 + snippet.cursorOffset);

    this.view.dispatch({
      changes: { from, to, insert },
      selection: { anchor },
      scrollIntoView: true,
    });
  }
}

const selectionToolbarPlugin = ViewPlugin.fromClass(SelectionToolbarController);

export function selectionToolbarExtensions(): Extension[] {
  return [selectionToolbarPlugin];
}
