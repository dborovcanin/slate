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

// Initialize synchronously in Node.js (test env), asynchronously in the browser.
// In the browser, vite-plugin-wasm handles WASM bundling; top-level await is enabled
// by vite-plugin-top-level-await. In Node.js (npm test), fetch doesn't work for
// file:// URLs, so we read the binary directly with fs.readFileSync.
// eslint-disable-next-line @typescript-eslint/no-explicit-any
const _isNode = typeof (globalThis as any).process?.versions?.node === "string";
if (_isNode) {
  // Dynamic imports avoid bundling Node-only modules in the browser bundle.
  const { readFileSync } = await import("fs" as string);
  const { fileURLToPath } = await import("url" as string);
  const { resolve, dirname } = await import("path" as string);
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  const __filename = fileURLToPath((import.meta as any).url);
  const __dirname = dirname(__filename);
  const wasmPath = resolve(__dirname, "../../pkg/editor-core/editor_core_bg.wasm");
  const wasmBytes = readFileSync(wasmPath);
  initSync({ module: wasmBytes });
} else {
  await init();
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
  return parseOp(
    wasm_run_doc_change_rules(toRustSnapshot(snapshot), options.markdownAutoformat ?? true),
  );
}

export function runEnterRules(
  snapshot: EditorContextSnapshot,
  options: TextRuleOptions = {},
): EditOperation | null {
  return parseOp(
    wasm_run_enter_rules(toRustSnapshot(snapshot), options.markdownAutoformat ?? true),
  );
}

export function runTabRules(
  snapshot: EditorContextSnapshot,
  options: TabRuleOptions = {},
): EditOperation | null {
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
  return parseOp(
    wasm_run_table_cell_navigation_rules(
      toRustSnapshot(snapshot),
      options.markdownAutoformat ?? true,
      options.outdent ?? false,
    ),
  );
}

export function rewriteLineWithChecklistToggleSuffix(lineText: string): string | null {
  return wasm_rewrite_line_with_checklist_toggle_suffix(lineText) ?? null;
}

export function formatMarkdown(text: string): string {
  return wasm_format_markdown(text);
}
