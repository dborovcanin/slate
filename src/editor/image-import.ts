import { Prec, type Extension } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { state } from "../state";
import {
  deleteNoteImage,
  importNoteImageFromClipboard,
  reserveNoteImage,
  writeNoteImage,
  writeNoteImageFromPath,
} from "../api";
import { requestMarkdownDecorationRefresh } from "./markdown-decoration";

const FILE_TO_BASE64_CHUNK = 0x8000;
const IMAGE_EXTENSIONS = new Set([
  "png",
  "jpg",
  "jpeg",
  "gif",
  "webp",
  "bmp",
]);

type PendingImageInsertion =
  | { mode: "selection"; from: number; to: number; selectionAfterInsert?: "start" | "end" }
  | { mode: "position"; position: number; selectionAfterInsert?: "start" | "end" };

function fileBaseName(fileName: string): string {
  const trimmed = fileName.trim();
  if (trimmed.length === 0) return "Image";
  const slash = Math.max(trimmed.lastIndexOf("/"), trimmed.lastIndexOf("\\"));
  const name = slash >= 0 ? trimmed.slice(slash + 1) : trimmed;
  const dot = name.lastIndexOf(".");
  const stem = dot > 0 ? name.slice(0, dot) : name;
  const cleaned = stem.replace(/[_-]+/g, " ").trim();
  if (cleaned.length === 0) return "Image";
  return cleaned.length > 80 ? `${cleaned.slice(0, 80)}...` : cleaned;
}

function isLikelyImageName(name: string): boolean {
  const trimmed = name.trim();
  if (trimmed.length === 0) return false;
  const base = trimmed.split(/[\\/]/).pop() ?? trimmed;
  const dot = base.lastIndexOf(".");
  if (dot < 0) return false;
  const ext = base.slice(dot + 1).toLowerCase();
  return IMAGE_EXTENSIONS.has(ext);
}

function escapeMarkdownImageAlt(text: string): string {
  return text.replace(/\\/g, "\\\\").replace(/\]/g, "\\]");
}

function bytesToBase64(bytes: Uint8Array): string {
  let binary = "";
  for (let i = 0; i < bytes.length; i += FILE_TO_BASE64_CHUNK) {
    const chunk = bytes.subarray(i, i + FILE_TO_BASE64_CHUNK);
    binary += String.fromCharCode(...chunk);
  }
  return btoa(binary);
}

function gatherClipboardImageFiles(event: ClipboardEvent): File[] {
  const files = Array.from(event.clipboardData?.files ?? []).filter((file) =>
    file.type.startsWith("image/") || isLikelyImageName(file.name),
  );
  if (files.length > 0) return files;
  const items = Array.from(event.clipboardData?.items ?? []).filter((item) =>
    item.type.startsWith("image/"),
  );
  return items
    .map((item) => item.getAsFile())
    .filter((file): file is File => !!file);
}

function gatherDropImageFiles(event: DragEvent): File[] {
  const files = Array.from(event.dataTransfer?.files ?? []).filter((file) =>
    file.type.startsWith("image/") || isLikelyImageName(file.name),
  );
  if (files.length > 0) return files;
  const items = Array.from(event.dataTransfer?.items ?? []).filter((item) =>
    item.type.startsWith("image/"),
  );
  return items
    .map((item) => item.getAsFile())
    .filter((file): file is File => !!file);
}

async function buildImageMarkdownSnippets(
  noteId: string,
  files: File[],
): Promise<Array<{ imageId: string; source: string; markdown: string; upload: () => Promise<void> }>> {
  const uploads: Array<{ imageId: string; source: string; markdown: string; upload: () => Promise<void> }> = [];
  for (const file of files) {
    const bytes = new Uint8Array(await file.arrayBuffer());
    if (bytes.length === 0) continue;
    const reserved = await reserveNoteImage(noteId, {
      fileName: file.name || null,
      mimeType: file.type || null,
    });
    const alt = escapeMarkdownImageAlt(fileBaseName(file.name));
    const payload = bytesToBase64(bytes);
    uploads.push({
      imageId: reserved.imageId,
      source: reserved.markdownPath,
      markdown: `![${alt}](${reserved.markdownPath})`,
      upload: () => writeNoteImage(noteId, reserved.imageId, payload, {
        fileName: file.name || null,
        mimeType: file.type || null,
      }),
    });
  }
  return uploads;
}

