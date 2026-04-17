import init, {
  WasmVimSession,
  initSync,
  wasm_calc_builtin_formula_label,
  wasm_calc_builtin_formula_labels,
  wasm_calc_compute_refresh,
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
  wasm_format_markdown,
  wasm_format_table_lines,
  wasm_list_command_suggestions,
  wasm_markdown_analyze_lines,
  wasm_markdown_classify_line,
  wasm_markdown_find_inline_tokens,
  wasm_markdown_is_code_fence,
  wasm_markdown_parse_fence_language,
  wasm_markdown_tokenize_code_line,
  wasm_normalize_command,
  wasm_rewrite_line_with_checklist_toggle_suffix,
  wasm_resolve_command,
  wasm_run_doc_change_rules,
  wasm_run_enter_rules,
  wasm_run_table_boundary_edit_rules,
  wasm_run_table_header_delete_column_rule,
  wasm_run_table_pipe_insert_column_rule,
  wasm_run_tab_rules,
  wasm_run_table_cell_navigation_rules,
} from "../../pkg/editor-core/editor_core.js";
import type {
  CommandMode,
  CommandSuggestion,
  EditOperation,
  EditorContextSnapshot,
} from "./core/types.ts";
import { startupMark } from "../perf/startup.ts";

// Initialize non-blocking in the browser to avoid delaying first paint.
// In Node.js tests we can call ensureWasmReady() up front.
// eslint-disable-next-line @typescript-eslint/no-explicit-any
const _isNode = typeof (globalThis as any).process?.versions?.node === "string";
let _ready = false;
let _initPromise: Promise<void> | null = null;
let _initErrorLogged = false;
const UTF8_ENCODER = new TextEncoder();

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
}

export interface TabRuleOptions {
  markdownAutoformat?: boolean;
  outdent?: boolean;
}

