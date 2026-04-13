export function escapeHtml(s: string): string {
  return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
}

export function highlightPositions(text: string, positions: number[]): HTMLElement {
  const span = document.createElement("span");
  if (positions.length === 0) {
    span.textContent = text;
    return span;
  }

  const posSet = new Set(positions);
  let i = 0;
  while (i < text.length) {
    if (posSet.has(i)) {
      const b = document.createElement("b");
      let j = i;
      while (j < text.length && posSet.has(j)) j++;
      b.textContent = text.slice(i, j);
      span.appendChild(b);
      i = j;
    } else {
      let j = i;
      while (j < text.length && !posSet.has(j)) j++;
      span.appendChild(document.createTextNode(text.slice(i, j)));
      i = j;
    }
  }
  return span;
}
