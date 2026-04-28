import init, {
  WasmVimSession,
  initSync,
  wasm_convert_line_to_list,
  wasm_calc_builtin_formula_label,
  wasm_calc_builtin_formula_labels,
  wasm_calc_compute_refresh,
  wasm_calc_decide_eval_scope,
  wasm_calc_decide_eval_window,
  wasm_calc_contains_builtin_formula,
  wasm_calc_contains_variable_assignment,
  wasm_calc_find_list_segment,
  wasm_calc_find_segment,
  wasm_calc_find_single_table_cell,
  wasm_calc_find_table_formula_segment,
  wasm_calc_find_table_formula_segments,
  wasm_calc_format_formula_display_value,
  wasm_calc_is_builtin_formula,
  wasm_calc_line_for_eval,
  wasm_calc_line_uses_assignment_prefix,
  wasm_calc_plan_incremental,
  wasm_calc_should_schedule_eval,
  wasm_command_history_next,
  wasm_command_history_prev,
  wasm_command_history_remember,
  wasm_command_history_sanitize,
  wasm_format_markdown,
  wasm_format_table_lines,
  wasm_list_command_suggestions,
  wasm_markdown_analyze_lines,
  wasm_markdown_build_fold_ranges_ui,
  wasm_markdown_fold_edits_require_rebuild_ui,
  wasm_markdown_fold_map_ranges_ui,
  wasm_markdown_classify_line,
  wasm_markdown_find_inline_tokens,
  wasm_markdown_inline_marker_component_ranges,
  wasm_markdown_is_code_fence,
  wasm_markdown_parse_fence_language,
  wasm_markdown_tokenize_code_line,
  wasm_normalize_command,
  wasm_rewrite_line_with_checklist_toggle_suffix,
  wasm_resolve_command,
  wasm_classify_command_dispatch,
  wasm_plan_host_command,
  wasm_execute_vim_action,
  wasm_execute_math_command,
  wasm_parse_note_security_command,
  wasm_plan_module_command,
  wasm_try_execute_vim_substitute,
  wasm_run_markdown_transaction_batch,
} from "../../pkg/editor-core/editor_core.js";
// NOTE: wasm functions for text rules now receive direct args instead of JSON snapshots.
import type {
  CommandMode,
  CommandSuggestion,
  EditOperation,
  EditorContextSnapshot,
} from "./core/types.ts";
import { startupMark } from "../perf/startup.ts";
import {
  editorProfilerNowMs,
  isEditorProfilerEnabled,
  recordEditorProfilerSample,
  type EditorProfilerMetrics,
} from "../perf/editor-profiler.ts";

// Initialize non-blocking in the browser to avoid delaying first paint.
// In Node.js tests we can call ensureWasmReady() up front.
// eslint-disable-next-line @typescript-eslint/no-explicit-any
const _isNode = typeof (globalThis as any).process?.versions?.node === "string";
let _ready = false;
let _initPromise: Promise<void> | null = null;
let _initErrorLogged = false;

async function initForNode(): Promise<void> {
  const { readFileSync } = await import("fs" as string);
  const { fileURLToPath } = await import("url" as string);
  const { resolve, dirname } = await import("path" as string);
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  const __filename = fileURLToPath((import.meta as any).url);
  const __dirname = dirname(__filename);
  const wasmPath = resolve(__dirname, "../../pkg/editor-core/editor_core_bg.wasm");
  const wasmBytes = readFileSync(wasmPath);
  initSync({ module: wasmBytes });
}

function logInitError(error: unknown) {
  if (_initErrorLogged) return;
  _initErrorLogged = true;
  console.error("Failed to initialize editor-core wasm:", error);
}

export function ensureWasmReady(): Promise<void> {
  if (_ready) return Promise.resolve();
  if (_initPromise) return _initPromise;

  _initPromise = (_isNode ? initForNode() : init())
    .then(() => {
      _ready = true;
      startupMark("ui_wasm_ready");
    })
    .catch((error) => {
      _initPromise = null;
      logInitError(error);
      throw error;
    });

  return _initPromise;
}

export function isWasmReady(): boolean {
  return _ready;
}

function ensureWasmReadyNonBlocking(): boolean {
  if (_ready) return true;
  void ensureWasmReady().catch(logInitError);
  return false;
}

export interface TextRuleOptions {
  markdownAutoformat?: boolean;
  checklistAutoReorder?: boolean;
  tableEnabled?: boolean;
}

export interface TabRuleOptions {
  markdownAutoformat?: boolean;
  outdent?: boolean;
  tableEnabled?: boolean;
}

export interface TableBoundaryEditOptions {
  markdownAutoformat?: boolean;
  backward?: boolean;
  structuralMerge?: boolean;
  tableEnabled?: boolean;
}

export type MarkdownTransactionKind =
  | "doc_change"
  | "enter"
  | "tab"
  | "table_cell_navigation"
  | "table_pipe_insert_column"
  | "table_header_delete_column"
  | "table_boundary_edit";

export interface MarkdownTransactionRequest {
  kind: MarkdownTransactionKind;
  markdownAutoformat?: boolean;
  checklistAutoReorder?: boolean;
  outdent?: boolean;
  backward?: boolean;
  structuralMerge?: boolean;
  tableEnabled?: boolean;
}

export interface MarkdownTransactionResult {
  kind: MarkdownTransactionKind;
  operation: EditOperation;
}

export type VimMode = "insert" | "normal" | "visual" | "visual_line";

export interface VimContext {
  has_search_matches?: boolean;
  line_count?: number;
}

export interface VimKeyInput {
  kind: number;
  charCode?: number;
}

