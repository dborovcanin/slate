type Align = "left" | "center" | "right" | "none";

const delimiterCellRe = /^:?-{3,}:?$/;
const tableRowRe = /^\s*\|.*\|\s*$/;

function splitTableCells(line: string): string[] {
  const trimmed = line.trim();
  const inner = trimmed.slice(1, -1);
  return inner.split("|").map((cell) => cell.trim());
}

function parseAlign(cell: string): Align {
  if (/^:-+:$/.test(cell)) return "center";
  if (/^:-+$/.test(cell)) return "left";
  if (/^-+:$/.test(cell)) return "right";
  return "none";
}

function minDelimiterLen(align: Align): number {
  if (align === "center") return 5;
  if (align === "left" || align === "right") return 4;
  return 3;
}

function isDelimiterRow(row: readonly string[]): boolean {
  return row.some((cell) => delimiterCellRe.test(cell)) && row.every((cell) => delimiterCellRe.test(cell) || cell.length === 0);
}

function normalizeDelimiterCellForWidth(cell: string, minWidth: number): string {
  const align = parseAlign(cell.trim());
  const width = Math.max(minWidth, minDelimiterLen(align));
  if (align === "left") return `:${"-".repeat(Math.max(3, width - 1))}`;
  if (align === "right") return `${"-".repeat(Math.max(3, width - 1))}:`;
  if (align === "center") return `:${"-".repeat(Math.max(3, width - 2))}:`;
  return "-".repeat(Math.max(3, width));
}

function normalizeDelimiterCell(cell: string): string {
  const trimmed = cell.trim();
  const align = parseAlign(trimmed);
  const dashCount = Math.max(
    3,
    [...trimmed].filter((char) => char === "-").length,
  );
  const minWidth =
    align === "center"
      ? dashCount + 2
      : align === "left" || align === "right"
        ? dashCount + 1
        : dashCount;
  return normalizeDelimiterCellForWidth(trimmed, minWidth);
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

  const hasDelimiter = normalizedRows.some((row) => isDelimiterRow(row));
  const outputRows =
    !hasDelimiter && normalizedRows.length >= 2
      ? [
          normalizedRows[0],
          new Array(columnCount).fill("---"),
          ...normalizedRows.slice(1),
        ]
      : normalizedRows;

  const normalized = outputRows.map((row) => {
    const delimiter = isDelimiterRow(row);
    return row.map((cell) =>
      delimiter
        ? normalizeDelimiterCell(cell.length === 0 ? "---" : cell)
        : cell.trim(),
    );
  });

  const widths = new Array(columnCount).fill(0);
  for (const row of normalized) {
    for (let col = 0; col < columnCount; col += 1) {
      const cell = row[col] ?? "";
      widths[col] = Math.max(widths[col], cell.length);
    }
  }

  return outputRows.map((row, rowIdx) => {
    const delimiter = isDelimiterRow(row);
    let out = "|";
    for (let col = 0; col < columnCount; col += 1) {
      const raw = (row[col] ?? "").trim();
      const content = delimiter
        ? normalizeDelimiterCellForWidth(raw.length === 0 ? "---" : raw, widths[col])
        : (normalized[rowIdx][col] ?? "");
      const padRight = Math.max(0, widths[col] - content.length) + 1;
      out += ` ${content}${" ".repeat(padRight)}|`;
    }
    return out;
  });
}
