import init, {
  initSync,
  wasm_format_markdown,
  wasm_list_command_suggestions,
  wasm_normalize_command,
  wasm_rewrite_line_with_checklist_toggle_suffix,
  wasm_resolve_command,
  wasm_run_doc_change_rules,
  wasm_run_enter_rules,
  wasm_run_tab_rules,
  wasm_run_table_cell_navigation_rules,
  wasm_vim_step,
} from "../../pkg/editor-core/editor_core.js";
import type {
  CommandMode,
  CommandSuggestion,
  EditOperation,
  EditorContextSnapshot,
} from "./core/types.ts";

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
export type VimPending =
  | "delete"
  | "yank"
  | "go"
  | "delete_inner"
  | "delete_around"
  | "yank_inner"
  | "yank_around";
export type VimIntent =
  | "move_left"
  | "move_right"
  | "move_up"
  | "move_down"
  | "move_word_forward"
  | "move_word_backward"
  | "move_line_start"
  | "move_line_end"
  | "move_doc_start"
  | "move_doc_end"
  | "move_to_line"
  | "enter_insert"
  | "append_insert"
  | "insert_line_start"
  | "append_line_end"
  | "open_line_below"
  | "open_line_above"
  | "enter_visual"
  | "enter_visual_line"
  | "exit_visual"
  | "delete_line"
  | "yank_line"
  | "delete_to_line_start"
  | "delete_to_line_end"
  | "yank_to_line_start"
  | "yank_to_line_end"
  | "delete_char"
  | "paste_after"
  | "undo"
  | "redo"
  | "open_command_bar"
  | "open_search"
  | "search_next"
  | "search_prev"
  | "delete_inside_word"
  | "delete_around_word"
  | "yank_inside_word"
  | "yank_around_word"
  | "delete_inside_pipe"
  | "delete_around_pipe"
  | "yank_inside_pipe"
  | "yank_around_pipe"
  | "swallow";

export interface VimState {
  mode: VimMode;
  count_buffer?: string;
  pending?: VimPending | null;
}

export interface VimContext {
  has_search_matches?: boolean;
  line_count?: number;
}

export interface VimAction {
  intent: VimIntent;
  count: number;
}

export interface VimStep {
  state: VimState;
  actions: VimAction[];
  handled: boolean;
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

// Translate the TS EditorContextSnapshot (camelCase) to the JSON shape Rust expects (snake_case).
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

export function vimStepFromWasm(
  state: VimState,
  keyToken: string,
  context: VimContext,
): VimStep | null {
  if (!ensureWasmReadyNonBlocking()) return null;
  const raw = wasm_vim_step(JSON.stringify(state), keyToken, JSON.stringify(context));
  if (!raw) return null;
  try {
    return JSON.parse(raw) as VimStep;
  } catch {
    return null;
  }
}

// Start compiling in the background as soon as this module is loaded.
void ensureWasmReady().catch(logInitError);