export const VIM_INTENT = {
  MOVE_LEFT: "move_left",
  MOVE_RIGHT: "move_right",
  MOVE_UP: "move_up",
  MOVE_DOWN: "move_down",
  MOVE_WORD_FORWARD: "move_word_forward",
  MOVE_WORD_BACKWARD: "move_word_backward",
  MOVE_LINE_START: "move_line_start",
  MOVE_LINE_END: "move_line_end",
  MOVE_DOC_START: "move_doc_start",
  MOVE_DOC_END: "move_doc_end",
  MOVE_TO_LINE: "move_to_line",
  ENTER_INSERT: "enter_insert",
  APPEND_INSERT: "append_insert",
  INSERT_LINE_START: "insert_line_start",
  APPEND_LINE_END: "append_line_end",
  OPEN_LINE_BELOW: "open_line_below",
  OPEN_LINE_ABOVE: "open_line_above",
  ENTER_VISUAL: "enter_visual",
  ENTER_VISUAL_LINE: "enter_visual_line",
  EXIT_VISUAL: "exit_visual",
  DELETE_LINE: "delete_line",
  YANK_LINE: "yank_line",
  DELETE_TO_LINE_START: "delete_to_line_start",
  DELETE_TO_LINE_END: "delete_to_line_end",
  YANK_TO_LINE_START: "yank_to_line_start",
  YANK_TO_LINE_END: "yank_to_line_end",
  DELETE_CHAR: "delete_char",
  PASTE_AFTER: "paste_after",
  UNDO: "undo",
  REDO: "redo",
  OPEN_COMMAND_BAR: "open_command_bar",
  OPEN_SEARCH: "open_search",
  SEARCH_NEXT: "search_next",
  SEARCH_PREV: "search_prev",
  DELETE_INSIDE_WORD: "delete_inside_word",
  DELETE_AROUND_WORD: "delete_around_word",
  YANK_INSIDE_WORD: "yank_inside_word",
  YANK_AROUND_WORD: "yank_around_word",
  DELETE_INSIDE_PIPE: "delete_inside_pipe",
  DELETE_AROUND_PIPE: "delete_around_pipe",
  YANK_INSIDE_PIPE: "yank_inside_pipe",
  YANK_AROUND_PIPE: "yank_around_pipe",
  SWALLOW: "swallow",
  DELETE_WORD_FORWARD: "delete_word_forward",
  DELETE_WORD_BACKWARD: "delete_word_backward",
  DELETE_WORD_END: "delete_word_end",
  YANK_WORD_FORWARD: "yank_word_forward",
  YANK_WORD_BACKWARD: "yank_word_backward",
  YANK_VISUAL_SELECTION: "yank_visual_selection",
  DELETE_VISUAL_SELECTION: "delete_visual_selection",
} as const;

export type VimIntent = (typeof VIM_INTENT)[keyof typeof VIM_INTENT];

export interface VimAction {
  intent: VimIntent;
  count: number;
}

export interface VimStep {
  mode: VimMode;
  actions: VimAction[];
  handled: boolean;
}

export const VIM_KEY_KIND = {
  ESC: 0,
  ENTER: 1,
  TAB: 2,
  BACKSPACE: 3,
  DELETE: 4,
  ARROW_UP: 5,
  ARROW_DOWN: 6,
  ARROW_LEFT: 7,
  ARROW_RIGHT: 8,
  CHAR: 9,
  CTRL: 10,
} as const;

const VIM_INTENT_VALUES = new Set<VimIntent>(
  Object.values(VIM_INTENT) as VimIntent[],
);

function asVimIntent(value: unknown): VimIntent | null {
  return typeof value === "string" && VIM_INTENT_VALUES.has(value as VimIntent)
    ? (value as VimIntent)
    : null;
}

const VIM_MODE_ID = {
  INSERT: 0,
  NORMAL: 1,
  VISUAL: 2,
  VISUAL_LINE: 3,
} as const;

export interface CalcSegment {
  expr: string;
  fromCol: number;
  toCol: number;
  fromByte: number;
  toByte: number;
}

export interface TableFormulaSegment {
  fromByte: number;
  toByte: number;
  fromChar: number;
  toChar: number;
  cellLeftPipeChar: number;
  cellRightPipeChar: number;
  cellIndex: number;
  labels: string[];
}

export interface CommitMarkerLoc {
  docPos: number;
  lineIdx: number;
  offsetInLine: number;
  lastLiteral: string;
}

export interface CalcRefreshChange {
  lineIdx: number;
  from: number;
  to: number;
  insert: string;
  newLiteral: string;
}

export interface CalcRefreshPlan {
  changes: CalcRefreshChange[];
  prune: number[];
  syncedLines: number[];
}

export interface CalcEvalScopeDecision {
  touchesAnyAssignment: boolean;
  touchesBuiltinFormula: boolean;
  canUsePartial: boolean;
}

export interface CalcEvalWindowDecision {
  evalFrom: number;
  evalTo: number;
  touchesAnyAssignment: boolean;
  touchesBuiltinFormula: boolean;
  canUsePartial: boolean;
}

export interface IncrementalCalcPlan {
  baseResults: Map<number, string>;
  evalFrom: number;
  evalTo: number;
  evalLines: string[];
}

interface LineCalcResultEntry {
  lineIdx: number;
  result: string;
}

interface IncrementalCalcPlanPayload {
  baseResults: LineCalcResultEntry[];
  evalFrom: number;
  evalTo: number;
  evalLines: string[];
}

interface EditOperationWire {
  changes: EditOperation["changes"];
  selection?: { anchor: number; head?: number | null } | null;
}

interface WasmCommandExecutionResultWire {
  message: string;
  operations: EditOperationWire[];
  clipboardText?: string | null;
  quitRequested?: boolean;
}

interface VimActionExecutionResultWire {
  operations: EditOperationWire[];
  register?: VimRegisterValue | null;
}

interface MarkdownTransactionResultWire {
  kind: MarkdownTransactionKind;
  operation: EditOperationWire;
}

export type MarkdownInlineTokenType =
  | "strong"
  | "emphasis"
  | "strikethrough"
  | "code"
  | "code-marker"
  | "link-text"
  | "link-url"
  | "link-marker";

export type MarkdownCodeTokenType =
  | "keyword"
  | "string"
  | "number"
  | "comment"
  | "function"
  | "type";

export interface MarkdownInlineToken {
  from: number;
  to: number;
  type: MarkdownInlineTokenType;
}

export interface MarkdownCodeToken {
  from: number;
  to: number;
  type: MarkdownCodeTokenType;
}

export interface MarkdownInlineMarkerComponentRange {
  from: number;
  to: number;
}

export interface MarkdownFoldRange {
  startLine: number; // 1-based
  endLine: number; // 1-based
  kind: "heading" | "fence" | "list" | "table" | "paragraph";
}

export interface MarkdownFoldLineEdit {
  oldStartLine: number; // 1-based
  oldLineSpan: number;
  newLineSpan: number;
  oldLineText: string;
  newLineText: string;
}

export interface VimSubstituteExecutionResult {
  message: string;
  operations: EditOperation[];
}

export interface WasmCommandExecutionResult {
  message: string;
  operations: EditOperation[];
  clipboardText: string | null;
  quitRequested: boolean;
}

export type VimRegisterMode = "charwise" | "linewise";

export interface VimRegisterValue {
  text: string;
  mode: VimRegisterMode;
}

export interface VimActionExecutionResult {
  operations: EditOperation[];
  register: VimRegisterValue | null;
}

export type CommandDispatchKind =
  | "core"
  | "host_date"
  | "host_notify"
  | "host_notify_delete"
  | "host_write"
  | "host_module"
  | "host_fold"
  | "host_clip_watch"
  | "host_note_security"
  | "quit";

