import { EditorView } from "@codemirror/view";
import { formatTableLines } from "./core/markdown-table.ts";

const tableRowRe = /^\s*\|.*\|\s*$/;
const headingNoSpaceRe = /^(#{1,6})([^\s#])(.*)$/;
const unorderedListRe = /^(\s*)[*+]\s+(.*)$/;
const orderedListRe = /^(\s*)(\d+)\.\s*(.*)$/;

function normalizeMarkdownLine(line: string): string {
  let out = line.replace(/\s+$/g, "");
  out = out.replace(headingNoSpaceRe, "$1 $2$3");
  out = out.replace(unorderedListRe, "$1- $2");
  out = out.replace(orderedListRe, "$1$2. $3");
  return out;
}

export function formatMarkdownText(input: string): string {
  if (input.length === 0) return input;

  const hasTrailingNewline = input.endsWith("\n");
  const sourceLines = input.split("\n");
  if (hasTrailingNewline) sourceLines.pop();

  const out: string[] = [];
  for (let i = 0; i < sourceLines.length; i++) {
    const line = sourceLines[i] ?? "";
    if (!tableRowRe.test(line)) {
      out.push(normalizeMarkdownLine(line));
      continue;
    }

    const block: string[] = [line];
    let j = i + 1;
    while (j < sourceLines.length && tableRowRe.test(sourceLines[j] ?? "")) {
      block.push(sourceLines[j] ?? "");
      j++;
    }
    out.push(...formatTableLines(block));
    i = j - 1;
  }

  const result = out.join("\n");
  return hasTrailingNewline ? `${result}\n` : result;
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
