import assert from "node:assert/strict";
import test from "node:test";
import type { WebSearchItem } from "../api.ts";
import { selectWebSearchText } from "./web-search.ts";

function result(title: string, snippet: string): WebSearchItem {
  return {
    title,
    url: `https://example.com/${encodeURIComponent(title)}`,
    snippet,
    markdown_link: `[${title}](https://example.com)`,
  };
}

test("web search text prioritizes direct answers", () => {
  assert.deepEqual(
    selectWebSearchText(
      "35 cm = 13.7795 inches",
      "A centimetre is a unit of length.",
      [result("Centimetre", "A unit of length.")],
    ),
    { label: "Answer", text: "35 cm = 13.7795 inches" },
  );
});

test("web search text follows the selected result snippet", () => {
  const results = [
    result("Novak", "A surname and given name."),
    result("Novak Djokovic", "A Serbian professional tennis player."),
  ];

  assert.deepEqual(selectWebSearchText(null, null, results, results[1]), {
    label: "Result text",
    text: "A Serbian professional tennis player.",
  });
});

test("web search text falls back to a provider summary or another useful snippet", () => {
  const results = [result("No text", ""), result("Useful", "Useful result text.")];

  assert.deepEqual(selectWebSearchText(null, "Provider summary.", results, results[0]), {
    label: "Summary",
    text: "Provider summary.",
  });
  assert.deepEqual(selectWebSearchText(null, null, results, results[0]), {
    label: "Result text",
    text: "Useful result text.",
  });
});