export type HostFoldAction = "fold" | "unfold" | "toggle";
export type HostClipWatchAction = "start" | "stop";

export type HostCommandPlan =
  | { kind: "date" }
  | { kind: "notify" }
  | { kind: "notify_delete" }
  | { kind: "write"; quit: boolean; force: boolean }
  | { kind: "module"; command: string }
  | { kind: "fold"; action: HostFoldAction }
  | { kind: "clip_watch"; action: HostClipWatchAction }
  | { kind: "note_security"; action: NoteSecurityAction; password: string }
  | { kind: "quit"; force: boolean };

interface CommandHistoryStepPayload {
  index: number;
  command: string;
}

export interface MarkdownLineInfo {
  headingLevel: number | null;
  headingMarkerEnd: number | null;
  quoteMarkerEnd: number | null;
  listMarkerEnd: number | null;
  checklistMarkerStart: number | null;
  checklistMarkerEnd: number | null;
  checklistContentStart: number | null;
  checklistChecked: boolean;
  isHorizontalRule: boolean;
  isCodeFence: boolean;
}

export interface MarkdownAnalyzedLine {
  info: MarkdownLineInfo;
  inCodeBlock: boolean;
  codeFenceLang: string | null;
  inlineTokens: MarkdownInlineToken[];
  codeTokens: MarkdownCodeToken[];
}

export interface MarkdownAnalyzeResult {
  lines: MarkdownAnalyzedLine[];
  finalInCodeBlock: boolean;
  finalCodeFenceLang: string | null;
}

function asVimMode(mode: unknown): VimMode {
  if (mode === "insert" || mode === "normal" || mode === "visual" || mode === "visual_line") {
    return mode;
  }
  return "normal";
}

function modeToId(mode: VimMode): number {
  switch (mode) {
    case "insert":
      return VIM_MODE_ID.INSERT;
    case "normal":
      return VIM_MODE_ID.NORMAL;
    case "visual":
      return VIM_MODE_ID.VISUAL;
    case "visual_line":
      return VIM_MODE_ID.VISUAL_LINE;
    default:
      return VIM_MODE_ID.NORMAL;
  }
}

// Convert multiple UTF-16 char offsets to UTF-8 byte offsets in a single O(max_offset) walk.
// Avoids N separate TextEncoder.encode(text.slice(0, offset)) calls (each O(offset) + alloc).
function batchUtf16ToUtf8(text: string, utf16Offsets: readonly number[]): number[] {
  const len = utf16Offsets.length;
  if (len === 0) return [];
  const results = new Array<number>(len).fill(0);
  // Pair each offset with its original index, sort ascending so we can walk once.
  const sorted = utf16Offsets.map((o, i) => ({ o: Math.max(0, Math.min(o, text.length)), i }));
  sorted.sort((a, b) => a.o - b.o);
  let bytePos = 0;
  let charPos = 0;
  for (const { o, i } of sorted) {
    while (charPos < o) {
      const cp = text.codePointAt(charPos);
      if (cp === undefined) break;
      bytePos += cp <= 0x7f ? 1 : cp <= 0x7ff ? 2 : cp <= 0xffff ? 3 : 4;
      charPos += cp > 0xffff ? 2 : 1;
    }
    results[i] = bytePos;
  }
  return results;
}

function utf8ToUtf16Offset(text: string, utf8Offset: number): number {
  const target = Math.max(0, utf8Offset);
  let bytes = 0;
  let index = 0;
  while (index < text.length) {
    const cp = text.codePointAt(index);
    if (cp === undefined) break;
    const utf16Len = cp > 0xffff ? 2 : 1;
    const utf8Len = cp <= 0x7f ? 1 : cp <= 0x7ff ? 2 : cp <= 0xffff ? 3 : 4;
    if (bytes + utf8Len > target) break;
    bytes += utf8Len;
    index += utf16Len;
    if (bytes === target) return index;
  }
  return index;
}

function normalizeOptionalOffset(value: number | null | undefined): number | undefined {
  return typeof value === "number" && Number.isFinite(value) ? value : undefined;
}

function applyChangesToText(text: string, changes: EditOperation["changes"]): string {
  const ordered = [...changes].sort((a, b) => b.from - a.from || b.to - a.to);
  let next = text;
  for (const change of ordered) {
    const from = Math.max(0, Math.min(change.from, next.length));
    const to = Math.max(from, Math.min(change.to, next.length));
    next = next.slice(0, from) + change.insert + next.slice(to);
  }
  return next;
}

function mapOperationFromUtf8ToUtf16(sourceText: string, operation: EditOperation): EditOperation {
  const convertedChanges = operation.changes.map((change) => ({
    from: utf8ToUtf16Offset(sourceText, change.from),
    to: utf8ToUtf16Offset(sourceText, change.to),
    insert: change.insert,
  }));

  const mapped: EditOperation = {
    ...operation,
    changes: convertedChanges,
  };

  if (operation.selection) {
    const rawHead = normalizeOptionalOffset(
      (operation.selection as { head?: number | null }).head,
    );
    const postText = applyChangesToText(sourceText, convertedChanges);
    const mappedSelection: NonNullable<EditOperation["selection"]> = {
      anchor: utf8ToUtf16Offset(postText, operation.selection.anchor),
    };
    if (rawHead !== undefined) {
      mappedSelection.head = utf8ToUtf16Offset(postText, rawHead);
    }
    mapped.selection = mappedSelection;
  }

  return mapped;
}

function snapshotToArgs(snapshot: EditorContextSnapshot): [string, number, number, boolean, number, number] {
  const text = snapshot.text;
  const hasRange = snapshot.changedRange !== undefined;
  const [anchor, head, changedFrom, changedTo] = batchUtf16ToUtf8(text, [
    snapshot.selection.anchor,
    snapshot.selection.head,
    snapshot.changedRange?.from ?? 0,
    snapshot.changedRange?.to ?? 0,
  ]);
  return [text, anchor!, head!, hasRange, changedFrom!, changedTo!];
}

function mapOperationWireFromUtf8ToUtf16(
  sourceText: string,
  operation: EditOperationWire,
): EditOperation {
  const selection = operation.selection
    ? {
      anchor: operation.selection.anchor,
      head: normalizeOptionalOffset(operation.selection.head),
    }
    : undefined;
  return mapOperationFromUtf8ToUtf16(sourceText, {
    changes: operation.changes,
    selection,
  });
}

