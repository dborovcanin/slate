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

// Translate the TS EditorContextSnapshot (camelCase) to the JSON shape Rust expects (snake_case).
function toRustSnapshot(snapshot: EditorContextSnapshot): string {
  return JSON.stringify({
    text: snapshot.text,
    selection: snapshot.selection,
    changed_range: snapshot.changedRange,
  });
}

function parseOp(json: string | undefined): EditOperation | null {
  if (!json) return null;
  return JSON.parse(json) as EditOperation;
}

export function runDocChangeRules(
  snapshot: EditorContextSnapshot,
  options: TextRuleOptions = {},
): EditOperation | null {
  if (!ensureWasmReadyNonBlocking()) return null;
  return parseOp(
    wasm_run_doc_change_rules(toRustSnapshot(snapshot), options.markdownAutoformat ?? true),
  );
}

export function runEnterRules(
  snapshot: EditorContextSnapshot,
  options: TextRuleOptions = {},
): EditOperation | null {
  if (!ensureWasmReadyNonBlocking()) return null;
  return parseOp(
    wasm_run_enter_rules(toRustSnapshot(snapshot), options.markdownAutoformat ?? true),
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
