const FILE_NOTE_PREFIX = "mdfile:";

function decodeBase64Url(input: string): Uint8Array | null {
  const padded = input.replace(/-/g, "+").replace(/_/g, "/");
  const withPadding = padded + "=".repeat((4 - (padded.length % 4)) % 4);
  try {
    if (typeof atob === "function") {
      const raw = atob(withPadding);
      const bytes = new Uint8Array(raw.length);
      for (let i = 0; i < raw.length; i += 1) {
        bytes[i] = raw.charCodeAt(i);
      }
      return bytes;
    }
    const maybeBuffer = (globalThis as { Buffer?: { from: (value: string, encoding: string) => Uint8Array } }).Buffer;
    if (maybeBuffer) {
      return Uint8Array.from(maybeBuffer.from(withPadding, "base64"));
    }
  } catch {
    return null;
  }
  return null;
}

function extensionForPath(path: string): string | null {
  const tail = path.split(/[\\/]/).pop() ?? "";
  const dot = tail.lastIndexOf(".");
  if (dot <= 0 || dot === tail.length - 1) return null;
  return tail.slice(dot + 1).toLowerCase();
}

function syntaxLanguageForExtension(ext: string): string | null {
  switch (ext) {
    case "json":
      return "json";
    case "yaml":
    case "yml":
      return "yaml";
    case "toml":
      return "toml";
    case "html":
    case "htm":
    case "xhtml":
      return "html";
    case "xml":
    case "svg":
      return "xml";
    case "css":
    case "scss":
    case "less":
      return "css";
    case "js":
    case "mjs":
    case "cjs":
    case "jsx":
    case "javascript":
      return "js";
    case "ts":
    case "mts":
    case "cts":
    case "tsx":
    case "typescript":
      return "ts";
    case "rs":
      return "rust";
    case "py":
      return "python";
    case "sh":
    case "bash":
    case "zsh":
    case "fish":
      return "sh";
    case "go":
      return "go";
    case "java":
      return "java";
    case "c":
    case "h":
    case "hpp":
    case "cpp":
    case "cc":
    case "cxx":
      return "c";
    case "ini":
    case "cfg":
    case "conf":
    case "properties":
      return "toml";
    default:
      return null;
  }
}

function isMarkdownExtension(ext: string | null): boolean {
  return ext === "md" || ext === "markdown" || ext === "mdown" || ext === "mkd";
}

export function filePathFromNoteId(noteId: string): string | null {
  if (!noteId.startsWith(FILE_NOTE_PREFIX)) return null;
  const encoded = noteId.slice(FILE_NOTE_PREFIX.length);
  if (encoded.length === 0) return null;
  const bytes = decodeBase64Url(encoded);
  if (!bytes) return null;
  try {
    return new TextDecoder().decode(bytes);
  } catch {
    return null;
  }
}

export function isNonMarkdownFileNoteId(noteId: string): boolean {
  const path = filePathFromNoteId(noteId);
  if (!path) return false;
  return !isMarkdownExtension(extensionForPath(path));
}

export function syntaxLanguageForNoteId(noteId: string): string | null {
  const path = filePathFromNoteId(noteId);
  if (!path) return null;
  const ext = extensionForPath(path);
  if (isMarkdownExtension(ext)) return null;
  return ext ? syntaxLanguageForExtension(ext) : null;
}
