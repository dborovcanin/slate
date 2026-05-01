import { Prec, Transaction, type Extension } from "@codemirror/state";
import {
  EditorView,
  ViewPlugin,
  type PluginValue,
  type ViewUpdate,
} from "@codemirror/view";
import { readSystemClipboardText } from "../api";
import { executeCommand } from "./command-engine";

interface EditorContextMenuOptions {
  dateFormat?: string;
  dateTimeFormat?: string;
}

type ContextMenuIcon =
  | "cut"
  | "copy"
  | "paste"
  | "select-all"
  | "format"
  | "paragraph"
  | "bold"
  | "italic"
  | "strikethrough"
  | "code"
  | "quote"
  | "bullet-list"
  | "numbered-list"
  | "checklist";

class EditorContextMenuController implements PluginValue {
  private menuEl: HTMLDivElement;
  private open = false;

  private readonly onDocumentPointerDown = (event: PointerEvent) => {
    if (!this.menuEl.contains(event.target as Node)) {
      this.close();
    }
  };

  private readonly onWindowResize = () => this.close();
  private readonly onWindowBlur = () => this.close();
  private readonly onWindowKeydown = (event: KeyboardEvent) => {
    if (event.key === "Escape") {
      event.preventDefault();
      this.close();
      this.view.focus();
    }
  };

  private readonly onEditorScroll = () => this.close();

  constructor(
    private readonly view: EditorView,
    private readonly options: EditorContextMenuOptions,
  ) {
    this.menuEl = this.createMenuElement();
    document.body.appendChild(this.menuEl);
    this.view.scrollDOM.addEventListener("scroll", this.onEditorScroll, {
      passive: true,
    });
  }

  update(_update: ViewUpdate): void {}

  destroy(): void {
    this.close();
    this.view.scrollDOM.removeEventListener("scroll", this.onEditorScroll);
    this.menuEl.remove();
  }

  shouldHandleContextMenu(target: EventTarget | null): boolean {
    if (!(target instanceof Element)) return false;
    if (target.closest(".editor-context-menu")) return false;
    if (target.closest(".editor-search-bar, .cm-search")) return false;
    if (target.closest(".command-picker-bar")) return false;
    if (target.closest(".variable-autocomplete, .cm-tooltip-autocomplete")) return false;
    return target.closest(".cm-scroller, .cm-content, .cm-line") !== null;
  }

  openFromEvent(event: MouseEvent): boolean {
    const position = this.view.posAtCoords({
      x: event.clientX,
      y: event.clientY,
    });

    if (position !== null) {
      this.alignSelectionForContext(position);
    }

    event.preventDefault();
    event.stopPropagation();

    this.openAt(event.clientX, event.clientY);
    return true;
  }

  private alignSelectionForContext(position: number) {
    const main = this.view.state.selection.main;
    const insideSelection =
      !main.empty && position >= main.from && position <= main.to;
    if (insideSelection) return;

    this.view.dispatch({
      selection: { anchor: position },
      annotations: Transaction.addToHistory.of(false),
    });
  }

  private openAt(clientX: number, clientY: number) {
    this.close();

    this.menuEl.hidden = false;
    this.menuEl.classList.add("editor-context-menu--open");
    this.menuEl.style.left = "0px";
    this.menuEl.style.top = "0px";

    const margin = 8;
    const rect = this.menuEl.getBoundingClientRect();
    const maxLeft = window.innerWidth - rect.width - margin;
    const maxTop = window.innerHeight - rect.height - margin;
    const left = Math.max(margin, Math.min(clientX, maxLeft));
    const top = Math.max(margin, Math.min(clientY, maxTop));
    this.menuEl.style.left = `${left}px`;
    this.menuEl.style.top = `${top}px`;

    this.adjustSubmenuSide();
    this.open = true;

    document.addEventListener("pointerdown", this.onDocumentPointerDown, true);
    window.addEventListener("resize", this.onWindowResize);
    window.addEventListener("blur", this.onWindowBlur);
    window.addEventListener("keydown", this.onWindowKeydown, true);
  }

  private close() {
    if (!this.open) return;
    this.open = false;
    this.menuEl.classList.remove("editor-context-menu--open");
    this.menuEl.hidden = true;
    this.menuEl.classList.remove("editor-context-menu--submenu-left");

    document.removeEventListener(
      "pointerdown",
      this.onDocumentPointerDown,
      true,
    );
    window.removeEventListener("resize", this.onWindowResize);
    window.removeEventListener("blur", this.onWindowBlur);
    window.removeEventListener("keydown", this.onWindowKeydown, true);
  }