function unquotePathToken(raw: string): string {
  const trimmed = raw.trim();
  if (
    (trimmed.startsWith("\"") && trimmed.endsWith("\"")) ||
    (trimmed.startsWith("'") && trimmed.endsWith("'"))
  ) {
    return trimmed.slice(1, -1);
  }
  return trimmed;
}

function decodeFileUrlPath(path: string): string {
  let value = path;
  if (!value.startsWith("file://")) return value;
  value = value.slice("file://".length);
  if (value.startsWith("localhost/")) {
    value = value.slice("localhost".length);
  }
  if (/^\/[A-Za-z]:\//.test(value)) {
    value = value.slice(1);
  }
  try {
    return decodeURIComponent(value);
  } catch {
    return value;
  }
}

export function normalizeImagePathToken(raw: string): string | null {
  let value = unquotePathToken(raw).trim();
  if (value.length === 0) return null;
  if (value.startsWith("#")) return null;
  value = decodeFileUrlPath(value);
  value = value.replace(/\\ /g, " ");
  if (!isLikelyImageName(value)) return null;
  return value;
}

function parsePastedPathCandidates(raw: string): string[] {
  const text = raw.trim();
  if (text.length === 0) return [];
  const matches = text.match(/"[^"]+"|'[^']+'|\S+/g) ?? [];
  const paths: string[] = [];
  for (const token of matches) {
    const value = normalizeImagePathToken(token);
    if (!value) continue;
    paths.push(value);
  }
  return paths;
}

function parseDroppedPathCandidates(event: DragEvent): string[] {
  const uriList = event.dataTransfer?.getData("text/uri-list") ?? "";
  const plain = event.dataTransfer?.getData("text/plain") ?? "";
  const fromUris = parsePastedPathCandidates(uriList);
  const fromPlain = parsePastedPathCandidates(plain);
  return [...fromUris, ...fromPlain];
}

function hasFileDropPayload(event: DragEvent): boolean {
  const dataTransfer = event.dataTransfer;
  if (!dataTransfer) return false;
  return Array.from(dataTransfer.types).includes("Files");
}

async function buildImageMarkdownSnippetsFromPaths(
  noteId: string,
  paths: string[],
): Promise<Array<{ imageId: string; source: string; markdown: string; upload: () => Promise<void> }>> {
  const uploads: Array<{ imageId: string; source: string; markdown: string; upload: () => Promise<void> }> = [];
  for (const path of paths) {
    const reserved = await reserveNoteImage(noteId, {
      fileName: path.split(/[\\/]/).pop() ?? path,
      mimeType: null,
    });
    const fallbackName = path.split(/[\\/]/).pop() ?? path;
    const alt = escapeMarkdownImageAlt(fileBaseName(fallbackName));
    uploads.push({
      imageId: reserved.imageId,
      source: reserved.markdownPath,
      markdown: `![${alt}](${reserved.markdownPath})`,
      upload: () => writeNoteImageFromPath(noteId, reserved.imageId, path),
    });
  }
  return uploads;
}

function applyImageSnippetsInsertion(
  editorView: EditorView,
  snippets: string[],
  insertion: PendingImageInsertion,
) {
  if (snippets.length === 0) return;
  const insert = snippets.join("\n");
  const selectionAfterInsert = insertion.selectionAfterInsert ?? "end";
  if (insertion.mode === "selection") {
    const from = Math.max(0, Math.min(insertion.from, editorView.state.doc.length));
    const to = Math.max(from, Math.min(insertion.to, editorView.state.doc.length));
    const anchor = selectionAfterInsert === "start" ? from : from + insert.length;
    editorView.dispatch({
      changes: { from, to, insert },
      selection: { anchor },
      scrollIntoView: true,
    });
    return;
  }
  const from = Math.max(0, Math.min(insertion.position, editorView.state.doc.length));
  const anchor = selectionAfterInsert === "start" ? from : from + insert.length;
  editorView.dispatch({
    changes: { from, to: from, insert },
    selection: { anchor },
    scrollIntoView: true,
  });
}

