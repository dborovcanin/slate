import init, {
  initSync,
  wasm_format_markdown,
  wasm_rewrite_line_with_checklist_toggle_suffix,
  wasm_run_doc_change_rules,
  wasm_run_enter_rules,
  wasm_run_tab_rules,
  wasm_run_table_cell_navigation_rules,
} from "../../pkg/editor-core/editor_core.js";
import type { EditOperation, EditorContextSnapshot } from "./core/types.ts";

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

// Start compiling in the background as soon as this module is loaded.
void ensureWasmReady().catch(logInitError);
