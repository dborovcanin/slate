import { Prec } from "@codemirror/state";
import { EditorView, keymap, ViewPlugin, type KeyBinding } from "@codemirror/view";
import type { ViewUpdate } from "@codemirror/view";
import { getCalcResultAtCursor } from "./calc-decoration.ts";
import { applyEditOperation, snapshotFromUpdate, snapshotFromView } from "./core/codemirror-adapter.ts";
import {
  markdownClassifyLine,
  runDocChangeRules,
  runEnterRules,
  runTabRules,
  rewriteLineWithChecklistToggleSuffix,
} from "./wasm.ts";

export { formatTableLines } from "./core/markdown-table.ts";
export { rewriteLineWithChecklistToggleSuffix };

function toggleWrap(view: EditorView, left: string, right = left): boolean {
  const main = view.state.selection.main;
  const from = main.from;
  const to = main.to;

  if (main.empty) {
    const insert = left + right;
    view.dispatch({
      changes: { from, to, insert },
      selection: { anchor: from + left.length },
      scrollIntoView: true,
    });
    return true;
  }

  const before = from >= left.length ? view.state.sliceDoc(from - left.length, from) : "";
  const after =
    to + right.length <= view.state.doc.length ? view.state.sliceDoc(to, to + right.length) : "";
  const isWrapped = before === left && after === right;

  if (isWrapped) {
    view.dispatch({
      changes: [
        { from: to, to: to + right.length, insert: "" },
        { from: from - left.length, to: from, insert: "" },
      ],
      selection: { anchor: from - left.length, head: to - left.length },
      scrollIntoView: true,
    });
    return true;
  }

  view.dispatch({
    changes: [
      { from, to: from, insert: left },
      { from: to, to, insert: right },
    ],
    selection: { anchor: from + left.length, head: to + left.length },
    scrollIntoView: true,
  });
  return true;
}

function wrapLink(view: EditorView): boolean {
  const main = view.state.selection.main;
  const selected = view.state.sliceDoc(main.from, main.to);
  if (main.empty) {
    const text = "[text](url)";
    view.dispatch({
      changes: { from: main.from, to: main.to, insert: text },
      selection: { anchor: main.from + 1, head: main.from + 5 },
      scrollIntoView: true,
    });
    return true;
  }

  const link = `[${selected}](url)`;
  view.dispatch({
    changes: { from: main.from, to: main.to, insert: link },
    selection: { anchor: main.from + selected.length + 3, head: main.from + selected.length + 6 },
    scrollIntoView: true,
  });
  return true;
}

function continueListOnEnter(view: EditorView, autoformat: boolean): boolean {
  const operation = runEnterRules(snapshotFromView(view), {
    markdownAutoformat: autoformat,
  });
  if (!operation) return false;
  applyEditOperation(view, operation);
  return true;
}

function indentListOnTab(view: EditorView, autoformat: boolean, outdent = false): boolean {
  if (!autoformat) return false;

  // Prefer calc Tab-apply behavior when a ghost result is available.
  if (!outdent && getCalcResultAtCursor(view) !== null) return false;

  const operation = runTabRules(snapshotFromView(view), {
    markdownAutoformat: autoformat,
    outdent,
  });
  if (!operation) return false;
  applyEditOperation(view, operation);
  return true;
}

function markdownShortcutKeymap(autoformat: boolean): KeyBinding[] {
  const keys: KeyBinding[] = [
    { key: "Mod-b", preventDefault: true, run: (view) => toggleWrap(view, "**") },
    { key: "Mod-i", preventDefault: true, run: (view) => toggleWrap(view, "*") },
    { key: "Mod-Shift-x", preventDefault: true, run: (view) => toggleWrap(view, "~~") },
    { key: "Mod-k", preventDefault: true, run: wrapLink },
  ];

  if (autoformat) {
    keys.push({
      key: "Enter",
      run: (view) => continueListOnEnter(view, autoformat),
      preventDefault: true,
    });
  }

  return keys;
}

function markdownTabKeymap(autoformat: boolean): KeyBinding[] {
  return [
    {
      key: "Tab",
      preventDefault: true,
      run: (view) => indentListOnTab(view, autoformat, false),
    },
    {
      key: "Shift-Tab",
      preventDefault: true,
      run: (view) => indentListOnTab(view, autoformat, true),
    },
  ];
}

function textRulesPlugin(autoformat: boolean, checklistAutoReorder: boolean) {
  return ViewPlugin.define(() => {
    let applying = false;
    return {
      update(update: ViewUpdate) {
        if (applying || !update.docChanged) return;
        applying = true;
        try {
          const operation = runDocChangeRules(snapshotFromUpdate(update), {
            markdownAutoformat: autoformat,
            checklistAutoReorder,
          });
          if (operation) {
            Promise.resolve().then(() => {
              applying = true;
              try {
                applyEditOperation(update.view, operation);
              } catch (error) {
                console.error("Markdown text rule dispatch failed:", error);
              } finally {
                applying = false;
              }
            });
            return;
          }
        } catch (error) {
          console.error("Markdown text rule failed:", error);
        } finally {
          applying = false;
        }
      },
    };
  });
}

function toggleChecklistAtPos(view: EditorView, pos: number): boolean {
  const line = view.state.doc.lineAt(pos);
  const info = markdownClassifyLine(line.text);
  if (info.checklistMarkerStart === null || info.checklistMarkerEnd === null) {
    return false;
  }
  const markerFrom = line.from + info.checklistMarkerStart;
  const markerTo = line.from + info.checklistMarkerEnd;

  const markFrom = markerFrom + 1;
  const markTo = markerTo - 1;
  if (markFrom >= markTo || markFrom < line.from || markTo > line.to) {
    return false;
  }

  const prevSelection = view.state.selection;
  view.dispatch({
    changes: { from: markFrom, to: markTo, insert: info.checklistChecked ? " " : "x" },
    selection: prevSelection,
  });
  return true;
}

function checklistClickHandlers() {
  return EditorView.domEventHandlers({
    mousedown(event) {
      const target = event.target;
      if (!(target instanceof Element)) return false;
      if (!target.closest(".md-checklist-mark")) return false;
      event.preventDefault();
      return true;
    },
    click(event, view) {
      const target = event.target;
      if (!(target instanceof Element)) return false;
      const markEl = target.closest(".md-checklist-mark");
      if (!markEl) return false;
      let pos: number;
      try {
        pos = view.posAtDOM(markEl, 0);
      } catch {
        return false;
      }
      const toggled = toggleChecklistAtPos(view, pos);
      if (!toggled) return false;
      event.preventDefault();
      event.stopPropagation();
      return true;
    },
  });
}

interface MarkdownEditingOptions {
  autoformat?: boolean;
  checklistAutoReorder?: boolean;
}

export function markdownEditingExtensions(options: MarkdownEditingOptions = {}) {
  const autoformat = options.autoformat ?? true;
  const checklistAutoReorder = options.checklistAutoReorder ?? true;
  return [
    Prec.high(keymap.of(markdownShortcutKeymap(autoformat))),
    Prec.low(keymap.of(markdownTabKeymap(autoformat))),
    checklistClickHandlers(),
    textRulesPlugin(autoformat, checklistAutoReorder),
  ];
}