function mapOperationWiresFromUtf8ToUtf16(
  sourceText: string,
  operations: readonly EditOperationWire[] | null | undefined,
): EditOperation[] {
  if (!Array.isArray(operations)) return [];
  return operations.map((operation) => mapOperationWireFromUtf8ToUtf16(sourceText, operation));
}

function decodeVimSubstituteExecutionResult(
  raw: unknown,
  sourceText: string,
): VimSubstituteExecutionResult | null {
  const decoded = decodeCommandExecutionResult(raw, sourceText);
  if (!decoded) return null;
  return {
    message: decoded.message,
    operations: decoded.operations,
  };
}

function decodeCommandExecutionResult(
  raw: unknown,
  sourceText: string,
): WasmCommandExecutionResult | null {
  if (typeof raw !== "object" || raw === null) return null;
  const payload = raw as WasmCommandExecutionResultWire;
  if (typeof payload.message !== "string") return null;
  return {
    message: payload.message,
    operations: mapOperationWiresFromUtf8ToUtf16(sourceText, payload.operations),
    clipboardText: typeof payload.clipboardText === "string" ? payload.clipboardText : null,
    quitRequested: payload.quitRequested === true,
  };
}

function decodeVimActionExecutionResult(
  raw: unknown,
  sourceText: string,
): VimActionExecutionResult | null {
  if (typeof raw !== "object" || raw === null) return null;
  const payload = raw as VimActionExecutionResultWire;
  let register: VimRegisterValue | null = null;
  if (typeof payload.register === "object" && payload.register !== null) {
    const mode = payload.register.mode;
    const text = payload.register.text;
    if (
      (mode === "charwise" || mode === "linewise") &&
      typeof text === "string"
    ) {
      register = {
        mode,
        text,
      };
    }
  }

  return {
    operations: mapOperationWiresFromUtf8ToUtf16(sourceText, payload.operations),
    register,
  };
}

function asMarkdownTransactionKind(value: unknown): MarkdownTransactionKind | null {
  if (typeof value !== "string") return null;
  if (
    value === "doc_change" ||
    value === "enter" ||
    value === "tab" ||
    value === "table_cell_navigation" ||
    value === "table_pipe_insert_column" ||
    value === "table_header_delete_column" ||
    value === "table_boundary_edit"
  ) {
    return value;
  }
  return null;
}

function decodeMarkdownTransactionResult(
  raw: unknown,
  sourceText: string,
): MarkdownTransactionResult | null {
  if (typeof raw !== "object" || raw === null) return null;
  const payload = raw as MarkdownTransactionResultWire;
  const kind = asMarkdownTransactionKind(payload.kind);
  if (!kind) return null;
  const operation = mapOperationWireFromUtf8ToUtf16(sourceText, payload.operation);
  return { kind, operation };
}

export function runMarkdownTransactions(
  snapshot: EditorContextSnapshot,
  transactions: readonly MarkdownTransactionRequest[],
): MarkdownTransactionResult | null {
  if (!ensureWasmReadyNonBlocking()) return null;
  if (transactions.length === 0) return null;
  const [text, anchor, head, hasRange, changedFrom, changedTo] = snapshotToArgs(snapshot);
  const raw = wasm_run_markdown_transaction_batch(
    text,
    anchor,
    head,
    hasRange,
    changedFrom,
    changedTo,
    [...transactions],
  ) as unknown;
  return decodeMarkdownTransactionResult(raw, snapshot.text);
}

export function runDocChangeRules(
  snapshot: EditorContextSnapshot,
  options: TextRuleOptions = {},
): EditOperation | null {
  const result = runMarkdownTransactions(snapshot, [
    {
      kind: "doc_change",
      markdownAutoformat: options.markdownAutoformat ?? true,
      checklistAutoReorder: options.checklistAutoReorder ?? true,
      tableEnabled: options.tableEnabled ?? true,
    },
  ]);
  return result?.operation ?? null;
}

export function runEnterRules(
  snapshot: EditorContextSnapshot,
  options: TextRuleOptions = {},
): EditOperation | null {
  const result = runMarkdownTransactions(snapshot, [
    {
      kind: "enter",
      markdownAutoformat: options.markdownAutoformat ?? true,
      tableEnabled: options.tableEnabled ?? true,
    },
  ]);
  return result?.operation ?? null;
}

export function runTabRules(
  snapshot: EditorContextSnapshot,
  options: TabRuleOptions = {},
): EditOperation | null {
  const result = runMarkdownTransactions(snapshot, [
    {
      kind: "tab",
      markdownAutoformat: options.markdownAutoformat ?? true,
      outdent: options.outdent ?? false,
      tableEnabled: options.tableEnabled ?? true,
    },
  ]);
  return result?.operation ?? null;
}

export function runTableCellNavigationRules(
  snapshot: EditorContextSnapshot,
  options: TabRuleOptions = {},
): EditOperation | null {
  const result = runMarkdownTransactions(snapshot, [
    {
      kind: "table_cell_navigation",
      markdownAutoformat: options.markdownAutoformat ?? true,
      outdent: options.outdent ?? false,
      tableEnabled: options.tableEnabled ?? true,
    },
  ]);
  return result?.operation ?? null;
}

export function runTableHeaderDeleteColumnRule(
  snapshot: EditorContextSnapshot,
): EditOperation | null {
  const result = runMarkdownTransactions(snapshot, [
    {
      kind: "table_header_delete_column",
    },
  ]);
  return result?.operation ?? null;
}

export function runTablePipeInsertColumnRule(
  snapshot: EditorContextSnapshot,
): EditOperation | null {
  const result = runMarkdownTransactions(snapshot, [
    {
      kind: "table_pipe_insert_column",
    },
  ]);
  return result?.operation ?? null;
}

export function runTableBoundaryEditRules(
  snapshot: EditorContextSnapshot,
  options: TableBoundaryEditOptions = {},
): EditOperation | null {
  const result = runMarkdownTransactions(snapshot, [
    {
      kind: "table_boundary_edit",
      markdownAutoformat: options.markdownAutoformat ?? true,
      backward: options.backward ?? true,
      structuralMerge: options.structuralMerge ?? false,
      tableEnabled: options.tableEnabled ?? true,
    },
  ]);
  return result?.operation ?? null;
}

export function rewriteLineWithChecklistToggleSuffix(lineText: string): string | null {
  if (!ensureWasmReadyNonBlocking()) return null;
  return wasm_rewrite_line_with_checklist_toggle_suffix(lineText) ?? null;
}

export type ListKind = "checklist" | "unordered" | "ordered";

