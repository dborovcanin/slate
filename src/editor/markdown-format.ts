import { EditorView } from "@codemirror/view";
import { ensureWasmReady, formatMarkdown } from "./wasm.ts";

export function formatMarkdownText(input: string): string {
  if (input.length === 0) return input;
  // The WASM formatter doesn't emit a trailing newline, so preserve it.
  const hasTrailingNewline = input.endsWith("\n");
  const body = hasTrailingNewline ? input.slice(0, -1) : input;
  const result = formatMarkdown(body);
  return hasTrailingNewline ? `${result}\n` : result;
}

export async function formatMarkdownTextAsync(input: string): Promise<string> {
  if (input.length === 0) return input;
  await ensureWasmReady();
  return formatMarkdownText(input);
}

export function applyMarkdownFormat(view: EditorView): boolean {
  const source = view.state.doc.toString();
  const formatted = formatMarkdownText(source);
  if (formatted === source) return false;

  const main = view.state.selection.main;
  const anchor = Math.min(main.anchor, formatted.length);
  const head = Math.min(main.head, formatted.length);
  view.dispatch({
    changes: { from: 0, to: view.state.doc.length, insert: formatted },
    selection: { anchor, head },
    scrollIntoView: true,
  });
  return true;
}