function lineStartInsertionPoint(editorView: EditorView, position: number): number {
  const clamped = Math.max(0, Math.min(position, editorView.state.doc.length));
  return editorView.state.doc.lineAt(clamped).from;
}

function placeCursorAtLineStart(editorView: EditorView, position: number): number {
  const lineStart = lineStartInsertionPoint(editorView, position);
  editorView.dispatch({
    selection: { anchor: lineStart },
    scrollIntoView: true,
  });
  return lineStart;
}

function fileNameFromMimeType(mimeType: string): string {
  const lower = mimeType.toLowerCase();
  if (lower.includes("png")) return "clipboard-image.png";
  if (lower.includes("jpeg") || lower.includes("jpg")) return "clipboard-image.jpg";
  if (lower.includes("webp")) return "clipboard-image.webp";
  if (lower.includes("gif")) return "clipboard-image.gif";
  if (lower.includes("bmp")) return "clipboard-image.bmp";
  return "clipboard-image.png";
}

async function gatherAsyncClipboardImageFiles(): Promise<File[]> {
  if (typeof navigator === "undefined" || !navigator.clipboard?.read) return [];
  try {
    const items = await navigator.clipboard.read();
    const files: File[] = [];
    for (const item of items) {
      const imageTypes = item.types.filter((type) => type.startsWith("image/"));
      for (const type of imageTypes) {
        const blob = await item.getType(type);
        if (blob.size === 0) continue;
        files.push(new File([blob], fileNameFromMimeType(type), { type }));
      }
    }
    return files;
  } catch {
    return [];
  }
}

async function importImagesAndInsert(
  editorView: EditorView,
  files: File[],
  insertion: PendingImageInsertion,
) {
  if (files.length === 0) return;
  const noteId = state.activeNote?.id;
  if (!noteId) return;
  try {
    const pending = await buildImageMarkdownSnippets(noteId, files);
    if (pending.length === 0) return;
    if (state.activeNote?.id !== noteId) return;
    const snippets = pending.map((entry) => entry.markdown);
    applyImageSnippetsInsertion(editorView, snippets, insertion);
    for (const entry of pending) {
      try {
        await entry.upload();
      } catch (error) {
        console.error("Image upload failed after placeholder insert:", error);
        void deleteNoteImage(noteId, entry.imageId).catch(() => {
          // Best effort cleanup; broken markdown placeholder stays editable.
        });
      }
    }
    requestMarkdownDecorationRefresh(editorView, {
      noteId,
      invalidatedImageSources: pending.map((entry) => entry.source),
    });
  } catch (error) {
    console.error("Image import failed:", error);
  }
}

async function importImagePathsAndInsert(
  editorView: EditorView,
  paths: string[],
  insertion: PendingImageInsertion,
): Promise<boolean> {
  if (paths.length === 0) return false;
  const noteId = state.activeNote?.id;
  if (!noteId) return false;
  try {
    const pending = await buildImageMarkdownSnippetsFromPaths(noteId, paths);
    if (pending.length === 0) return false;
    if (state.activeNote?.id !== noteId) return false;
    const snippets = pending.map((entry) => entry.markdown);
    applyImageSnippetsInsertion(editorView, snippets, insertion);
    for (const entry of pending) {
      try {
        await entry.upload();
      } catch (error) {
        console.error("Image path upload failed after placeholder insert:", error);
        void deleteNoteImage(noteId, entry.imageId).catch(() => {
          // Best effort cleanup; broken markdown placeholder stays editable.
        });
      }
    }
    requestMarkdownDecorationRefresh(editorView, {
      noteId,
      invalidatedImageSources: pending.map((entry) => entry.source),
    });
    return true;
  } catch (error) {
    console.error("Image path import failed:", error);
    return false;
  }
}

export async function insertImagePathsAtCursor(
  editorView: EditorView,
  paths: string[],
): Promise<boolean> {
  const filtered = paths
    .map((path) => normalizeImagePathToken(path))
    .filter((path): path is string => !!path);
  if (filtered.length === 0) return false;
  const anchor = editorView.state.selection.main.head;
  return importImagePathsAndInsert(editorView, filtered, {
    mode: "position",
    position: anchor,
  });
}