export function convertLineToList(
  line: string,
  kind: ListKind,
  orderedIndex: number,
): { text: string; changed: boolean } {
  if (!ensureWasmReadyNonBlocking()) return { text: line, changed: false };
  const result = wasm_convert_line_to_list(line, kind, orderedIndex);
  if (result === null || result === undefined) return { text: line, changed: false };
  return { text: result, changed: result !== line };
}

export function formatMarkdown(text: string): string {
  if (!ensureWasmReadyNonBlocking()) return text;
  return wasm_format_markdown(text);
}

export function formatTableLines(lines: readonly string[]): string[] {
  if (lines.length === 0) return [];
  if (!ensureWasmReadyNonBlocking()) return [...lines];
  const raw = wasm_format_table_lines([...lines]) as unknown;
  if (!Array.isArray(raw)) return [...lines];
  return raw.map((line) => (typeof line === "string" ? line : String(line)));
}

export function normalizeCommand(rawInput: string): string {
  if (!ensureWasmReadyNonBlocking()) return "";
  return wasm_normalize_command(rawInput);
}

export function listCommandSuggestionsFromWasm(
  mode: CommandMode,
  rawInput: string,
): CommandSuggestion[] {
  if (!ensureWasmReadyNonBlocking()) return [];
  const raw = wasm_list_command_suggestions(mode, rawInput) as
    | CommandSuggestion[]
    | null
    | undefined;
  return Array.isArray(raw) ? raw : [];
}

export function resolveCommandFromWasm(mode: CommandMode, rawInput: string): string | null {
  if (!ensureWasmReadyNonBlocking()) return null;
  return wasm_resolve_command(mode, rawInput) ?? null;
}

export function classifyCommandDispatchFromWasm(
  mode: CommandMode,
  rawInput: string,
): CommandDispatchKind | null {
  if (!ensureWasmReadyNonBlocking()) return null;
  const raw = wasm_classify_command_dispatch(mode, rawInput);
  if (
    raw === "core" ||
    raw === "host_date" ||
    raw === "host_notify" ||
    raw === "host_notify_delete" ||
    raw === "host_write" ||
    raw === "host_module" ||
    raw === "host_fold" ||
    raw === "host_clip_watch" ||
    raw === "host_note_security" ||
    raw === "quit"
  ) {
    return raw;
  }
  return null;
}

export function planHostCommandFromWasm(
  mode: CommandMode,
  rawInput: string,
): HostCommandPlan | null {
  if (!ensureWasmReadyNonBlocking()) return null;
  const raw = wasm_plan_host_command(mode, rawInput) as HostCommandPlan | null | undefined;
  return raw ?? null;
}

export type NoteSecurityAction =
  | "lock"
  | "unlock"
  | "encrypt"
  | "decrypt"
  | "unprotect";

export interface ParsedNoteSecurityCommand {
  action: NoteSecurityAction;
  password: string;
}

export interface ModuleStateSnapshot {
  math: boolean;
  table: boolean;
  variables: boolean;
  style: boolean;
}

export interface ModuleCommandPlan {
  changed: boolean;
  next: ModuleStateSnapshot;
  message: string;
}

export function parseNoteSecurityCommand(
  rawInput: string,
): ParsedNoteSecurityCommand | null {
  if (!ensureWasmReadyNonBlocking()) return null;
  const raw = wasm_parse_note_security_command(rawInput) as ParsedNoteSecurityCommand | null | undefined;
  return raw ?? null;
}

export function planModuleCommandFromWasm(
  mode: CommandMode,
  rawInput: string,
  current: ModuleStateSnapshot,
): ModuleCommandPlan | null {
  if (!ensureWasmReadyNonBlocking()) return null;
  const raw = wasm_plan_module_command(
    mode,
    rawInput,
    current.math,
    current.table,
    current.variables,
    current.style,
  ) as ModuleCommandPlan | null | undefined;
  return raw ?? null;
}

export function tryExecuteVimSubstituteFromWasm(
  snapshot: EditorContextSnapshot,
  rawInput: string,
  mode: CommandMode,
): VimSubstituteExecutionResult | null {
  if (!ensureWasmReadyNonBlocking()) return null;
  const text = snapshot.text;
  const [anchor, head] = batchUtf16ToUtf8(text, [
    snapshot.selection.anchor,
    snapshot.selection.head,
  ]);
  const raw = wasm_try_execute_vim_substitute(
    text,
    anchor!,
    head!,
    rawInput,
    mode,
  ) as unknown;
  return decodeVimSubstituteExecutionResult(raw, text);
}

export function executeMathCommandFromWasm(
  snapshot: EditorContextSnapshot,
  rawInput: string,
  mode: CommandMode,
): WasmCommandExecutionResult | null {
  if (!ensureWasmReadyNonBlocking()) return null;
  const [text, anchor, head] = snapshotToArgs(snapshot);
  const raw = wasm_execute_math_command(
    text,
    anchor!,
    head!,
    rawInput,
    mode,
  ) as unknown;
  return decodeCommandExecutionResult(raw, text);
}

export function executeVimActionFromWasm(
  snapshot: EditorContextSnapshot,
  intent: VimIntent,
  count: number,
  register: VimRegisterValue | null,
): VimActionExecutionResult | null {
  if (!ensureWasmReadyNonBlocking()) return null;
  const text = snapshot.text;
  const [anchor, head] = batchUtf16ToUtf8(text, [
    snapshot.selection.anchor,
    snapshot.selection.head,
  ]);
  const raw = wasm_execute_vim_action(
    text,
    anchor!,
    head!,
    intent,
    count,
    register?.text ?? "",
    register?.mode ?? "",
  ) as unknown;
  return decodeVimActionExecutionResult(raw, text);
}

export function sanitizeCommandHistoryCommand(rawCommand: string): string {
  if (!ensureWasmReadyNonBlocking()) {
    const trimmed = rawCommand.trim();
    const base = trimmed.replace(/^:/, "");
    const tokens = base.split(/\s+/).filter((token) => token.length > 0);
    if ((tokens[0] ?? "").toLowerCase() === "note" && tokens.length >= 3) {
      return `${tokens[0]} ${tokens[1]}`;
    }
    if (tokens.length >= 2) {
      return tokens[0] ?? base;
    }
    return base;
  }
  return wasm_command_history_sanitize(rawCommand);
}

function asCommandHistoryStep(value: unknown): CommandHistoryStepPayload | null {
  if (typeof value !== "object" || value === null) return null;
  const raw = value as Partial<CommandHistoryStepPayload>;
  if (typeof raw.index !== "number" || typeof raw.command !== "string") return null;
  return { index: raw.index, command: raw.command };
}

