import type { ChangeDesc } from "@codemirror/state";
import type { EditorView, ViewUpdate } from "@codemirror/view";
import type { EditOperation, EditorContextSnapshot, TextRange } from "./types.ts";

interface ScopedSnapshot {
  snapshot: EditorContextSnapshot;
  offset: number;
}

function isMarkdownTableLine(text: string): boolean {
  const trimmed = text.trim();
  return trimmed.startsWith("|") && trimmed.endsWith("|");
}

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

// Extract a windowed snapshot around the cursor (and optionally a changed range),
// expanded by `marginLines` lines in each direction. Uses doc.sliceString so only
// the window bytes cross the WASM boundary instead of the full document.
export function snapshotFromViewLines(
  view: EditorView,
  marginLines: number,
  changedRange?: TextRange,
): ScopedSnapshot {
  const doc = view.state.doc;
  const sel = view.state.selection.main;
  const curLineNo = doc.lineAt(sel.head).number;

  let startLineNo = Math.max(1, curLineNo - marginLines);
  let endLineNo = Math.min(doc.lines, curLineNo + marginLines);

  if (changedRange) {
    const safeFrom = Math.min(changedRange.from, doc.length);
    const safeTo = Math.min(changedRange.to, doc.length);
    const changeStart = doc.lineAt(safeFrom).number;
    const changeEnd = doc.lineAt(safeTo).number;
    startLineNo = Math.max(1, Math.min(startLineNo, changeStart - marginLines));
    endLineNo = Math.min(doc.lines, Math.max(endLineNo, changeEnd + marginLines));
  }

  const windowFrom = doc.line(startLineNo).from;
  const windowTo = doc.line(endLineNo).to;
  const localMax = windowTo - windowFrom;
  const localAnchor = Math.max(0, Math.min(localMax, sel.anchor - windowFrom));
  const localHead = Math.max(0, Math.min(localMax, sel.head - windowFrom));

  return {
    offset: windowFrom,
    snapshot: {
      text: doc.sliceString(windowFrom, windowTo),
      selection: { anchor: localAnchor, head: localHead },
      changedRange: changedRange
        ? {
            from: Math.max(0, Math.min(localMax, changedRange.from - windowFrom)),
            to: Math.max(0, Math.min(localMax, changedRange.to - windowFrom)),
          }
        : undefined,
    },
  };
}

export function snapshotFromViewTableBlock(view: EditorView): ScopedSnapshot | null {
  const selection = view.state.selection.main;
  const current = view.state.doc.lineAt(selection.head);
  if (!isMarkdownTableLine(current.text)) return null;

  const lineCount = view.state.doc.lines;
  let startLine = current.number;
  let endLine = current.number;

  while (startLine > 1 && isMarkdownTableLine(view.state.doc.line(startLine - 1).text)) {
    startLine -= 1;
  }
  while (endLine < lineCount && isMarkdownTableLine(view.state.doc.line(endLine + 1).text)) {
    endLine += 1;
  }

  const from = view.state.doc.line(startLine).from;
  const to = view.state.doc.line(endLine).to;
  const localMax = to - from;
  const localAnchor = Math.max(0, Math.min(localMax, selection.anchor - from));
  const localHead = Math.max(0, Math.min(localMax, selection.head - from));

  return {
    offset: from,
    snapshot: {
      text: view.state.doc.sliceString(from, to),
      selection: {
        anchor: localAnchor,
        head: localHead,
      },
    },
  };
}

export function offsetEditOperation(operation: EditOperation, offset: number): EditOperation {
  if (offset === 0) return operation;
  return {
    ...operation,
    changes: operation.changes.map((change) => ({
      ...change,
      from: change.from + offset,
      to: change.to + offset,
    })),
    selection: operation.selection
      ? {
          anchor: operation.selection.anchor + offset,
          head:
            operation.selection.head === undefined || operation.selection.head === null
              ? operation.selection.head
              : operation.selection.head + offset,
        }
      : undefined,
  };
}

export function applyEditOperation(view: EditorView, operation: EditOperation): void {
  const selection = operation.selection
    ? {
        anchor: operation.selection.anchor,
        head:
          operation.selection.head === undefined || operation.selection.head === null
            ? operation.selection.anchor
            : operation.selection.head,
      }
    : undefined;

  if (operation.changes.length === 0) {
    if (!selection) return;
    view.dispatch({
      selection,
      scrollIntoView: true,
    });
    return;
  }

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
