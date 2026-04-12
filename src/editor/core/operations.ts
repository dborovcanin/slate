import type { EditOperation, OperationSelection, TextChange } from "./types";

export function singleChange(change: TextChange, selection?: OperationSelection): EditOperation {
  return { changes: [change], selection };
}

export function replaceRange(
  from: number,
  to: number,
  insert: string,
  selection?: OperationSelection,
): EditOperation {
  return singleChange({ from, to, insert }, selection);
}