export function rememberCommandHistory(
  history: readonly string[],
  rawCommand: string,
  maxEntries: number,
): string[] {
  if (!ensureWasmReadyNonBlocking()) {
    const command = sanitizeCommandHistoryCommand(rawCommand);
    if (!command) return [...history];
    const next = [...history];
    const existing = next.lastIndexOf(command);
    if (existing >= 0) next.splice(existing, 1);
    next.push(command);
    if (next.length > maxEntries) {
      next.splice(0, next.length - maxEntries);
    }
    return next;
  }
  const raw = wasm_command_history_remember(
    [...history],
    rawCommand,
    maxEntries,
  ) as unknown;
  return Array.isArray(raw) ? raw.filter((entry): entry is string => typeof entry === "string") : [...history];
}

export function cycleCommandHistoryPrev(
  history: readonly string[],
  currentIndex: number | null,
): CommandHistoryStepPayload | null {
  if (!ensureWasmReadyNonBlocking()) {
    if (history.length === 0) return null;
    const nextIndex = currentIndex === null
      ? history.length - 1
      : (currentIndex + history.length - 1) % history.length;
    return { index: nextIndex, command: history[nextIndex] ?? "" };
  }
  const raw = wasm_command_history_prev(
    [...history],
    currentIndex === null ? -1 : currentIndex,
  ) as unknown;
  return asCommandHistoryStep(raw);
}

export function cycleCommandHistoryNext(
  history: readonly string[],
  currentIndex: number | null,
): CommandHistoryStepPayload | null {
  if (!ensureWasmReadyNonBlocking()) {
    if (history.length === 0) return null;
    const nextIndex = currentIndex === null ? 0 : (currentIndex + 1) % history.length;
    return { index: nextIndex, command: history[nextIndex] ?? "" };
  }
  const raw = wasm_command_history_next(
    [...history],
    currentIndex === null ? -1 : currentIndex,
  ) as unknown;
  return asCommandHistoryStep(raw);
}

const DEFAULT_MARKDOWN_LINE_INFO: MarkdownLineInfo = {
  headingLevel: null,
  headingMarkerEnd: null,
  quoteMarkerEnd: null,
  listMarkerEnd: null,
  checklistMarkerStart: null,
  checklistMarkerEnd: null,
  checklistContentStart: null,
  checklistChecked: false,
  isHorizontalRule: false,
  isCodeFence: false,
};

function asMarkdownLineInfo(value: unknown): MarkdownLineInfo | null {
  if (typeof value !== "object" || value === null) return null;
  const raw = value as Partial<MarkdownLineInfo>;
  return {
    headingLevel: typeof raw.headingLevel === "number" ? raw.headingLevel : null,
    headingMarkerEnd: typeof raw.headingMarkerEnd === "number" ? raw.headingMarkerEnd : null,
    quoteMarkerEnd: typeof raw.quoteMarkerEnd === "number" ? raw.quoteMarkerEnd : null,
    listMarkerEnd: typeof raw.listMarkerEnd === "number" ? raw.listMarkerEnd : null,
    checklistMarkerStart:
      typeof raw.checklistMarkerStart === "number" ? raw.checklistMarkerStart : null,
    checklistMarkerEnd: typeof raw.checklistMarkerEnd === "number" ? raw.checklistMarkerEnd : null,
    checklistContentStart:
      typeof raw.checklistContentStart === "number" ? raw.checklistContentStart : null,
    checklistChecked: raw.checklistChecked === true,
    isHorizontalRule: raw.isHorizontalRule === true,
    isCodeFence: raw.isCodeFence === true,
  };
}

function asMarkdownInlineTokens(value: unknown): MarkdownInlineToken[] {
  return Array.isArray(value) ? (value as MarkdownInlineToken[]) : [];
}

function asMarkdownCodeTokens(value: unknown): MarkdownCodeToken[] {
  return Array.isArray(value) ? (value as MarkdownCodeToken[]) : [];
}

function asMarkdownInlineMarkerComponentRanges(
  value: unknown,
): MarkdownInlineMarkerComponentRange[] {
  return Array.isArray(value) ? (value as MarkdownInlineMarkerComponentRange[]) : [];
}

function asMarkdownFoldRanges(value: unknown): MarkdownFoldRange[] {
  return Array.isArray(value) ? (value as MarkdownFoldRange[]) : [];
}

function singleLinePayloadMetrics(lineText: string): EditorProfilerMetrics {
  const len = lineText.length;
  return {
    lineCount: 1,
    totalChars: len,
    maxLineLength: len,
  };
}

function lineArrayPayloadMetrics(lines: readonly string[]): EditorProfilerMetrics {
  let totalChars = 0;
  let maxLineLength = 0;
  for (const line of lines) {
    const len = line.length;
    totalChars += len;
    if (len > maxLineLength) maxLineLength = len;
  }
  return {
    lineCount: lines.length,
    totalChars,
    maxLineLength,
  };
}

export function markdownClassifyLine(lineText: string): MarkdownLineInfo {
  if (!ensureWasmReadyNonBlocking()) return DEFAULT_MARKDOWN_LINE_INFO;
  const value = wasm_markdown_classify_line(lineText) as unknown;
  return asMarkdownLineInfo(value) ?? DEFAULT_MARKDOWN_LINE_INFO;
}

export function markdownFindInlineTokens(lineText: string): MarkdownInlineToken[] {
  if (!ensureWasmReadyNonBlocking()) return [];
  return asMarkdownInlineTokens(wasm_markdown_find_inline_tokens(lineText));
}

export function markdownInlineMarkerComponentRanges(
  lineText: string,
): MarkdownInlineMarkerComponentRange[] {
  if (!ensureWasmReadyNonBlocking()) return [];
  const profiling = isEditorProfilerEnabled();
  const startedAt = profiling ? editorProfilerNowMs() : 0;
  const result = asMarkdownInlineMarkerComponentRanges(
    wasm_markdown_inline_marker_component_ranges(lineText),
  );
  if (profiling) {
    recordEditorProfilerSample(
      "wasm.markdownInlineMarkerComponentRanges",
      editorProfilerNowMs() - startedAt,
      {
        reason: "sync",
        metrics: singleLinePayloadMetrics(lineText),
      },
    );
  }
  return result;
}

export function markdownTokenizeCodeLine(
  lineText: string,
  lang: string | null,
): MarkdownCodeToken[] {
  if (!ensureWasmReadyNonBlocking()) return [];
  return asMarkdownCodeTokens(wasm_markdown_tokenize_code_line(lineText, lang ?? undefined));
}

export function markdownIsCodeFence(lineText: string): boolean {
  if (!ensureWasmReadyNonBlocking()) return false;
  return wasm_markdown_is_code_fence(lineText);
}