export interface TableBoundaryEditOptions {
  markdownAutoformat?: boolean;
  backward?: boolean;
  structuralMerge?: boolean;
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

export interface VimAction {
  intent: number;
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

interface CalcRefreshPlanPayload {
  changes: CalcRefreshChange[];
  prune: number[];
  syncedLines: number[];
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

function modeFromId(modeId: number): VimMode {
  switch (modeId) {
    case VIM_MODE_ID.INSERT:
      return "insert";
    case VIM_MODE_ID.NORMAL:
      return "normal";
    case VIM_MODE_ID.VISUAL:
      return "visual";
    case VIM_MODE_ID.VISUAL_LINE:
      return "visual_line";
    default:
      return "normal";
  }
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

function utf16ToUtf8Offset(text: string, utf16Offset: number): number {
  const clamped = Math.max(0, Math.min(utf16Offset, text.length));
  if (clamped === 0) return 0;
  return UTF8_ENCODER.encode(text.slice(0, clamped)).length;
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
    mapped.selection = {
      anchor: utf8ToUtf16Offset(postText, operation.selection.anchor),
      head: rawHead === undefined ? undefined : utf8ToUtf16Offset(postText, rawHead),
    };
  }

  return mapped;
}

function toRustSnapshot(snapshot: EditorContextSnapshot): string {
  const text = snapshot.text;
  return JSON.stringify({
    text,
    selection: {
      anchor: utf16ToUtf8Offset(text, snapshot.selection.anchor),
      head: utf16ToUtf8Offset(text, snapshot.selection.head),
    },
    changed_range: snapshot.changedRange
      ? {
          from: utf16ToUtf8Offset(text, snapshot.changedRange.from),
          to: utf16ToUtf8Offset(text, snapshot.changedRange.to),
        }
      : undefined,
  });
}

function parseOp(json: string | undefined, sourceText: string): EditOperation | null {
  if (!json) return null;
  const operation = JSON.parse(json) as EditOperation;
  return mapOperationFromUtf8ToUtf16(sourceText, operation);
}

export function runDocChangeRules(
  snapshot: EditorContextSnapshot,
  options: TextRuleOptions = {},
): EditOperation | null {
  if (!ensureWasmReadyNonBlocking()) return null;
  return parseOp(
    wasm_run_doc_change_rules(
      toRustSnapshot(snapshot),
      options.markdownAutoformat ?? true,
      options.checklistAutoReorder ?? true,
    ),
    snapshot.text,
  );
}

export function runEnterRules(
  snapshot: EditorContextSnapshot,
  options: TextRuleOptions = {},
): EditOperation | null {
  if (!ensureWasmReadyNonBlocking()) return null;
  return parseOp(
    wasm_run_enter_rules(toRustSnapshot(snapshot), options.markdownAutoformat ?? true),
    snapshot.text,
  );
}

export function runTabRules(
  snapshot: EditorContextSnapshot,
  options: TabRuleOptions = {},
): EditOperation | null {
  if (!ensureWasmReadyNonBlocking()) return null;
  return parseOp(
    wasm_run_tab_rules(
      toRustSnapshot(snapshot),
      options.markdownAutoformat ?? true,
      options.outdent ?? false,
    ),
    snapshot.text,
  );
}

export function runTableCellNavigationRules(
  snapshot: EditorContextSnapshot,
  options: TabRuleOptions = {},
): EditOperation | null {
  if (!ensureWasmReadyNonBlocking()) return null;
  return parseOp(
    wasm_run_table_cell_navigation_rules(
      toRustSnapshot(snapshot),
      options.markdownAutoformat ?? true,
      options.outdent ?? false,
    ),
    snapshot.text,
  );
}

export function runTableHeaderDeleteColumnRule(
  snapshot: EditorContextSnapshot,
): EditOperation | null {
  if (!ensureWasmReadyNonBlocking()) return null;
  return parseOp(
    wasm_run_table_header_delete_column_rule(toRustSnapshot(snapshot)),
    snapshot.text,
  );
}

export function runTablePipeInsertColumnRule(
  snapshot: EditorContextSnapshot,
): EditOperation | null {
  if (!ensureWasmReadyNonBlocking()) return null;
  return parseOp(
    wasm_run_table_pipe_insert_column_rule(toRustSnapshot(snapshot)),
    snapshot.text,
  );
}

export function runTableBoundaryEditRules(
  snapshot: EditorContextSnapshot,
  options: TableBoundaryEditOptions = {},
): EditOperation | null {
  if (!ensureWasmReadyNonBlocking()) return null;
  return parseOp(
    wasm_run_table_boundary_edit_rules(
      toRustSnapshot(snapshot),
      options.markdownAutoformat ?? true,
      options.backward ?? true,
      options.structuralMerge ?? false,
    ),
    snapshot.text,
  );
}

export function rewriteLineWithChecklistToggleSuffix(lineText: string): string | null {
  if (!ensureWasmReadyNonBlocking()) return null;
  return wasm_rewrite_line_with_checklist_toggle_suffix(lineText) ?? null;
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
  if (!ensureWasmReadyNonBlocking()) {
    return rawInput.trim().replace(/^:/, "").toLowerCase();
  }
  return wasm_normalize_command(rawInput);
}

export function listCommandSuggestionsFromWasm(
  mode: CommandMode,
  rawInput: string,
): CommandSuggestion[] {
  if (!ensureWasmReadyNonBlocking()) return [];
  const raw = wasm_list_command_suggestions(mode, rawInput);
  if (!raw) return [];
  try {
    return JSON.parse(raw) as CommandSuggestion[];
  } catch {
    return [];
  }
}

export function resolveCommandFromWasm(mode: CommandMode, rawInput: string): string | null {
  if (!ensureWasmReadyNonBlocking()) return null;
  return wasm_resolve_command(mode, rawInput) ?? null;
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
  if (
    typeof raw.checklistChecked !== "boolean" ||
    typeof raw.isHorizontalRule !== "boolean" ||
    typeof raw.isCodeFence !== "boolean"
  ) {
    return null;
  }
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
    checklistChecked: raw.checklistChecked,
    isHorizontalRule: raw.isHorizontalRule,
    isCodeFence: raw.isCodeFence,
  };
}

function asMarkdownInlineTokens(value: unknown): MarkdownInlineToken[] {
  if (!Array.isArray(value)) return [];
  const out: MarkdownInlineToken[] = [];
  for (const token of value) {
    if (typeof token !== "object" || token === null) continue;
    const raw = token as Partial<MarkdownInlineToken>;
    if (
      typeof raw.from !== "number" ||
      typeof raw.to !== "number" ||
      typeof raw.type !== "string"
    ) {
      continue;
    }
    out.push({
      from: raw.from,
      to: raw.to,
      type: raw.type as MarkdownInlineTokenType,
    });
  }
  return out;
}

function asMarkdownCodeTokens(value: unknown): MarkdownCodeToken[] {
  if (!Array.isArray(value)) return [];
  const out: MarkdownCodeToken[] = [];
  for (const token of value) {
    if (typeof token !== "object" || token === null) continue;
    const raw = token as Partial<MarkdownCodeToken>;
    if (
      typeof raw.from !== "number" ||
      typeof raw.to !== "number" ||
      typeof raw.type !== "string"
    ) {
      continue;
    }
    out.push({
      from: raw.from,
      to: raw.to,
      type: raw.type as MarkdownCodeTokenType,
    });
  }
  return out;
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
  if (!ensureWasmReadyNonBlocking()) {
    return {
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
  }

  const raw = wasm_markdown_analyze_lines(
    [...lines],
    start.inCodeBlock,
    start.codeFenceLang ?? undefined,
  ) as unknown;

  if (typeof raw !== "object" || raw === null) {
    return {
      lines: [],
      finalInCodeBlock: start.inCodeBlock,
      finalCodeFenceLang: start.codeFenceLang,
    };
  }

  const payload = raw as Partial<MarkdownAnalyzeResult>;
  const outLines: MarkdownAnalyzedLine[] = [];
  if (Array.isArray(payload.lines)) {
    for (const line of payload.lines as unknown[]) {
      if (typeof line !== "object" || line === null) continue;
      const rawLine = line as Partial<MarkdownAnalyzedLine>;
      const info = asMarkdownLineInfo(rawLine.info);
      if (!info || typeof rawLine.inCodeBlock !== "boolean") continue;
      outLines.push({
        info,
        inCodeBlock: rawLine.inCodeBlock,
        codeFenceLang:
          typeof rawLine.codeFenceLang === "string" ? rawLine.codeFenceLang : null,
        inlineTokens: asMarkdownInlineTokens(rawLine.inlineTokens),
        codeTokens: asMarkdownCodeTokens(rawLine.codeTokens),
      });
    }
  }

  return {
    lines: outLines,
    finalInCodeBlock:
      typeof payload.finalInCodeBlock === "boolean"
        ? payload.finalInCodeBlock
        : start.inCodeBlock,
    finalCodeFenceLang:
      typeof payload.finalCodeFenceLang === "string" ? payload.finalCodeFenceLang : null,
  };
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
  return (
    wasm_calc_find_table_formula_segments(lineText) as TableFormulaSegment[] | null | undefined
  ) ?? [];
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

export function calcPlanIncremental(
  prevLines: string[],
  prevResults: ReadonlyMap<number, string>,
  nextLines: string[],
): IncrementalCalcPlan {
  if (!ensureWasmReadyNonBlocking()) {
    return {
      baseResults: new Map(),
      evalFrom: 0,
      evalTo: nextLines.length,
      evalLines: [...nextLines],
    };
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

  if (
    !rawPlan ||
    !Array.isArray(rawPlan.baseResults) ||
    !Array.isArray(rawPlan.evalLines) ||
    typeof rawPlan.evalFrom !== "number" ||
    typeof rawPlan.evalTo !== "number"
  ) {
    return {
      baseResults: new Map(),
      evalFrom: 0,
      evalTo: nextLines.length,
      evalLines: [...nextLines],
    };
  }

  const baseResults = new Map<number, string>();
  for (const entry of rawPlan.baseResults) {
    if (typeof entry?.lineIdx !== "number" || typeof entry?.result !== "string") continue;
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
  if (!ensureWasmReadyNonBlocking()) {
    return { changes: [], prune: [], syncedLines: [] };
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
  ) as CalcRefreshPlanPayload | null | undefined;

  if (
    !rawPlan ||
    !Array.isArray(rawPlan.changes) ||
    !Array.isArray(rawPlan.prune) ||
    !Array.isArray(rawPlan.syncedLines)
  ) {
    return { changes: [], prune: [], syncedLines: [] };
  }

  return {
    changes: rawPlan.changes,
    prune: rawPlan.prune,
    syncedLines: rawPlan.syncedLines,
  };
}

function decodeVimStep(raw: Uint32Array): VimStep {
  const handled = raw[0] === 1;
  const mode = modeFromId(raw[1] ?? VIM_MODE_ID.NORMAL);
  const actionCount = raw[2] ?? 0;
  const actions: VimAction[] = [];

  for (let i = 0; i < actionCount; i++) {
    const base = 3 + i * 2;
    if (base + 1 >= raw.length) break;
    const intent = raw[base] ?? 42;
    const count = Math.max(1, raw[base + 1] ?? 1);
    actions.push({ intent, count });
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
