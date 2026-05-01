import type { Text } from "@codemirror/state";

export type AutosaveSnapshotSource = "cache" | "inline" | "background";

export interface AutosaveSnapshot {
  body: string;
  version: number;
  source: AutosaveSnapshotSource;
  docLength: number;
  lineCount: number;
}

export interface AutosaveSnapshotSample {
  name: string;
  durationMs: number;
  reason: string;
  metrics: Record<string, number>;
}

export interface AutosaveSnapshotTrackerOptions {
  inlineMaxLength?: number;
  chunkLines?: number;
  now?: () => number;
  yieldToMain?: () => Promise<void>;
  onSample?: (sample: AutosaveSnapshotSample) => void;
}

const DEFAULT_INLINE_MAX_LENGTH = 200_000;
const DEFAULT_CHUNK_LINES = 1_024;

function defaultNow(): number {
  if (typeof performance !== "undefined") return performance.now();
  return Date.now();
}

function defaultYieldToMain(): Promise<void> {
  if (typeof window !== "undefined") {
    const idleWindow = window as Window & {
      requestIdleCallback?: (
        callback: () => void,
        options?: { timeout?: number },
      ) => number;
    };
    if (idleWindow.requestIdleCallback) {
      return new Promise((resolve) => {
        idleWindow.requestIdleCallback?.(() => resolve(), { timeout: 50 });
      });
    }
  }
  return new Promise((resolve) => setTimeout(resolve, 0));
}

function normalizePositiveInteger(value: number | undefined, fallback: number): number {
  if (!Number.isFinite(value)) return fallback;
  return Math.max(1, Math.floor(value!));
}

export class AutosaveSnapshotTracker {
  private versionValue = 0;
  private cached: AutosaveSnapshot | null = null;
  private task: {
    doc: Text;
    version: number;
    promise: Promise<AutosaveSnapshot>;
  } | null = null;
  private readonly inlineMaxLength: number;
  private readonly chunkLines: number;
  private readonly now: () => number;
  private readonly yieldToMain: () => Promise<void>;
  private readonly onSample: ((sample: AutosaveSnapshotSample) => void) | null;

  constructor(options: AutosaveSnapshotTrackerOptions = {}) {
    this.inlineMaxLength = normalizePositiveInteger(
      options.inlineMaxLength,
      DEFAULT_INLINE_MAX_LENGTH,
    );
    this.chunkLines = normalizePositiveInteger(options.chunkLines, DEFAULT_CHUNK_LINES);
    this.now = options.now ?? defaultNow;
    this.yieldToMain = options.yieldToMain ?? defaultYieldToMain;
    this.onSample = options.onSample ?? null;
  }

  get version(): number {
    return this.versionValue;
  }

  reset(doc: Text, body: string) {
    this.versionValue += 1;
    this.task = null;
    this.cached = {
      body,
      version: this.versionValue,
      source: "cache",
      docLength: doc.length,
      lineCount: doc.lines,
    };
  }

  markDirty(doc: Text): number {
    this.versionValue += 1;
    this.cached = null;
    this.task = null;
    if (doc.length > this.inlineMaxLength) {
      const task = this.startBackgroundSerialization(doc, this.versionValue);
      // The foreground save path will surface errors if it awaits this task.
      // Detached background serialization should not create unhandled rejections.
      void task.promise.catch(() => {});
    }
    return this.versionValue;
  }

  async bodyForDoc(doc: Text): Promise<AutosaveSnapshot> {
    for (;;) {
      const version = this.versionValue;
      if (this.cached && this.cached.version === version) {
        return { ...this.cached, source: "cache" };
      }

      if (doc.length <= this.inlineMaxLength) {
        return this.serializeInline(doc, version);
      }

      const task =
        this.task && this.task.doc === doc && this.task.version === version
          ? this.task
          : this.startBackgroundSerialization(doc, version);
      const snapshot = await task.promise;
      if (snapshot.version === this.versionValue && task.doc === doc) {
        return snapshot;
      }
    }
  }

  private serializeInline(doc: Text, version: number): AutosaveSnapshot {
    const startedAt = this.now();
    const body = doc.toString();
    const snapshot: AutosaveSnapshot = {
      body,
      version,
      source: "inline",
      docLength: doc.length,
      lineCount: doc.lines,
    };
    if (version === this.versionValue) {
      this.cached = snapshot;
      this.task = null;
    }
    this.recordSample("inline", startedAt, snapshot, {
      chunkLines: doc.lines,
    });
    return snapshot;
  }

  private startBackgroundSerialization(doc: Text, version: number) {
    const task = {
      doc,
      version,
      promise: this.serializeInChunks(doc, version),
    };
    this.task = task;
    return task;
  }

  private async serializeInChunks(doc: Text, version: number): Promise<AutosaveSnapshot> {
    const startedAt = this.now();
    const chunks: string[] = [];
    for (let lineNo = 1; lineNo <= doc.lines; lineNo += this.chunkLines) {
      const endLine = Math.min(doc.lines, lineNo + this.chunkLines - 1);
      const lines: string[] = [];
      for (let current = lineNo; current <= endLine; current++) {
        lines.push(doc.line(current).text);
      }
      chunks.push(lines.join("\n"));
      if (endLine < doc.lines) {
        await this.yieldToMain();
      }
    }

    const snapshot: AutosaveSnapshot = {
      body: chunks.join("\n"),
      version,
      source: "background",
      docLength: doc.length,
      lineCount: doc.lines,
    };
    if (version === this.versionValue && this.task?.doc === doc) {
      this.cached = snapshot;
      this.task = null;
    }
    this.recordSample("background", startedAt, snapshot, {
      chunkLines: this.chunkLines,
    });
    return snapshot;
  }

  private recordSample(
    reason: string,
    startedAt: number,
    snapshot: AutosaveSnapshot,
    extraMetrics: Record<string, number>,
  ) {
    this.onSample?.({
      name: "editor.autosave.snapshot",
      durationMs: Math.max(0, this.now() - startedAt),
      reason,
      metrics: {
        docLength: snapshot.docLength,
        lineCount: snapshot.lineCount,
        bodyLength: snapshot.body.length,
        ...extraMetrics,
      },
    });
  }
}
