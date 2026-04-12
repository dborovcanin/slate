import type { ChangeDesc } from "@codemirror/state";
import type { EditorView, ViewUpdate } from "@codemirror/view";
import type { EditOperation, EditorContextSnapshot, TextRange } from "./types.ts";

export function changedRangeFromChanges(changes: ChangeDesc): TextRange | undefined {
  let from = Number.POSITIVE_INFINITY;
  let to = Number.NEGATIVE_INFINITY;

  changes.iterChangedRanges((_fromA, _toA, fromB, toB) => {
    from = Math.min(from, fromB);
    to = Math.max(to, toB);
  });

  if (!Number.isFinite(from) || !Number.isFinite(to)) return undefined;
  return { from, to };
}

export function snapshotFromView(
  view: EditorView,
  changedRange?: TextRange,
): EditorContextSnapshot {
  const selection = view.state.selection.main;
  return {
    text: view.state.doc.toString(),
    selection: {
      anchor: selection.anchor,
      head: selection.head,
    },
    changedRange,
  };
}

export function snapshotFromUpdate(update: ViewUpdate): EditorContextSnapshot {
  return snapshotFromView(update.view, changedRangeFromChanges(update.changes));
}

export function applyEditOperation(view: EditorView, operation: EditOperation): void {
  if (operation.changes.length === 0) return;

  const selection = operation.selection
    ? operation.selection.head === undefined
      ? { anchor: operation.selection.anchor }
      : {
          anchor: operation.selection.anchor,
          head: operation.selection.head,
        }
    : undefined;

  view.dispatch({
    changes: operation.changes.length === 1 ? operation.changes[0] : operation.changes,
    selection,
    scrollIntoView: true,
  });
}

export function applyEditOperations(view: EditorView, operations: EditOperation[]): void {
  for (const operation of operations) {
    applyEditOperation(view, operation);
  }
}