  private adjustSubmenuSide() {
    this.menuEl.classList.remove("editor-context-menu--submenu-left");
    const menuRect = this.menuEl.getBoundingClientRect();
    const submenu = this.menuEl.querySelector(
      ".editor-context-submenu",
    ) as HTMLElement | null;
    if (!submenu) return;
    const submenuWidth = submenu.offsetWidth || 210;
    if (menuRect.right + submenuWidth > window.innerWidth - 8) {
      this.menuEl.classList.add("editor-context-menu--submenu-left");
    }
  }

  private createActionButton(
    label: string,
    icon: ContextMenuIcon,
    action: () => void | Promise<void>,
  ): HTMLButtonElement {
    const button = document.createElement("button");
    button.type = "button";
    button.className = "editor-context-menu-item";
    const iconEl = this.createIcon(icon);
    const labelEl = document.createElement("span");
    labelEl.className = "editor-context-menu-label";
    labelEl.textContent = label;
    button.append(iconEl, labelEl);
    button.addEventListener("click", () => {
      void this.runAction(action);
    });
    return button;
  }

  private createSubmenuGroup(
    label: string,
    icon: ContextMenuIcon,
    items: ReadonlyArray<{ label: string; icon: ContextMenuIcon; action: () => void | Promise<void> }>,
  ): HTMLDivElement {
    const group = document.createElement("div");
    group.className = "editor-context-menu-group";

    const trigger = document.createElement("button");
    trigger.type = "button";
    trigger.className =
      "editor-context-menu-item editor-context-menu-item--submenu-trigger";
    const left = document.createElement("span");
    left.className = "editor-context-menu-left";
    const triggerIcon = this.createIcon(icon);
    const triggerLabel = document.createElement("span");
    triggerLabel.className = "editor-context-menu-label";
    triggerLabel.textContent = label;
    left.append(triggerIcon, triggerLabel);
    const caret = document.createElement("span");
    caret.className = "editor-context-menu-caret";
    caret.textContent = "▸";
    trigger.append(left, caret);

    const submenu = document.createElement("div");
    submenu.className = "editor-context-submenu";
    submenu.setAttribute("role", "menu");
    for (const item of items) {
      submenu.appendChild(this.createActionButton(item.label, item.icon, item.action));
    }

    group.appendChild(trigger);
    group.appendChild(submenu);
    return group;
  }

  private createMenuElement(): HTMLDivElement {
    const menu = document.createElement("div");
    menu.className = "editor-context-menu";
    menu.hidden = true;
    menu.setAttribute("role", "menu");
    menu.addEventListener("contextmenu", (event) => event.preventDefault());

    menu.appendChild(this.createActionButton("Cut", "cut", () => this.cutSelection()));
    menu.appendChild(this.createActionButton("Copy", "copy", () => this.copySelection()));
    menu.appendChild(this.createActionButton("Paste", "paste", () => this.pasteSelection()));
    menu.appendChild(this.createActionButton("Select all", "select-all", () => this.selectAll()));
    menu.appendChild(this.createSubmenuGroup("Format", "format", [
      { label: "Bold", icon: "bold", action: () => this.toggleWrap("**") },
      { label: "Italic", icon: "italic", action: () => this.toggleWrap("*") },
      { label: "Strikethrough", icon: "strikethrough", action: () => this.toggleWrap("~~") },
      { label: "Code", icon: "code", action: () => this.toggleWrap("`") },
      { label: "Quote", icon: "quote", action: () => this.toggleQuotePrefix() },
    ]));
    menu.appendChild(this.createSubmenuGroup("Paragraph", "paragraph", [
      { label: "Bullet list", icon: "bullet-list", action: () => this.runCommand("ulist") },
      { label: "Numbered list", icon: "numbered-list", action: () => this.runCommand("olist") },
      { label: "Checklist", icon: "checklist", action: () => this.runCommand("clist") },
    ]));
    return menu;
  }

