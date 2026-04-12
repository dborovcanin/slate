export interface SelectionSnapshot {
  anchor: number;
  head: number;
}

export interface TextRange {
  from: number;
  to: number;
}

export interface EditorContextSnapshot {
  text: string;
  selection: SelectionSnapshot;
  changedRange?: TextRange;
}

export interface LineContext {
  number: number; // 1-based
  from: number;
  to: number;
  text: string;
}

export interface SelectionContext {
  anchor: number;
  head: number;
  from: number;
  to: number;
  empty: boolean;
}

export interface WordContext {
  from: number;
  to: number;
  text: string;
}

export interface BlockLineRange {
  startLine: number;
  endLine: number;
}

export interface TextChange {
  from: number;
  to: number;
  insert: string;
}

export interface OperationSelection {
  anchor: number;
  head?: number;
}

export interface EditOperation {
  changes: TextChange[];
  selection?: OperationSelection;
}

export type RuleTrigger = "doc_change" | "key_enter";

export type CommandMode = "vim" | "editor";

export interface CommandSuggestion {
  value: string;
  description: string;
}
