import { searchWeb, type WebSearchItem, type WebSearchResult } from "../api.ts";
import { createListOverlay, type ListOverlay } from "../overlays/overlay.ts";

export interface OpenWebSearchOptions {
  prefillQuery?: string | null;
  onInsertLink?: (linkMarkdown: string) => void;
}

let overlay: ListOverlay | null = null;
let activeQuery = "";
let currentResults: WebSearchItem[] = [];
let currentAnswer: string | null = null;
let currentSummary: string | null = null;
let isLoading = false;
let errorMessage: string | null = null;
let completedQuery: string | null = null;
let searchRequestId = 0;
let searchSessionId = 0;
let onInsertLinkCallback: ((linkMarkdown: string) => void) | null = null;

function invalidatePendingSearch() {
  searchRequestId += 1;
}

function executeSearch(query: string, sessionId: number, refreshOverlay: () => void) {
  if (sessionId !== searchSessionId) return;
  const trimmed = query.trim();
  if (trimmed.length === 0) {
    invalidatePendingSearch();
    currentResults = [];
    currentAnswer = null;
    currentSummary = null;
    errorMessage = null;
    completedQuery = null;
    isLoading = false;
    refreshOverlay();
    return;
  }

  const reqId = ++searchRequestId;
  isLoading = true;
  errorMessage = null;
  refreshOverlay();

  searchWeb(trimmed)
    .then((res: WebSearchResult) => {
      if (sessionId !== searchSessionId || reqId !== searchRequestId) return;
      currentResults = res.items || [];
      currentAnswer = res.answer || null;
      currentSummary = res.summary || null;
      isLoading = false;
      errorMessage = null;
      completedQuery = trimmed;
      refreshOverlay();
    })
    .catch((err: unknown) => {
      if (sessionId !== searchSessionId || reqId !== searchRequestId) return;
      currentResults = [];
      currentAnswer = null;
      currentSummary = null;
      isLoading = false;
      errorMessage = err instanceof Error ? err.message : String(err);
      completedQuery = trimmed;
      refreshOverlay();
    });
}

function renderWebSearchItem(item: WebSearchItem, selected: boolean): HTMLElement {
  const row = document.createElement("div");
  row.className = "switcher-item" + (selected ? " switcher-item--active" : "");
  row.setAttribute("role", "option");
  row.setAttribute("aria-selected", selected ? "true" : "false");

  const titleRow = document.createElement("div");
  titleRow.className = "web-search-result-heading";

  const title = document.createElement("span");
  title.className = "switcher-item-title";
  title.textContent = item.title;
  titleRow.appendChild(title);

  const url = document.createElement("span");
  url.className = "web-search-result-url";
  url.textContent = item.url;
  titleRow.appendChild(url);

  row.appendChild(titleRow);

  if (item.snippet && item.snippet.trim().length > 0) {
    const snippet = document.createElement("div");
    snippet.className = "switcher-item-snippet";
    snippet.textContent = item.snippet;
    row.appendChild(snippet);
  }

  return row;
}

export interface WebSearchTextDisplay {
  label: "Answer" | "Summary" | "Result text";
  text: string;
}

export function selectWebSearchText(
  answer: string | null,
  summary: string | null,
  results: WebSearchItem[],
  selectedItem?: WebSearchItem,
): WebSearchTextDisplay | null {
  const directAnswer = answer?.trim();
  if (directAnswer) return { label: "Answer", text: directAnswer };

  const selectedText = selectedItem?.snippet.trim();
  if (selectedText) return { label: "Result text", text: selectedText };

  const providerSummary = summary?.trim();
  if (providerSummary) return { label: "Summary", text: providerSummary };

  const fallbackText = results.find((item) => item.snippet.trim().length > 0)?.snippet.trim();
  return fallbackText ? { label: "Result text", text: fallbackText } : null;
}

