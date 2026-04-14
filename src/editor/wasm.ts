import init, {
  WasmVimSession,
  initSync,
  wasm_calc_builtin_formula_label,
  wasm_calc_compute_refresh,
  wasm_calc_contains_builtin_formula,
  wasm_calc_contains_variable_assignment,
  wasm_calc_find_list_segment,
  wasm_calc_find_segment,
  wasm_calc_find_single_table_cell,
  wasm_calc_find_table_formula_segment,
  wasm_calc_format_formula_display_value,
  wasm_calc_is_builtin_formula,
  wasm_calc_line_for_eval,
  wasm_calc_line_uses_assignment_prefix,
  wasm_calc_plan_incremental,
  wasm_format_markdown,
  wasm_list_command_suggestions,
  wasm_normalize_command,
  wasm_rewrite_line_with_checklist_toggle_suffix,
  wasm_resolve_command,
  wasm_run_doc_change_rules,
  wasm_run_enter_rules,
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
}

export interface TabRuleOptions {
  markdownAutoformat?: boolean;
  outdent?: boolean;
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
  label: string;
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

interface CalcSegmentRaw {
  expr: string;
  from_col: number;
  to_col: number;
  from_byte: number;
  to_byte: number;
}

interface TableFormulaSegmentRaw {
  from_byte: number;
  to_byte: number;
  from_char: number;
  to_char: number;
  label: string;
}

interface CommitMarkerLocRaw {
  doc_pos: number;
  line_idx: number;
  offset_in_line: number;
  last_literal: string;
}

interface CalcRefreshChangeRaw {
  line_idx: number;
  from: number;
  to: number;
  insert: string;
  new_literal: string;
}

interface CalcRefreshPlanRaw {
  changes: CalcRefreshChangeRaw[];
  prune: number[];
  synced_lines: number[];
}

interface LineCalcResultRaw {
  line_idx: number;
  result: string;
}

interface IncrementalCalcPlanRaw {
  base_results: LineCalcResultRaw[];
  eval_from: number;
  eval_to: number;
  eval_lines: string[];
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
  return new TextEncoder().encode(text.slice(0, clamped)).length;
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

function parseJson<T>(raw: string | null | undefined): T | null {
  if (!raw) return null;
  try {
    return JSON.parse(raw) as T;
  } catch {
    return null;
  }
}

function mapSegment(raw: CalcSegmentRaw | null): CalcSegment | null {
  if (!raw) return null;
  return {
    expr: raw.expr,
    fromCol: raw.from_col,
    toCol: raw.to_col,
    fromByte: raw.from_byte,
    toByte: raw.to_byte,
  };
}

function mapTableFormula(raw: TableFormulaSegmentRaw | null): TableFormulaSegment | null {
  if (!raw) return null;
  return {
    fromByte: raw.from_byte,
    toByte: raw.to_byte,
    fromChar: raw.from_char,
    toChar: raw.to_char,
    label: raw.label,
  };
}

export function runDocChangeRules(
  snapshot: EditorContextSnapshot,
  options: TextRuleOptions = {},
): EditOperation | null {
  if (!ensureWasmReadyNonBlocking()) return null;
  return parseOp(
    wasm_run_doc_change_rules(toRustSnapshot(snapshot), options.markdownAutoformat ?? true),
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

export function rewriteLineWithChecklistToggleSuffix(lineText: string): string | null {
  if (!ensureWasmReadyNonBlocking()) return null;
  return wasm_rewrite_line_with_checklist_toggle_suffix(lineText) ?? null;
}

export function formatMarkdown(text: string): string {
  if (!ensureWasmReadyNonBlocking()) return text;
  return wasm_format_markdown(text);
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

export function calcFindSingleTableCell(lineText: string): CalcSegment | null {
  if (!ensureWasmReadyNonBlocking()) return null;
  return mapSegment(parseJson<CalcSegmentRaw>(wasm_calc_find_single_table_cell(lineText)));
}

export function calcFindListSegment(lineText: string): CalcSegment | null {
  if (!ensureWasmReadyNonBlocking()) return null;
  return mapSegment(parseJson<CalcSegmentRaw>(wasm_calc_find_list_segment(lineText)));
}

export function calcFindSegment(lineText: string): CalcSegment | null {
  if (!ensureWasmReadyNonBlocking()) return null;
  return mapSegment(parseJson<CalcSegmentRaw>(wasm_calc_find_segment(lineText)));
}

export function calcFindTableFormulaSegment(lineText: string): TableFormulaSegment | null {
  if (!ensureWasmReadyNonBlocking()) return null;
  return mapTableFormula(
    parseJson<TableFormulaSegmentRaw>(wasm_calc_find_table_formula_segment(lineText)),
  );
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
  return wasm_calc_contains_variable_assignment(JSON.stringify(lines)) ?? false;
}

export function calcContainsBuiltinFormula(lines: readonly string[]): boolean {
  if (!ensureWasmReadyNonBlocking()) return false;
  return wasm_calc_contains_builtin_formula(JSON.stringify(lines)) ?? false;
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

  const rawPlan = parseJson<IncrementalCalcPlanRaw>(
    wasm_calc_plan_incremental(
      JSON.stringify(prevLines),
      JSON.stringify(prevDense),
      JSON.stringify(nextLines),
    ),
  );

  if (!rawPlan) {
    return {
      baseResults: new Map(),
      evalFrom: 0,
      evalTo: nextLines.length,
      evalLines: [...nextLines],
    };
  }

  const baseResults = new Map<number, string>();
  for (const entry of rawPlan.base_results) {
    baseResults.set(entry.line_idx, entry.result);
  }

  return {
    baseResults,
    evalFrom: rawPlan.eval_from,
    evalTo: rawPlan.eval_to,
    evalLines: rawPlan.eval_lines,
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

  const markersRaw: CommitMarkerLocRaw[] = markers.map((marker) => ({
    doc_pos: marker.docPos,
    line_idx: marker.lineIdx,
    offset_in_line: marker.offsetInLine,
    last_literal: marker.lastLiteral,
  }));

  const denseResults: (string | null)[] = [];
  for (let i = 0; i < lines.length; i++) {
    denseResults.push(nextResults.get(i) ?? null);
  }

  const rawPlan = parseJson<CalcRefreshPlanRaw>(
    wasm_calc_compute_refresh(
      JSON.stringify(markersRaw),
      JSON.stringify(lines),
      JSON.stringify(lineStarts),
      JSON.stringify(denseResults),
      selection.from,
      selection.to,
    ),
  );

  if (!rawPlan) {
    return { changes: [], prune: [], syncedLines: [] };
  }

  return {
    changes: rawPlan.changes.map((change) => ({
      lineIdx: change.line_idx,
      from: change.from,
      to: change.to,
      insert: change.insert,
      newLiteral: change.new_literal,
    })),
    prune: rawPlan.prune,
    syncedLines: rawPlan.synced_lines,
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