export function markdownParseFenceLanguage(lineText: string): string | null {
  if (!ensureWasmReadyNonBlocking()) return null;
  return wasm_markdown_parse_fence_language(lineText) ?? null;
}

export function markdownAnalyzeLines(
  lines: readonly string[],
  start: { inCodeBlock: boolean; codeFenceLang: string | null },
): MarkdownAnalyzeResult {
  const profiling = isEditorProfilerEnabled();
  const startedAt = profiling ? editorProfilerNowMs() : 0;
  const metrics = profiling ? lineArrayPayloadMetrics(lines) : null;

  if (!ensureWasmReadyNonBlocking()) {
    const fallback: MarkdownAnalyzeResult = {
      lines: lines.map(() => ({
        info: DEFAULT_MARKDOWN_LINE_INFO,
        inCodeBlock: start.inCodeBlock,
        codeFenceLang: start.codeFenceLang,
        inlineTokens: [],
        codeTokens: [],
      })),
      finalInCodeBlock: start.inCodeBlock,
      finalCodeFenceLang: start.codeFenceLang,
    };
    if (profiling) {
      recordEditorProfilerSample(
        "wasm.markdownAnalyzeLines",
        editorProfilerNowMs() - startedAt,
        {
          reason: "sync_wasm_not_ready",
          metrics,
        },
      );
    }
    return fallback;
  }

  const raw = wasm_markdown_analyze_lines(
    [...lines],
    start.inCodeBlock,
    start.codeFenceLang ?? undefined,
  ) as unknown;

  if (typeof raw !== "object" || raw === null || !Array.isArray((raw as MarkdownAnalyzeResult).lines)) {
    const fallback: MarkdownAnalyzeResult = {
      lines: [],
      finalInCodeBlock: start.inCodeBlock,
      finalCodeFenceLang: start.codeFenceLang,
    };
    if (profiling) {
      recordEditorProfilerSample(
        "wasm.markdownAnalyzeLines",
        editorProfilerNowMs() - startedAt,
        {
          reason: "sync_invalid_payload",
          metrics,
        },
      );
    }
    return fallback;
  }

  const payload = raw as MarkdownAnalyzeResult;

  const outLines: MarkdownAnalyzedLine[] = payload.lines.map((line) => {
    const rawLine = line as Partial<MarkdownAnalyzedLine>;
    return {
      info: asMarkdownLineInfo(rawLine.info) ?? DEFAULT_MARKDOWN_LINE_INFO,
      inCodeBlock: rawLine.inCodeBlock === true,
      codeFenceLang: typeof rawLine.codeFenceLang === "string" ? rawLine.codeFenceLang : null,
      inlineTokens: asMarkdownInlineTokens(rawLine.inlineTokens),
      codeTokens: asMarkdownCodeTokens(rawLine.codeTokens),
    };
  });

  const out: MarkdownAnalyzeResult = {
    lines: outLines,
    finalInCodeBlock: payload.finalInCodeBlock ?? start.inCodeBlock,
    finalCodeFenceLang: payload.finalCodeFenceLang ?? null,
  };
  if (profiling) {
    recordEditorProfilerSample(
      "wasm.markdownAnalyzeLines",
      editorProfilerNowMs() - startedAt,
      {
        reason: "sync",
        metrics,
      },
    );
  }
  return out;
}

export function markdownBuildFoldRangesUi(
  lines: readonly string[],
): MarkdownFoldRange[] {
  if (!ensureWasmReadyNonBlocking()) return [];
  return asMarkdownFoldRanges(wasm_markdown_build_fold_ranges_ui([...lines]));
}

export function markdownFoldMapRangesUi(
  ranges: readonly MarkdownFoldRange[],
  edits: readonly MarkdownFoldLineEdit[],
  newLineCount: number,
): MarkdownFoldRange[] | null {
  if (!ensureWasmReadyNonBlocking()) return null;
  if (newLineCount < 1) return [];
  const raw = wasm_markdown_fold_map_ranges_ui([...ranges], [...edits], newLineCount) as unknown;
  return asMarkdownFoldRanges(raw);
}

export function markdownFoldEditsRequireRebuildUi(
  edits: readonly MarkdownFoldLineEdit[],
): boolean | null {
  if (!ensureWasmReadyNonBlocking()) return null;
  const raw = wasm_markdown_fold_edits_require_rebuild_ui([...edits]) as unknown;
  return typeof raw === "boolean" ? raw : null;
}

export function calcFindSingleTableCell(lineText: string): CalcSegment | null {
  if (!ensureWasmReadyNonBlocking()) return null;
  return (wasm_calc_find_single_table_cell(lineText) as CalcSegment | null | undefined) ?? null;
}

export function calcFindListSegment(lineText: string): CalcSegment | null {
  if (!ensureWasmReadyNonBlocking()) return null;
  return (wasm_calc_find_list_segment(lineText) as CalcSegment | null | undefined) ?? null;
}

export function calcFindSegment(lineText: string): CalcSegment | null {
  if (!ensureWasmReadyNonBlocking()) return null;
  return (wasm_calc_find_segment(lineText) as CalcSegment | null | undefined) ?? null;
}

export function calcFindTableFormulaSegment(lineText: string): TableFormulaSegment | null {
  if (!ensureWasmReadyNonBlocking()) return null;
  return (
    wasm_calc_find_table_formula_segment(lineText) as TableFormulaSegment | null | undefined
  ) ?? null;
}

export function calcFindTableFormulaSegments(lineText: string): TableFormulaSegment[] {
  if (!ensureWasmReadyNonBlocking()) return [];
  const profiling = isEditorProfilerEnabled();
  const startedAt = profiling ? editorProfilerNowMs() : 0;
  const result = (
    wasm_calc_find_table_formula_segments(lineText) as TableFormulaSegment[] | null | undefined
  ) ?? [];
  if (profiling) {
    recordEditorProfilerSample(
      "wasm.calcFindTableFormulaSegments",
      editorProfilerNowMs() - startedAt,
      {
        reason: "sync",
        metrics: singleLinePayloadMetrics(lineText),
      },
    );
  }
  return result;
}

export function calcLineForEvaluation(lineText: string): string {
  if (!ensureWasmReadyNonBlocking()) return lineText;
  return wasm_calc_line_for_eval(lineText);
}

export function calcIsBuiltinFormula(text: string): boolean {
  if (!ensureWasmReadyNonBlocking()) return false;
  return wasm_calc_is_builtin_formula(text);
}

export function calcBuiltinFormulaLabel(text: string): string | null {
  if (!ensureWasmReadyNonBlocking()) return null;
  return wasm_calc_builtin_formula_label(text) ?? null;
}