function renderWebSearchText(selectedItem?: WebSearchItem): HTMLElement | null {
  const display = selectWebSearchText(
    currentAnswer,
    currentSummary,
    currentResults,
    selectedItem,
  );
  if (!display) return null;

  const block = document.createElement("div");
  block.className = "web-search-text";

  const label = document.createElement("div");
  label.className = "web-search-text-label";
  label.textContent = display.label;
  block.appendChild(label);

  const content = document.createElement("div");
  content.className = "web-search-text-content";
  content.textContent = display.text;
  block.appendChild(content);

  return block;
}

export function isWebSearchOpen(): boolean {
  return overlay?.isOpen() ?? false;
}

function closeWebSearchSession(sessionId: number) {
  if (sessionId !== searchSessionId) return;
  invalidatePendingSearch();
  overlay?.close();
  overlay = null;
  onInsertLinkCallback = null;
}

function openWebResult(url: string) {
  try {
    const parsed = new URL(url);
    if (parsed.protocol !== "http:" && parsed.protocol !== "https:") return;
    window.open(parsed.href, "_blank", "noopener,noreferrer");
  } catch {
    // The shared backend filters invalid URLs; keep the adapter defensive.
  }
}

export function openWebSearch(options?: OpenWebSearchOptions) {
  invalidatePendingSearch();
  searchSessionId += 1;
  const sessionId = searchSessionId;
  onInsertLinkCallback = options?.onInsertLink ?? null;
  const initialQuery = options?.prefillQuery?.trim() ?? "";

  if (overlay) {
    overlay.close();
    overlay = null;
  }

  currentResults = [];
  currentAnswer = null;
  currentSummary = null;
  isLoading = false;
  errorMessage = null;
  completedQuery = null;
  activeQuery = initialQuery;

  const refreshOverlay = () => {
    if (sessionId === searchSessionId) overlay?.refresh();
  };

  overlay = createListOverlay<WebSearchItem>({
    classPrefix: "switcher",
    placeholder: "Search the web...",
    initialQuery,
    backdrop: true,
    debounceMs: 0,
    getItems: (query: string) => {
      const trimmed = query.trim();
      if (trimmed !== activeQuery) {
        activeQuery = trimmed;
        invalidatePendingSearch();
        currentResults = [];
        currentAnswer = null;
        currentSummary = null;
        errorMessage = null;
        completedQuery = null;
        isLoading = false;
      }
      return currentResults;
    },
    renderItem: renderWebSearchItem,
    renderSupplement: renderWebSearchText,
    onSelect: (item: WebSearchItem) => {
      closeWebSearchSession(sessionId);
      openWebResult(item.url);
    },
    onClose: () => {
      closeWebSearchSession(sessionId);
    },
    onKeydown: (event: KeyboardEvent, state) => {
      if (event.key === "Enter" && event.shiftKey) {
        event.preventDefault();
        event.stopPropagation();
        const selected = state.items[state.selectedIndex];
        if (selected) {
          const insertLink = onInsertLinkCallback;
          closeWebSearchSession(sessionId);
          insertLink?.(selected.markdown_link);
        }
        return true;
      }
      if (event.key === "Enter" && !event.shiftKey) {
        if (currentResults.length === 0 && activeQuery.length > 0) {
          event.preventDefault();
          event.stopPropagation();
          if (!isLoading && completedQuery !== activeQuery) {
            executeSearch(activeQuery, sessionId, refreshOverlay);
          }
          return true;
        }
      }
      return false;
    },
    emptyMessage: () =>
      isLoading
        ? "Searching..."
        : errorMessage
          ? `Error: ${errorMessage}`
          : activeQuery.length === 0
            ? "Type a query and press Enter"
            : completedQuery === activeQuery
              ? currentAnswer || currentSummary
                ? "No link results"
                : "No web results"
              : "Press Enter to search",
  });

  overlay.open();

  if (initialQuery.length > 0) {
    executeSearch(initialQuery, sessionId, refreshOverlay);
  }
}
