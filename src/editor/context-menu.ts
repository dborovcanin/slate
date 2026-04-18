import { Prec, Transaction, type Extension } from "@codemirror/state";
import {
  EditorView,
  ViewPlugin,
  type PluginValue,
  type ViewUpdate,
} from "@codemirror/view";
import { executeCommand } from "./command-engine";

interface EditorContextMenuOptions {
  dateFormat?: string;
  dateTimeFormat?: string;
}

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
    if (target.closest(".editor-search-bar")) return false;
    if (target.closest(".command-picker-bar")) return false;
    if (target.closest(".variable-autocomplete")) return false;
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

  private createActionButton(label: string, command: string): HTMLButtonElement {
    const button = document.createElement("button");
    button.type = "button";
    button.className = "editor-context-menu-item";
    button.textContent = label;
    button.addEventListener("click", () => {
      void this.runCommand(command);
    });
    return button;
  }

  private createMenuElement(): HTMLDivElement {
    const menu = document.createElement("div");
    menu.className = "editor-context-menu";
    menu.hidden = true;
    menu.setAttribute("role", "menu");
    menu.addEventListener("contextmenu", (event) => event.preventDefault());

    const convertGroup = document.createElement("div");
    convertGroup.className = "editor-context-menu-group";

    const convertTrigger = document.createElement("button");
    convertTrigger.type = "button";
    convertTrigger.className =
      "editor-context-menu-item editor-context-menu-item--submenu-trigger";
    convertTrigger.innerHTML =
      "<span>Convert to</span><span class=\"editor-context-menu-caret\">▸</span>";

    const submenu = document.createElement("div");
    submenu.className = "editor-context-submenu";
    submenu.setAttribute("role", "menu");
    submenu.appendChild(this.createActionButton("Ordered list", "olist"));
    submenu.appendChild(this.createActionButton("Unordered list", "ulist"));
    submenu.appendChild(this.createActionButton("Checklist", "clist"));

    convertGroup.appendChild(convertTrigger);
    convertGroup.appendChild(submenu);

    const formatButton = this.createActionButton("Format", "format");

    menu.appendChild(convertGroup);
    menu.appendChild(formatButton);
    return menu;
  }

  private async runCommand(command: string) {
    this.close();
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