export function calcBuiltinFormulaLabels(text: string): string[] {
  if (!ensureWasmReadyNonBlocking()) return [];
  return (wasm_calc_builtin_formula_labels(text) as string[] | null | undefined) ?? [];
}

export function calcFormatFormulaDisplayValue(raw: string): string {
  if (!ensureWasmReadyNonBlocking()) return raw;
  return wasm_calc_format_formula_display_value(raw);
}

export function calcLineUsesAssignmentPrefix(lineText: string): boolean {
  if (!ensureWasmReadyNonBlocking()) return false;
  return wasm_calc_line_uses_assignment_prefix(lineText);
}

export function calcContainsVariableAssignment(lines: readonly string[]): boolean {
  if (!ensureWasmReadyNonBlocking()) return false;
  return wasm_calc_contains_variable_assignment([...lines]) ?? false;
}

export function calcContainsBuiltinFormula(lines: readonly string[]): boolean {
  if (!ensureWasmReadyNonBlocking()) return false;
  return wasm_calc_contains_builtin_formula([...lines]) ?? false;
}

export function calcDecideEvalScope(
  evalLines: readonly string[],
  prevChangedLines: readonly string[],
  hasPrev: boolean,
  variablesEnabled: boolean,
): CalcEvalScopeDecision {
  const fallback: CalcEvalScopeDecision = {
    touchesAnyAssignment: false,
    touchesBuiltinFormula: false,
    canUsePartial: false,
  };
  if (!ensureWasmReadyNonBlocking()) {
    return fallback;
  }
  const raw = wasm_calc_decide_eval_scope(
    [...evalLines],
    [...prevChangedLines],
    hasPrev,
    variablesEnabled,
  ) as CalcEvalScopeDecision | null | undefined;
  return raw ?? fallback;
}

export function calcDecideEvalWindow(
  lines: readonly string[],
  changedFrom: number,
  changedTo: number,
  prevChangedLines: readonly string[],
  hasPrev: boolean,
  variablesEnabled: boolean,
): CalcEvalWindowDecision {
  const fallback: CalcEvalWindowDecision = {
    evalFrom: 0,
    evalTo: lines.length,
    touchesAnyAssignment: false,
    touchesBuiltinFormula: false,
    canUsePartial: false,
  };
  if (!ensureWasmReadyNonBlocking()) {
    return fallback;
  }
  const raw = wasm_calc_decide_eval_window(
    [...lines],
    changedFrom,
    changedTo,
    [...prevChangedLines],
    hasPrev,
    variablesEnabled,
  ) as CalcEvalWindowDecision | null | undefined;
  return raw ?? fallback;
}

export function calcShouldScheduleEval(
  docLineCount: number,
  maxEvalLines: number,
  hasGlobalSyntax: boolean,
  touchesCalcExpression: boolean,
  visibleHasCalcSyntax: boolean,
): boolean {
  if (!ensureWasmReadyNonBlocking()) return false;
  return wasm_calc_should_schedule_eval(
    docLineCount,
    maxEvalLines,
    hasGlobalSyntax,
    touchesCalcExpression,
    visibleHasCalcSyntax,
  );
}

export function calcPlanIncremental(
  prevLines: string[],
  prevResults: ReadonlyMap<number, string>,
  nextLines: string[],
): IncrementalCalcPlan {
  const fallback: IncrementalCalcPlan = {
    baseResults: new Map(),
    evalFrom: 0,
    evalTo: nextLines.length,
    evalLines: [...nextLines],
  };
  if (!ensureWasmReadyNonBlocking()) {
    return fallback;
  }

  const prevDense: (string | null)[] = [];
  for (let i = 0; i < prevLines.length; i++) {
    prevDense.push(prevResults.get(i) ?? null);
  }

  const rawPlan = wasm_calc_plan_incremental(
    prevLines,
    prevDense,
    nextLines,
  ) as IncrementalCalcPlanPayload | null | undefined;

  if (!rawPlan) return fallback;

  const baseResults = new Map<number, string>();
  for (const entry of rawPlan.baseResults) {
    baseResults.set(entry.lineIdx, entry.result);
  }

  return {
    baseResults,
    evalFrom: rawPlan.evalFrom,
    evalTo: rawPlan.evalTo,
    evalLines: rawPlan.evalLines,
  };
}

export function calcComputeRefresh(
  markers: readonly CommitMarkerLoc[],
  lines: readonly string[],
  lineStarts: readonly number[],
  nextResults: ReadonlyMap<number, string>,
  selection: { from: number; to: number },
): CalcRefreshPlan {
  const fallback: CalcRefreshPlan = { changes: [], prune: [], syncedLines: [] };
  if (!ensureWasmReadyNonBlocking()) {
    return fallback;
  }

  const denseResults: (string | null)[] = [];
  for (let i = 0; i < lines.length; i++) {
    denseResults.push(nextResults.get(i) ?? null);
  }

  const rawPlan = wasm_calc_compute_refresh(
    [...markers],
    [...lines],
    [...lineStarts],
    denseResults,
    selection.from,
    selection.to,
  ) as CalcRefreshPlan | null | undefined;
  return rawPlan ?? fallback;
}

function decodeVimStep(raw: unknown): VimStep {
  if (typeof raw !== "object" || raw === null) {
    return { mode: "normal", actions: [], handled: false };
  }

  const payload = raw as Record<string, unknown>;
  const handled = payload.handled === true;
  const mode = asVimMode(payload.mode);

  const actions: VimAction[] = [];
  if (Array.isArray(payload.actions)) {
    for (const rawAction of payload.actions) {
      if (typeof rawAction !== "object" || rawAction === null) continue;
      const action = rawAction as Record<string, unknown>;
      const intent = asVimIntent(action.intent);
      if (!intent) continue;
      const rawCount = typeof action.count === "number" ? action.count : 1;
      actions.push({ intent, count: Math.max(1, rawCount) });
    }
  }

  return { mode, actions, handled };
}

export class VimSession {
  private session: WasmVimSession | null = null;
  private readonly initialMode: VimMode;

  constructor(initialMode: VimMode = "normal") {
    this.initialMode = initialMode;
  }

  step(key: VimKeyInput, context: VimContext = {}): VimStep | null {
    if (!ensureWasmReadyNonBlocking()) return null;
    if (!this.session) {
      this.session = new WasmVimSession(modeToId(this.initialMode));
    }

    const raw = this.session.step(
      key.kind,
      key.charCode ?? 0,
      context.has_search_matches ?? false,
      context.line_count ?? 0,
    );
    return decodeVimStep(raw);
  }
}

// Start compiling in the background as soon as this module is loaded.
void ensureWasmReady().catch(logInitError);