  private createIcon(icon: ContextMenuIcon): HTMLSpanElement {
    const span = document.createElement("span");
    span.className = "editor-context-menu-icon";
    const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
    svg.setAttribute("viewBox", "0 0 24 24");
    svg.setAttribute("aria-hidden", "true");
    svg.classList.add("editor-context-menu-icon-svg");

    const addPath = (d: string) => {
      const path = document.createElementNS("http://www.w3.org/2000/svg", "path");
      path.setAttribute("d", d);
      svg.appendChild(path);
    };
    const addLine = (x1: number, y1: number, x2: number, y2: number) => {
      const line = document.createElementNS("http://www.w3.org/2000/svg", "line");
      line.setAttribute("x1", String(x1));
      line.setAttribute("y1", String(y1));
      line.setAttribute("x2", String(x2));
      line.setAttribute("y2", String(y2));
      svg.appendChild(line);
    };
    const addRect = (x: number, y: number, width: number, height: number, rx = 0) => {
      const rect = document.createElementNS("http://www.w3.org/2000/svg", "rect");
      rect.setAttribute("x", String(x));
      rect.setAttribute("y", String(y));
      rect.setAttribute("width", String(width));
      rect.setAttribute("height", String(height));
      if (rx > 0) rect.setAttribute("rx", String(rx));
      svg.appendChild(rect);
    };
    const addCircle = (cx: number, cy: number, r: number, filled = false) => {
      const circle = document.createElementNS("http://www.w3.org/2000/svg", "circle");
      circle.setAttribute("cx", String(cx));
      circle.setAttribute("cy", String(cy));
      circle.setAttribute("r", String(r));
      if (filled) {
        circle.setAttribute("fill", "currentColor");
        circle.setAttribute("stroke", "none");
      }
      svg.appendChild(circle);
    };

    switch (icon) {
      case "cut":
        addCircle(6, 7, 2.2);
        addCircle(6, 17, 2.2);
        addLine(8, 8.5, 18, 3.5);
        addLine(8, 15.5, 18, 20.5);
        break;
      case "copy":
        addRect(8, 8, 11, 11, 2);
        addPath("M5 15V6a2 2 0 0 1 2-2h9");
        break;
      case "paste":
        addRect(7, 5, 10, 15, 2);
        addRect(9, 3, 6, 4, 1.5);
        break;
      case "select-all":
        addRect(4.5, 4.5, 15, 15, 2);
        addPath("M9 12h6M12 9v6");
        break;
      case "format":
        addPath("M6 6h12");
        addPath("M10 6v12");
        addPath("M14 12H6");
        break;
      case "paragraph":
        addPath("M8 5h6a4 4 0 0 1 0 8H8z");
        addPath("M12 5v14");
        addPath("M16 5v14");
        break;
      case "bold":
        addPath("M8 6h5a3 3 0 0 1 0 6H8z");
        addPath("M8 12h6a3 3 0 0 1 0 6H8z");
        break;
      case "italic":
        addPath("M10 5h7");
        addPath("M7 19h7");
        addPath("M14 5l-4 14");
        break;
      case "strikethrough":
        addPath("M6 12h12");
        addPath("M9 7.5a3 3 0 0 1 3-2.5c1.8 0 3 1 3 2.8");
        addPath("M15 16.5a3 3 0 0 1-3 2.5c-1.8 0-3-1-3-2.8");
        break;
      case "code":
        addPath("M9 8l-4 4 4 4");
        addPath("M15 8l4 4-4 4");
        break;
      case "quote":
        addPath("M8 10h3v6H7v-4a4 4 0 0 1 4-4");
        addPath("M16 10h3v6h-4v-4a4 4 0 0 1 4-4");
        break;
      case "bullet-list":
        addCircle(6, 7, 1.4, true);
        addCircle(6, 12, 1.4, true);
        addCircle(6, 17, 1.4, true);
        addLine(10, 7, 19, 7);
        addLine(10, 12, 19, 12);
        addLine(10, 17, 19, 17);
        break;
      case "numbered-list":
        addPath("M4.5 7h2v3");
        addPath("M4.5 10h2");
        addPath("M4.5 14h2l-2 3h2");
        addLine(10, 7, 19, 7);
        addLine(10, 12, 19, 12);
        addLine(10, 17, 19, 17);
        break;
      case "checklist":
        addRect(4.5, 5.5, 3.5, 3.5, 0.6);
        addRect(4.5, 10.5, 3.5, 3.5, 0.6);
        addRect(4.5, 15.5, 3.5, 3.5, 0.6);
        addPath("M5.5 17.2l1.3 1.3 2.1-2.3");
        addLine(10, 7.2, 19, 7.2);
        addLine(10, 12.2, 19, 12.2);
        addLine(10, 17.2, 19, 17.2);
        break;
    }

    span.appendChild(svg);
    return span;
  }

