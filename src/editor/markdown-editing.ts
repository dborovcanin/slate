import { Prec } from "@codemirror/state";
import { keymap, ViewPlugin, type KeyBinding } from "@codemirror/view";
import type { EditorView, ViewUpdate } from "@codemirror/view";
import { getCalcResultAtCursor } from "./calc-decoration.ts";
import { applyEditOperation, snapshotFromUpdate, snapshotFromView } from "./core/codemirror-adapter.ts";
import { runDocChangeRules, runEnterRules, runTabRules } from "./core/text-rules.ts";

export { formatTableLines } from "./core/markdown-table.ts";
export { rewriteLineWithChecklistToggleSuffix } from "./core/text-rules.ts";

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

function textRulesPlugin(enabled: boolean) {
  return ViewPlugin.define(() => {
    let applying = false;
    return {
      update(update: ViewUpdate) {
        if (applying || !update.docChanged) return;
        applying = true;
        try {
          try {
            const operation = runDocChangeRules(snapshotFromUpdate(update), {
              markdownAutoformat: enabled,
            });
            if (operation) {
              applyEditOperation(update.view, operation);
            }
          } catch (error) {
            // Keep editing and markdown rendering alive even if a rule fails.
            console.error("Markdown text rule failed:", error);
          }
        } finally {
          applying = false;
        }
      },
    };
  });
}

interface MarkdownEditingOptions {
  autoformat?: boolean;
}

export function markdownEditingExtensions(options: MarkdownEditingOptions = {}) {
  const autoformat = options.autoformat ?? true;
  return [
    Prec.high(keymap.of(markdownShortcutKeymap(autoformat))),
    Prec.low(keymap.of(markdownTabKeymap(autoformat))),
    textRulesPlugin(autoformat),
  ];
}
