export interface AppMenuItem {
  label: string;
  shortcut?: string;
  action: () => void | Promise<void>;
  disabled?: () => boolean;
}

export interface AppMenuGroup {
  items: AppMenuItem[];
}

export interface AppMenuSection {
  label: string;
  groups: AppMenuGroup[];
}

export interface AppMenuController {
  close: () => void;
}

function menuItemDisabled(item: AppMenuItem): boolean {
  return item.disabled?.() ?? false;
}

export function mountAppMenu(
  container: HTMLElement,
  sections: AppMenuSection[],
): AppMenuController {
  const root = document.createElement("nav");
  root.className = "app-menu";
  root.setAttribute("aria-label", "Application menu");

  let openIndex: number | null = null;
  const sectionButtons: HTMLButtonElement[] = [];
  const panels: HTMLDivElement[] = [];

  const close = () => {
    if (openIndex === null) return;
    const button = sectionButtons[openIndex];
    const panel = panels[openIndex];
    button?.setAttribute("aria-expanded", "false");
    panel?.classList.remove("app-menu-panel--open");
    openIndex = null;
  };

  const open = (index: number) => {
    if (openIndex === index) {
      close();
      return;
    }
    close();
    openIndex = index;
    const button = sectionButtons[index];
    const panel = panels[index];
    button?.setAttribute("aria-expanded", "true");
    panel?.classList.add("app-menu-panel--open");
    const rect = button?.getBoundingClientRect();
    if (rect && panel) {
      const margin = 8;
      const maxLeft = window.innerWidth - panel.offsetWidth - margin;
      panel.style.left = `${Math.max(margin, Math.min(rect.left, maxLeft))}px`;
      panel.style.top = `${rect.bottom}px`;
    }
  };

  const runItem = (item: AppMenuItem) => {
    if (menuItemDisabled(item)) return;
    close();
    void Promise.resolve()
      .then(item.action)
      .catch((error) => {
        console.error("Menu action failed:", error);
      });
  };

  sections.forEach((section, index) => {
    const button = document.createElement("button");
    button.type = "button";
    button.className = "app-menu-trigger";
    button.textContent = section.label;
    button.setAttribute("aria-haspopup", "menu");
    button.setAttribute("aria-expanded", "false");
    button.addEventListener("click", () => open(index));
    button.addEventListener("pointerenter", () => {
      if (openIndex !== null && openIndex !== index) open(index);
    });
    button.addEventListener("keydown", (event) => {
      if (event.key === "ArrowDown" || event.key === "Enter" || event.key === " ") {
        event.preventDefault();
        open(index);
        panels[index]?.querySelector<HTMLButtonElement>(".app-menu-item:not(:disabled)")?.focus();
        return;
      }
      if (event.key === "ArrowRight") {
        event.preventDefault();
        sectionButtons[(index + 1) % sectionButtons.length]?.focus();
        return;
      }
      if (event.key === "ArrowLeft") {
        event.preventDefault();
        sectionButtons[(index - 1 + sectionButtons.length) % sectionButtons.length]?.focus();
      }
    });

    const panel = document.createElement("div");
    panel.className = "app-menu-panel";
    panel.setAttribute("role", "menu");
    panel.setAttribute("aria-label", section.label);
    panel.addEventListener("keydown", (event) => {
      if (event.key === "Escape") {
        event.preventDefault();
        close();
        button.focus();
        return;
      }
      if (event.key !== "ArrowDown" && event.key !== "ArrowUp") return;
      const focusable = Array.from(
        panel.querySelectorAll<HTMLButtonElement>(".app-menu-item:not(:disabled)"),
      );
      if (focusable.length === 0) return;
      event.preventDefault();
      const current = document.activeElement;
      const currentIndex = focusable.findIndex((entry) => entry === current);
      const delta = event.key === "ArrowDown" ? 1 : -1;
      const next = (Math.max(0, currentIndex) + delta + focusable.length) % focusable.length;
      focusable[next]?.focus();
    });

    section.groups.forEach((group, groupIndex) => {
      if (groupIndex > 0) {
        const separator = document.createElement("div");
        separator.className = "app-menu-separator";
        separator.setAttribute("role", "separator");
        panel.appendChild(separator);
      }
      for (const item of group.items) {
        const itemButton = document.createElement("button");
        itemButton.type = "button";
        itemButton.className = "app-menu-item";
        itemButton.setAttribute("role", "menuitem");
        itemButton.disabled = menuItemDisabled(item);

        const label = document.createElement("span");
        label.className = "app-menu-item-label";
        label.textContent = item.label;
        itemButton.appendChild(label);

        if (item.shortcut) {
          const shortcut = document.createElement("span");
          shortcut.className = "app-menu-item-shortcut";
          shortcut.textContent = item.shortcut;
          itemButton.appendChild(shortcut);
        }

        itemButton.addEventListener("click", () => runItem(item));
        panel.appendChild(itemButton);
      }
    });

    sectionButtons.push(button);
    panels.push(panel);
    root.appendChild(button);
    document.body.appendChild(panel);
  });

  const onPointerDown = (event: PointerEvent) => {
    const target = event.target as Node | null;
    if (!target) return;
    if (root.contains(target) || panels.some((panel) => panel.contains(target))) return;
    close();
  };
  const onKeydown = (event: KeyboardEvent) => {
    if (event.key === "Escape") close();
  };
  const onResize = () => close();

  document.addEventListener("pointerdown", onPointerDown, true);
  window.addEventListener("keydown", onKeydown, true);
  window.addEventListener("resize", onResize);
  container.appendChild(root);

  return { close };
}