async function importClipboardImageAndInsert(
  editorView: EditorView,
  insertion: PendingImageInsertion,
): Promise<boolean> {
  const noteId = state.activeNote?.id;
  if (!noteId) return false;
  try {
    const imported = await importNoteImageFromClipboard(noteId);
    if (!imported) return false;
    if (state.activeNote?.id !== noteId) return false;
    applyImageSnippetsInsertion(
      editorView,
      [`![Clipboard Image](${imported.markdownPath})`],
      insertion,
    );
    requestMarkdownDecorationRefresh(editorView, {
      noteId,
      invalidatedImageSources: [imported.markdownPath],
    });
    return true;
  } catch (error) {
    console.error("Clipboard image import failed:", error);
    return false;
  }
}

export function handleImagePasteAtPosition(
  event: ClipboardEvent,
  view: EditorView,
  position: number,
): boolean {
  const imageFiles = gatherClipboardImageFiles(event);
  if (imageFiles.length > 0) {
    event.preventDefault();
    void importImagesAndInsert(view, imageFiles, {
      mode: "position",
      position,
      selectionAfterInsert: "end",
    });
    return true;
  }
  const text = event.clipboardData?.getData("text/plain") ?? "";
  const imagePaths = parsePastedPathCandidates(text);
  if (imagePaths.length > 0) {
    event.preventDefault();
    void importImagePathsAndInsert(view, imagePaths, {
      mode: "position",
      position,
      selectionAfterInsert: "end",
    });
    return true;
  }
  if ((event.clipboardData?.items?.length ?? 0) > 0 || text.trim().length > 0) {
    return false;
  }
  event.preventDefault();
  void (async () => {
    const asyncFiles = await gatherAsyncClipboardImageFiles();
    if (asyncFiles.length > 0) {
      await importImagesAndInsert(view, asyncFiles, {
        mode: "position",
        position,
        selectionAfterInsert: "end",
      });
      return;
    }
    await importClipboardImageAndInsert(view, {
      mode: "position",
      position,
      selectionAfterInsert: "end",
    });
  })();
  return true;
}

export function imageImportDomHandlers(): Extension {
  let dragCursorPos = -1;
  return Prec.highest(
    EditorView.domEventHandlers({
      paste: (event, view) => {
        const main = view.state.selection.main;
        const lineStart = placeCursorAtLineStart(view, main.from);
        return handleImagePasteAtPosition(event, view, lineStart);
      },
      dragover: (event, view) => {
        const imageFiles = gatherDropImageFiles(event);
        const imagePaths = imageFiles.length === 0 ? parseDroppedPathCandidates(event) : [];
        if (imageFiles.length === 0 && imagePaths.length === 0 && !hasFileDropPayload(event)) {
          return false;
        }
        event.preventDefault();
        const coords = { x: event.clientX, y: event.clientY };
        const at = view.posAtCoords(coords) ?? view.state.selection.main.head;
        if (at !== dragCursorPos) {
          dragCursorPos = at;
          view.dispatch({
            selection: { anchor: at },
            scrollIntoView: true,
          });
        }
        if (event.dataTransfer) {
          event.dataTransfer.dropEffect = "copy";
        }
        return true;
      },
      drop: (event, view) => {
        const imageFiles = gatherDropImageFiles(event);
        const imagePaths = imageFiles.length === 0 ? parseDroppedPathCandidates(event) : [];
        if (imageFiles.length === 0 && imagePaths.length === 0) return false;
        event.preventDefault();
        const coords = { x: event.clientX, y: event.clientY };
        const at = dragCursorPos >= 0
          ? dragCursorPos
          : (view.posAtCoords(coords) ?? view.state.selection.main.head);
        dragCursorPos = -1;
        if (imageFiles.length > 0) {
          void importImagesAndInsert(view, imageFiles, {
            mode: "position",
            position: at,
          });
        } else {
          void importImagePathsAndInsert(view, imagePaths, {
            mode: "position",
            position: at,
          });
        }
        return true;
      },
      dragleave: () => {
        dragCursorPos = -1;
        return false;
      },
    }),
  );
}
