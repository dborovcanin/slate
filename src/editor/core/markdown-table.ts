import { formatTableLines as formatTableLinesWasm } from "../wasm.ts";

const tableRowRe = /^\s*\|.*\|\s*$/;

export function isTableRow(text: string): boolean {
  return tableRowRe.test(text);
}

export function formatTableLines(lines: string[]): string[] {
  return formatTableLinesWasm(lines);
}