  private async runAction(action: () => void | Promise<void>) {
    this.close();
    try {
      await action();
    } catch (error) {
      console.error("Context menu action failed:", error);
    } finally {
      this.view.focus();
    }
  }

  private async copySelection() {
    this.view.focus();
    if (this.execClipboardCommand("copy")) return;
    const main = this.view.state.selection.main;
    if (main.empty) return;
    const text = this.view.state.sliceDoc(main.from, main.to);
    await this.writeClipboardText(text);
  }

  private async cutSelection() {
    this.view.focus();
    if (this.execClipboardCommand("cut")) return;
    const main = this.view.state.selection.main;
    if (main.empty) return;
    const text = this.view.state.sliceDoc(main.from, main.to);
    await this.writeClipboardText(text);
    this.view.dispatch({
      changes: { from: main.from, to: main.to, insert: "" },
      selection: { anchor: main.from },
      scrollIntoView: true,
    });
  }

  private async pasteSelection() {
    this.view.focus();
    if (this.execClipboardCommand("paste")) return;
    const pasted = await this.readClipboardText();
    if (!pasted) return;
    const main = this.view.state.selection.main;
    this.view.dispatch({
      changes: { from: main.from, to: main.to, insert: pasted },
      selection: { anchor: main.from + pasted.length },
      scrollIntoView: true,
    });
  }

  private selectAll() {
    this.view.dispatch({
      selection: { anchor: 0, head: this.view.state.doc.length },
      scrollIntoView: true,
    });
  }

  private toggleWrap(left: string, right = left) {
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

    const before = from >= left.length ? this.view.state.sliceDoc(from - left.length, from) : "";
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

  private toggleQuotePrefix() {
    const main = this.view.state.selection.main;
    const startLine = this.view.state.doc.lineAt(main.from).number;
    const endLine = this.view.state.doc.lineAt(main.to).number;
    const lines: Array<{ from: number; to: number; text: string }> = [];
    let allQuoted = true;

    for (let lineNo = startLine; lineNo <= endLine; lineNo += 1) {
      const line = this.view.state.doc.line(lineNo);
      lines.push({ from: line.from, to: line.to, text: line.text });
      const trimmed = line.text.trimStart();
      if (!trimmed.startsWith("> ")) {
        allQuoted = false;
      }
    }

    const changes = lines
      .map(({ from, to, text }) => {
        if (allQuoted) {
          const unquoted = text.replace(/^(\s*)>\s?/, "$1");
          return { from, to, insert: unquoted };
        }
        const indent = text.length - text.trimStart().length;
        const quoted = `${text.slice(0, indent)}> ${text.slice(indent)}`;
        return { from, to, insert: quoted };
      })
      .reverse();

    this.view.dispatch({
      changes,
      scrollIntoView: true,
    });
  }

  private execClipboardCommand(command: "cut" | "copy" | "paste"): boolean {
    if (typeof document === "undefined" || typeof document.execCommand !== "function") {
      return false;
    }
    try {
      return document.execCommand(command);
    } catch {
      return false;
    }
  }

  private async writeClipboardText(text: string) {
    if (typeof navigator === "undefined" || !navigator.clipboard?.writeText) return;
    try {
      await navigator.clipboard.writeText(text);
    } catch {
      // Keep fallback silent; clipboard write may be blocked by host policies.
    }
  }

  private async readClipboardText(): Promise<string | null> {
    if (typeof navigator !== "undefined" && navigator.clipboard?.readText) {
      try {
        return await navigator.clipboard.readText();
      } catch {
        // fall through to host clipboard read.
      }
    }
    return readSystemClipboardText();
  }

  private async runCommand(command: string) {
    try {
      await executeCommand(this.view, command, {
        mode: "editor",
        dateFormat: this.options.dateFormat,
        dateTimeFormat: this.options.dateTimeFormat,
      });
    } catch (error) {
      console.error("Context menu command failed:", error);
    } finally {
      this.view.focus();
    }
  }
}

export function editorContextMenuExtensions(
  options: EditorContextMenuOptions = {},
): Extension[] {
  const plugin = ViewPlugin.fromClass(
    class extends EditorContextMenuController {
      constructor(view: EditorView) {
        super(view, options);
      }
    },
  );

  return [
    plugin,
    Prec.highest(
      EditorView.domEventHandlers({
        contextmenu: (event, view) => {
          const controller = view.plugin(plugin);
          if (!controller) return false;
          if (!controller.shouldHandleContextMenu(event.target)) return false;
          return controller.openFromEvent(event as MouseEvent);
        },
      }),
    ),
  ];
}
