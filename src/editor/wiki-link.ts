import {
  completionStatus,
  startCompletion,
  type CompletionSource,
} from "@codemirror/autocomplete";
import {
  Prec,
  StateEffect,
  StateField,
  type Extension,
} from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { listNotesMeta, resolveWikiLink, resolveWikiLinkHeadings, type NoteSummary } from "../api.ts";
import { markdownWikiLinkAtCursor } from "./wasm.ts";

// Regex: match [[ followed by anything that isn't ] up to cursor position.
const WIKI_LINK_OPEN_RE = /\[\[[^\]]*$/;
const SHORT_ID_RE = /^[0-9A-Za-z]{8}$/;
const NOTE_LIST_CACHE_TTL_MS = 15_000;
const headingCache = new Map<string, string[]>();
let noteListCache: { expiresAt: number; notes: NoteSummary[] } | null = null;
let noteListInFlight: Promise<NoteSummary[]> | null = null;

type WikiLinkApi = {
  listNotesMeta(activeId?: string | null): Promise<NoteSummary[]>;
  resolveWikiLink(shortId: string): Promise<NoteSummary | null>;
  resolveWikiLinkHeadings(shortId: string): Promise<string[]>;
};

const defaultWikiLinkApi: WikiLinkApi = {
  listNotesMeta,
  resolveWikiLink,
  resolveWikiLinkHeadings,
};

async function loadWikiLinkNoteCandidates(api: WikiLinkApi): Promise<NoteSummary[]> {
  const now = Date.now();
  if (noteListCache && noteListCache.expiresAt > now) {
    return noteListCache.notes;
  }
  if (noteListInFlight) {
    return noteListInFlight;
  }
  noteListInFlight = api.listNotesMeta()
    .then((notes) => {
      noteListCache = {
        notes,
        expiresAt: Date.now() + NOTE_LIST_CACHE_TTL_MS,
      };
      return notes;
    })
    .finally(() => {
      noteListInFlight = null;
    });
  return noteListInFlight;
}

type WikiLinkCompletionContext =
  | { kind: "note"; noteQuery: string }
  | { kind: "heading"; shortId: string; headingQuery: string };

type PendingAutoHeadingPrompt = {
  from: number;
  shortId: string;
  opened: boolean;
};

const setPendingAutoHeadingPrompt = StateEffect.define<PendingAutoHeadingPrompt | null>();
const markPendingAutoHeadingPromptOpened = StateEffect.define<void>();
const clearPendingAutoHeadingPrompt = StateEffect.define<void>();

const pendingAutoHeadingPromptField = StateField.define<PendingAutoHeadingPrompt | null>({
  create: () => null,
  update: (value, tr) => {
    let next = value ? { ...value, from: tr.changes.mapPos(value.from) } : null;
    for (const effect of tr.effects) {
      if (effect.is(setPendingAutoHeadingPrompt)) {
        next = effect.value;
      } else if (effect.is(markPendingAutoHeadingPromptOpened) && next) {
        next = { ...next, opened: true };
      } else if (effect.is(clearPendingAutoHeadingPrompt)) {
        next = null;
      }
    }
    return next;
  },
});

function clearPendingAutoHeadingPromptIfPresent(view: EditorView) {
  const pending = view.state.field(pendingAutoHeadingPromptField, false);
  if (!pending) return;
  view.dispatch({ effects: clearPendingAutoHeadingPrompt.of() });
}

function cleanupPendingAutoHeadingPrompt(view: EditorView, pending: PendingAutoHeadingPrompt) {
  const line = view.state.doc.lineAt(pending.from);
  const lineOffset = pending.from - line.from;
  const prefix = `[[${pending.shortId}`;
  if (!line.text.startsWith(prefix, lineOffset)) {
    clearPendingAutoHeadingPromptIfPresent(view);
    return;
  }
  const hashOffset = lineOffset + prefix.length;
  if (hashOffset >= line.text.length || line.text[hashOffset] !== "#") {
    clearPendingAutoHeadingPromptIfPresent(view);
    return;
  }
  const closeOffset = line.text.indexOf("]]", hashOffset);
  if (closeOffset < 0) {
    clearPendingAutoHeadingPromptIfPresent(view);
    return;
  }
  const from = line.from + hashOffset;
  const to = line.from + closeOffset;
  if (from >= to) {
    clearPendingAutoHeadingPromptIfPresent(view);
    return;
  }
  view.dispatch({
    changes: { from, to, insert: "" },
    selection: { anchor: from },
    effects: clearPendingAutoHeadingPrompt.of(),
  });
}

const autoHeadingPromptLifecycle = EditorView.updateListener.of((update) => {
  const pending = update.state.field(pendingAutoHeadingPromptField, false);
  if (!pending) return;
  const status = completionStatus(update.state);
  if (!pending.opened) {
    if (status === "active" || status === "pending") {
      update.view.dispatch({ effects: markPendingAutoHeadingPromptOpened.of() });
      return;
    }
    // Ignore the insertion transaction; wait for completion state update.
    if (update.docChanged) return;
    cleanupPendingAutoHeadingPrompt(update.view, pending);
    return;
  }
  if (status === "active" || status === "pending") return;
  cleanupPendingAutoHeadingPrompt(update.view, pending);
});

function parseWikiLinkCompletionContext(raw: string): WikiLinkCompletionContext | null {
  if (raw.includes("]") || raw.includes("|")) return null;
  const hashIdx = raw.indexOf("#");
  if (hashIdx < 0) {
    return { kind: "note", noteQuery: raw.toLowerCase() };
  }
  const shortId = raw.slice(0, hashIdx);
  if (!SHORT_ID_RE.test(shortId)) return null;
  return {
    kind: "heading",
    shortId,
    headingQuery: raw.slice(hashIdx + 1).toLowerCase(),
  };
}

export function createWikiLinkCompletionSource(
  api: WikiLinkApi = defaultWikiLinkApi,
): CompletionSource {
  return async (context) => {
    const match = context.matchBefore(WIKI_LINK_OPEN_RE);
    if (!match) return null;

    const raw = match.text.slice(2);
    const parsed = parseWikiLinkCompletionContext(raw);
    if (!parsed) return null;

    if (parsed.kind === "note") {
      const notes = await loadWikiLinkNoteCandidates(api);
      const filtered = notes
        .filter((n) => {
          const title = n.title.toLowerCase();
          return parsed.noteQuery.length === 0 || title.includes(parsed.noteQuery);
        })
        .slice(0, 30);
      if (filtered.length === 0) return null;
      return {
        from: match.from,
        options: filtered.map((note) => {
          const shortId = note.id.slice(0, 8);
          const title = note.title || "Untitled";
          return {
            label: title,
            type: "wiki-link",
            apply: (view: EditorView, _completion: object, from: number, to: number) => {
              let actualTo = to;
              if (view.state.doc.sliceString(to, to + 2) === "]]") {
                actualTo = to + 2;
              }
              const insertText = `[[${shortId}#]]`;
              view.dispatch({
                changes: { from, to: actualTo, insert: insertText },
                // Keep caret right after # so heading suggestions are immediate.
                selection: { anchor: from + insertText.length - 2 },
                effects: setPendingAutoHeadingPrompt.of({
                  from,
                  shortId,
                  opened: false,
                }),
              });
              if (view instanceof EditorView) {
                startCompletion(view);
              }
            },
          };
        }),
        filter: false,
      };
    }

    let headings = headingCache.get(parsed.shortId);
    if (!headings) {
      headings = await api.resolveWikiLinkHeadings(parsed.shortId);
      headingCache.set(parsed.shortId, headings);
    }
    const filtered = headings
      .filter((heading) =>
        parsed.headingQuery.length === 0
          || heading.toLowerCase().includes(parsed.headingQuery),
      )
      .slice(0, 30);
    if (filtered.length === 0) return null;

    const headingFrom = match.from + 2 + parsed.shortId.length + 1;
    return {
      from: headingFrom,
      options: filtered.map((heading) => ({
        label: heading,
        type: "wiki-link",
        apply: (view: EditorView, _completion: object, from: number, to: number) => {
          view.dispatch({
            changes: { from, to, insert: heading },
            selection: { anchor: from + heading.length },
            effects: clearPendingAutoHeadingPrompt.of(),
          });
        },
      })),
      filter: false,
    };
  };
}

const wikiLinkCompletionSource = createWikiLinkCompletionSource(defaultWikiLinkApi);

// Input handler: when user types the second [, auto-insert ]] and open picker.
const wikiLinkInputHandler = EditorView.inputHandler.of((view, from, to, text) => {
  if (text !== "[") return false;
  const prevChar = from > 0 ? view.state.doc.sliceString(from - 1, from) : "";
  if (prevChar !== "[") return false;
  view.dispatch({
    changes: { from, to, insert: "[]]" },
    selection: { anchor: from + 1 },
  });
  startCompletion(view);
  return true;
});

function targetElementFromEvent(event: MouseEvent): Element | null {
  const target = event.target;
  if (!target) return null;
  if (target instanceof Element) return target;
  return target instanceof Node ? target.parentElement : null;
}

function wikiLinkAtMouseEvent(view: EditorView, event: MouseEvent) {
  const coordPos = view.posAtCoords({ x: event.clientX, y: event.clientY });
  if (coordPos != null) {
    const line = view.state.doc.lineAt(coordPos);
    return markdownWikiLinkAtCursor(line.text, coordPos - line.from);
  }
  const targetEl = targetElementFromEvent(event);
  if (!targetEl) return null;
  const pos = view.posAtDOM(targetEl, 0);
  const line = view.state.doc.lineAt(pos);
  return markdownWikiLinkAtCursor(line.text, pos - line.from);
}

export function tryNavigateWikiLinkFromMouseEvent(
  event: MouseEvent,
  view: EditorView,
  onNavigate: (noteId: string, heading?: string) => void,
  api: WikiLinkApi = defaultWikiLinkApi,
): boolean {
  if (!event.ctrlKey && !event.metaKey) return false;
  const link = wikiLinkAtMouseEvent(view, event);
  if (!link) return false;
  api.resolveWikiLink(link.shortId)
    .then((result) => {
      if (result) onNavigate(result.id, link.heading ?? undefined);
    })
    .catch(() => {});
  event.preventDefault();
  event.stopPropagation();
  return true;
}

function wikiLinkClickHandler(
  onNavigate: (noteId: string, heading?: string) => void,
  api: WikiLinkApi,
): (event: MouseEvent, view: EditorView) => boolean {
  return (event, view) => tryNavigateWikiLinkFromMouseEvent(event, view, onNavigate, api);
}

function wikiLinkContextMenuHandler(
  onNavigate: (noteId: string, heading?: string) => void,
  api: WikiLinkApi,
): (event: MouseEvent, view: EditorView) => boolean {
  return (event, view) => tryNavigateWikiLinkFromMouseEvent(event, view, onNavigate, api);
}

export { wikiLinkCompletionSource };

export function invalidateWikiLinkCompletionCaches(shortId?: string) {
  noteListCache = null;
  if (shortId && SHORT_ID_RE.test(shortId)) {
    headingCache.delete(shortId);
  } else {
    headingCache.clear();
  }
}

export function wikiLinkExtensions(
  onNavigate?: (noteId: string, heading?: string) => void,
  api: WikiLinkApi = defaultWikiLinkApi,
) {
  const extensions: Extension[] = [
    pendingAutoHeadingPromptField,
    autoHeadingPromptLifecycle,
    wikiLinkInputHandler,
  ];

  if (onNavigate) {
    extensions.push(
      Prec.highest(EditorView.domEventHandlers({
        click: wikiLinkClickHandler(onNavigate, api),
        mousedown: (event, view) => {
          const mouseEvent = event as MouseEvent;
          if (mouseEvent.button !== 2) return false;
          return tryNavigateWikiLinkFromMouseEvent(mouseEvent, view, onNavigate, api);
        },
        contextmenu: wikiLinkContextMenuHandler(onNavigate, api),
      })),
    );
  }

  return extensions;
}
