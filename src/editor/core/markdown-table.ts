type Align = "left" | "center" | "right" | "none";

const delimiterCellRe = /^:?-{3,}:?$/;
const tableRowRe = /^\s*\|.*\|\s*$/;

function repeat(char: string, count: number): string {
  return new Array(Math.max(0, count) + 1).join(char);
}

function splitTableCells(line: string): string[] {
  const trimmed = line.trim();
  const inner = trimmed.replace(/^\|/, "").replace(/\|$/, "");
  return inner.split("|").map((cell) => cell.trim());
}

function parseAlign(cell: string): Align {
  if (/^:-+:$/.test(cell)) return "center";
  if (/^:-+$/.test(cell)) return "left";
  if (/^-+:$/.test(cell)) return "right";
  return "none";
}

function delimiterForWidth(width: number, align: Align): string {
  const w = Math.max(3, width);
  if (align === "left") return `:${repeat("-", Math.max(3, w - 1))}`;
  if (align === "right") return `${repeat("-", Math.max(3, w - 1))}:`;
  if (align === "center") return `:${repeat("-", Math.max(3, w - 2))}:`;
  return repeat("-", w);
}

export function isTableRow(text: string): boolean {
  return tableRowRe.test(text);
}

export function formatTableLines(lines: string[]): string[] {
  if (lines.length === 0) return lines;

  const rows = lines.map(splitTableCells);
  const columnCount = rows.reduce((max, row) => Math.max(max, row.length), 0);
  const normalizedRows = rows.map((row) => {
    const cells = [...row];
    while (cells.length < columnCount) cells.push("");
    return cells;
  });

  const align: Align[] = new Array(columnCount).fill("none");
  for (const row of normalizedRows) {
    const isDelimiter = row.every((cell) => delimiterCellRe.test(cell) || cell.length === 0);
    if (!isDelimiter) continue;
    for (let i = 0; i < columnCount; i++) {
      if (delimiterCellRe.test(row[i])) {
        align[i] = parseAlign(row[i]);
      }
    }
    break;
  }

  const widths = new Array(columnCount).fill(3);
  for (const row of normalizedRows) {
    const isDelimiter = row.every((cell) => delimiterCellRe.test(cell) || cell.length === 0);
    if (isDelimiter) continue;
    for (let i = 0; i < columnCount; i++) {
      widths[i] = Math.max(widths[i], row[i].length);
    }
  }

  return normalizedRows.map((row) => {
    const isDelimiter = row.every((cell) => delimiterCellRe.test(cell) || cell.length === 0);
    const parts = row.map((cell, i) => {
      if (isDelimiter) {
        return delimiterForWidth(widths[i], align[i]);
      }
      return cell.padEnd(widths[i], " ");
    });
    return `| ${parts.join(" | ")} |`;
  });
}
