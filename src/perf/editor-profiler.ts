import { appendStartupLog } from "../api.ts";

export interface EditorProfilerMetrics {
  [key: string]: number;
}

export interface EditorProfilerSample {
  name: string;
  reason: string;
  durationMs: number;
  atMs: number;
  metrics: EditorProfilerMetrics | null;
}

export interface EditorProfilerBucketSummary {
  name: string;
  reason: string;
  count: number;
  totalMs: number;
  avgMs: number;
  minMs: number;
  p50Ms: number;
  p95Ms: number;
  p99Ms: number;
  maxMs: number;
  metricAvg: EditorProfilerMetrics;
  metricMax: EditorProfilerMetrics;
}

export interface EditorProfilerSummary {
  enabled: boolean;
  capacity: number;
  sampleCount: number;
  dropped: number;
  buckets: EditorProfilerBucketSummary[];
}

interface MutableBucket {
  name: string;
  reason: string;
  count: number;
  totalMs: number;
  minMs: number;
  maxMs: number;
  durations: number[];
  metricSums: EditorProfilerMetrics;
  metricMax: EditorProfilerMetrics;
}

const DEFAULT_CAPACITY = 512;
const DEFAULT_TOP_BUCKETS = 12;
const PERF_LOG_MODE = "ui_perf";
const PERF_LOG_BASENAME = "note-startup-ui_perf.log";
const STORAGE_KEY = "slate.editorProfiler.enabled";
const QUERY_KEYS = [
  "editor_profiler",
  "profile_editor",
  "perf_editor",
];
const ENABLED_VALUES = new Set(["1", "true", "on", "yes", "enabled"]);
const DISABLED_VALUES = new Set(["0", "false", "off", "no", "disabled"]);

let profilerEnabled = false;
let runtimeConfigLoaded = false;
let globalApiInstalled = false;

function perfNowMs(): number {
  if (typeof performance !== "undefined") return performance.now();
  return Date.now();
}

function normalizeBooleanLike(value: string | null | undefined): boolean | null {
  if (typeof value !== "string") return null;
  const normalized = value.trim().toLowerCase();
  if (ENABLED_VALUES.has(normalized)) return true;
  if (DISABLED_VALUES.has(normalized)) return false;
  return null;
}

function normalizeName(name: string): string {
  const trimmed = name.trim();
  return trimmed.length > 0 ? trimmed : "unknown";
}

function normalizeReason(reason: string | null | undefined): string {
  const trimmed = typeof reason === "string" ? reason.trim() : "";
  return trimmed.length > 0 ? trimmed : "unspecified";
}

function normalizeMetrics(metrics: EditorProfilerMetrics | null | undefined): EditorProfilerMetrics | null {
  if (!metrics) return null;
  const out: EditorProfilerMetrics = {};
  for (const [key, value] of Object.entries(metrics)) {
    if (!Number.isFinite(value)) continue;
    out[key] = value;
  }
  return Object.keys(out).length > 0 ? out : null;
}

function normalizeCapacity(capacity: number): number {
  if (!Number.isFinite(capacity)) return DEFAULT_CAPACITY;
  return Math.max(1, Math.floor(capacity));
}

function formatMs(value: number): string {
  const normalized = value >= 100 ? value.toFixed(1) : value.toFixed(2);
  return `${normalized.replace(/\.0+$/, "").replace(/(\.\d*[1-9])0+$/, "$1")}ms`;
}

function formatMetricRecord(record: EditorProfilerMetrics): string {
  const parts: string[] = [];
  for (const key of Object.keys(record).sort()) {
    const value = record[key];
    if (!Number.isFinite(value)) continue;
    parts.push(`${key}=${Number(value.toFixed(2))}`);
  }
  return parts.join(",");
}

export class RollingProfilerBuffer {
  private capacityValue: number;
  private entries: (EditorProfilerSample | undefined)[];
  private start = 0;
  private size = 0;
  private dropped = 0;

  constructor(capacity = DEFAULT_CAPACITY) {
    this.capacityValue = normalizeCapacity(capacity);
    this.entries = new Array(this.capacityValue);
  }

  get capacity(): number {
    return this.capacityValue;
  }

  get droppedCount(): number {
    return this.dropped;
  }

  get sampleCount(): number {
    return this.size;
  }

  clear() {
    this.entries = new Array(this.capacityValue);
    this.start = 0;
    this.size = 0;
    this.dropped = 0;
  }

  resize(capacity: number) {
    const nextCapacity = normalizeCapacity(capacity);
    if (nextCapacity === this.capacityValue) return;
    const current = this.snapshot();
    this.capacityValue = nextCapacity;
    this.entries = new Array(nextCapacity);
    this.start = 0;
    this.size = 0;
    const keep = current.slice(Math.max(0, current.length - nextCapacity));
    this.dropped += current.length - keep.length;
    for (const entry of keep) {
      this.push(entry);
    }
  }

  push(sample: EditorProfilerSample) {
    if (this.size < this.capacityValue) {
      const index = (this.start + this.size) % this.capacityValue;
      this.entries[index] = sample;
      this.size += 1;
      return;
    }
    this.entries[this.start] = sample;
    this.start = (this.start + 1) % this.capacityValue;
    this.dropped += 1;
  }

  snapshot(): EditorProfilerSample[] {
    const out: EditorProfilerSample[] = [];
    for (let i = 0; i < this.size; i++) {
      const index = (this.start + i) % this.capacityValue;
      const entry = this.entries[index];
      if (entry) out.push(entry);
    }
    return out;
  }
}

const profilerBuffer = new RollingProfilerBuffer(DEFAULT_CAPACITY);

function hasBrowserWindow(): boolean {
  return typeof window !== "undefined";
}

function loadEnabledFromRuntime(): boolean {
  if (!hasBrowserWindow()) return false;
  const params = new URLSearchParams(window.location.search);
  for (const key of QUERY_KEYS) {
    const parsed = normalizeBooleanLike(params.get(key));
    if (parsed !== null) return parsed;
  }
  try {
    const parsed = normalizeBooleanLike(window.localStorage.getItem(STORAGE_KEY));
    if (parsed !== null) return parsed;
  } catch {
    // Ignore localStorage failures (private mode / blocked storage).
  }
  return false;
}

function persistEnabled(enabled: boolean) {
  if (!hasBrowserWindow()) return;
  try {
    window.localStorage.setItem(STORAGE_KEY, enabled ? "1" : "0");
  } catch {
    // Ignore localStorage failures.
  }
}

function ensureRuntimeConfigLoaded() {
  if (runtimeConfigLoaded) return;
  runtimeConfigLoaded = true;
  profilerEnabled = loadEnabledFromRuntime();
  installGlobalProfilerApi();
}

function installGlobalProfilerApi() {
  if (globalApiInstalled || !hasBrowserWindow()) return;
  globalApiInstalled = true;
  window.slateEditorProfiler = {
    enable: () => {
      setEditorProfilerEnabled(true);
    },
    disable: () => {
      setEditorProfilerEnabled(false);
    },
    toggle: () => {
      setEditorProfilerEnabled(!isEditorProfilerEnabled());
      return isEditorProfilerEnabled();
    },
    clear: () => {
      clearEditorProfilerSamples();
    },
    dump: (topBuckets?: number) => dumpEditorProfilerToConsole({ topBuckets }),
    status: () => getEditorProfilerSummary(),
    setCapacity: (capacity: number) => {
      setEditorProfilerCapacity(capacity);
      return getEditorProfilerSummary();
    },
  };
}

export function editorProfilerNowMs(): number {
  return perfNowMs();
}

export function isEditorProfilerEnabled(): boolean {
  ensureRuntimeConfigLoaded();
  return profilerEnabled;
}

export function setEditorProfilerEnabled(enabled: boolean) {
  ensureRuntimeConfigLoaded();
  profilerEnabled = enabled;
  persistEnabled(enabled);
}

export function setEditorProfilerCapacity(capacity: number) {
  ensureRuntimeConfigLoaded();
  profilerBuffer.resize(capacity);
}

export function clearEditorProfilerSamples() {
  profilerBuffer.clear();
}

export function recordEditorProfilerSample(
  name: string,
  durationMs: number,
  options: {
    reason?: string | null;
    metrics?: EditorProfilerMetrics | null;
    atMs?: number;
  } = {},
) {
  if (!isEditorProfilerEnabled()) return;
  const safeDuration = Number.isFinite(durationMs) ? Math.max(0, durationMs) : 0;
  profilerBuffer.push({
    name: normalizeName(name),
    reason: normalizeReason(options.reason),
    durationMs: safeDuration,
    atMs: Number.isFinite(options.atMs) ? options.atMs! : perfNowMs(),
    metrics: normalizeMetrics(options.metrics),
  });
}

export function percentile(sortedValues: readonly number[], p: number): number {
  if (sortedValues.length === 0) return 0;
  if (sortedValues.length === 1) return sortedValues[0];
  const clamped = Math.min(100, Math.max(0, p));
  const rank = (clamped / 100) * (sortedValues.length - 1);
  const lower = Math.floor(rank);
  const upper = Math.ceil(rank);
  if (lower === upper) return sortedValues[lower];
  const lowerVal = sortedValues[lower];
  const upperVal = sortedValues[upper];
  const weight = rank - lower;
  return lowerVal + (upperVal - lowerVal) * weight;
}

export function summarizeEditorProfilerSamples(
  samples: readonly EditorProfilerSample[],
  options: {
    enabled?: boolean;
    capacity?: number;
    dropped?: number;
    sortBy?: "p95" | "p99" | "total" | "max" | "avg";
  } = {},
): EditorProfilerSummary {
  const buckets = new Map<string, MutableBucket>();
  for (const sample of samples) {
    const key = `${sample.name}\u0000${sample.reason}`;
    let bucket = buckets.get(key);
    if (!bucket) {
      bucket = {
        name: sample.name,
        reason: sample.reason,
        count: 0,
        totalMs: 0,
        minMs: Number.POSITIVE_INFINITY,
        maxMs: 0,
        durations: [],
        metricSums: {},
        metricMax: {},
      };
      buckets.set(key, bucket);
    }
    bucket.count += 1;
    bucket.totalMs += sample.durationMs;
    bucket.minMs = Math.min(bucket.minMs, sample.durationMs);
    bucket.maxMs = Math.max(bucket.maxMs, sample.durationMs);
    bucket.durations.push(sample.durationMs);
    if (!sample.metrics) continue;
    for (const [metric, value] of Object.entries(sample.metrics)) {
      if (!Number.isFinite(value)) continue;
      bucket.metricSums[metric] = (bucket.metricSums[metric] ?? 0) + value;
      bucket.metricMax[metric] = Math.max(bucket.metricMax[metric] ?? value, value);
    }
  }

  const summaryBuckets: EditorProfilerBucketSummary[] = [];
  for (const bucket of buckets.values()) {
    bucket.durations.sort((a, b) => a - b);
    const metricAvg: EditorProfilerMetrics = {};
    for (const [metric, sum] of Object.entries(bucket.metricSums)) {
      metricAvg[metric] = sum / bucket.count;
    }
    summaryBuckets.push({
      name: bucket.name,
      reason: bucket.reason,
      count: bucket.count,
      totalMs: bucket.totalMs,
      avgMs: bucket.totalMs / bucket.count,
      minMs: Number.isFinite(bucket.minMs) ? bucket.minMs : 0,
      p50Ms: percentile(bucket.durations, 50),
      p95Ms: percentile(bucket.durations, 95),
      p99Ms: percentile(bucket.durations, 99),
      maxMs: bucket.maxMs,
      metricAvg,
      metricMax: bucket.metricMax,
    });
  }

  const sortBy = options.sortBy ?? "p95";
  summaryBuckets.sort((a, b) => {
    if (sortBy === "p99") return b.p99Ms - a.p99Ms || b.totalMs - a.totalMs;
    if (sortBy === "total") return b.totalMs - a.totalMs || b.p95Ms - a.p95Ms;
    if (sortBy === "max") return b.maxMs - a.maxMs || b.p95Ms - a.p95Ms;
    if (sortBy === "avg") return b.avgMs - a.avgMs || b.p95Ms - a.p95Ms;
    return b.p95Ms - a.p95Ms || b.totalMs - a.totalMs;
  });

  return {
    enabled: options.enabled ?? false,
    capacity: options.capacity ?? samples.length,
    sampleCount: samples.length,
    dropped: options.dropped ?? 0,
    buckets: summaryBuckets,
  };
}

export function getEditorProfilerSummary(
  options: {
    sortBy?: "p95" | "p99" | "total" | "max" | "avg";
  } = {},
): EditorProfilerSummary {
  ensureRuntimeConfigLoaded();
  return summarizeEditorProfilerSamples(
    profilerBuffer.snapshot(),
    {
      enabled: profilerEnabled,
      capacity: profilerBuffer.capacity,
      dropped: profilerBuffer.droppedCount,
      sortBy: options.sortBy,
    },
  );
}

export function formatEditorProfilerSummary(
  summary: EditorProfilerSummary,
  options: {
    topBuckets?: number;
  } = {},
): string {
  const lines: string[] = [];
  lines.push(
    `[editor-profiler] enabled=${summary.enabled ? "1" : "0"} `
      + `samples=${summary.sampleCount}/${summary.capacity} `
      + `dropped=${summary.dropped} buckets=${summary.buckets.length}`,
  );
  const topBuckets = Math.max(1, Math.floor(options.topBuckets ?? DEFAULT_TOP_BUCKETS));
  const buckets = summary.buckets.slice(0, topBuckets);
  for (let i = 0; i < buckets.length; i++) {
    const bucket = buckets[i];
    const metricsMax = formatMetricRecord(bucket.metricMax);
    const metricsAvg = formatMetricRecord(bucket.metricAvg);
    const metricsChunk = metricsMax.length > 0
      ? ` metrics(max:${metricsMax}${metricsAvg.length > 0 ? ` avg:${metricsAvg}` : ""})`
      : "";
    lines.push(
      `${i + 1}. ${bucket.name} reason=${bucket.reason} `
        + `count=${bucket.count} avg=${formatMs(bucket.avgMs)} `
        + `p95=${formatMs(bucket.p95Ms)} p99=${formatMs(bucket.p99Ms)} `
        + `max=${formatMs(bucket.maxMs)} total=${formatMs(bucket.totalMs)}${metricsChunk}`,
    );
  }
  return lines.join("\n");
}

export function dumpEditorProfilerToConsole(
  options: {
    topBuckets?: number;
    sortBy?: "p95" | "p99" | "total" | "max" | "avg";
  } = {},
): string {
  const summary = getEditorProfilerSummary({ sortBy: options.sortBy });
  const report = formatEditorProfilerSummary(summary, { topBuckets: options.topBuckets });
  console.info(report);
  const ts = new Date().toISOString();
  void appendStartupLog(PERF_LOG_MODE, `time:${ts}\n${report}`).catch((error) => {
    console.error("Failed to append profiler log:", error);
  });
  return report;
}

function profilerLogLocationHint(): string {
  return `${PERF_LOG_BASENAME} (system temp dir)`;
}

function normalizeCommand(rawInput: string): string {
  return rawInput.trim().replace(/^:/, "").toLowerCase();
}

function parseOptionalTopBuckets(rawToken: string | undefined): number | null {
  if (!rawToken) return null;
  const parsed = Number.parseInt(rawToken, 10);
  if (!Number.isFinite(parsed) || parsed <= 0) return null;
  return parsed;
}

function profilerStatusMessage(): string {
  const summary = getEditorProfilerSummary();
  return `perf ${summary.enabled ? "on" : "off"} `
    + `samples=${summary.sampleCount}/${summary.capacity} `
    + `dropped=${summary.dropped} buckets=${summary.buckets.length}`;
}

export function tryExecuteEditorProfilerCommand(rawInput: string): string | null {
  const normalized = normalizeCommand(rawInput);
  if (!normalized) return null;
  const tokens = normalized.split(/\s+/).filter((token) => token.length > 0);
  if (tokens.length === 0) return null;
  const root = tokens[0];
  if (root !== "perf" && root !== "profile" && root !== "profiler") return null;

  const subcommand = tokens[1] ?? "status";
  if (subcommand === "on" || subcommand === "enable" || subcommand === "1") {
    setEditorProfilerEnabled(true);
    return profilerStatusMessage();
  }
  if (subcommand === "off" || subcommand === "disable" || subcommand === "0") {
    setEditorProfilerEnabled(false);
    return profilerStatusMessage();
  }
  if (subcommand === "toggle") {
    setEditorProfilerEnabled(!isEditorProfilerEnabled());
    return profilerStatusMessage();
  }
  if (subcommand === "status") {
    return profilerStatusMessage();
  }
  if (subcommand === "clear" || subcommand === "reset") {
    clearEditorProfilerSamples();
    return profilerStatusMessage();
  }
  if (subcommand === "capacity" || subcommand === "cap") {
    const rawCap = tokens[2];
    const parsed = Number.parseInt(rawCap ?? "", 10);
    if (!Number.isFinite(parsed) || parsed <= 0) {
      return "perf usage: perf cap <positive-number>";
    }
    setEditorProfilerCapacity(parsed);
    return profilerStatusMessage();
  }
  if (subcommand === "dump") {
    const topBuckets = parseOptionalTopBuckets(tokens[2]) ?? DEFAULT_TOP_BUCKETS;
    const report = dumpEditorProfilerToConsole({ topBuckets, sortBy: "p95" });
    const lines = report.split("\n");
    return `perf dump logged (${Math.max(0, lines.length - 1)} buckets) -> ${profilerLogLocationHint()}`;
  }
  if (subcommand === "where") {
    return `perf logs: ${profilerLogLocationHint()}`;
  }

  return "perf usage: perf [status|on|off|toggle|dump [top]|where|clear|cap <n>]";
}

export interface EditorProfilerCommandSuggestion {
  value: string;
  description: string;
}

const PROFILER_COMMAND_SUGGESTIONS: EditorProfilerCommandSuggestion[] = [
  { value: "perf status", description: "show profiler status" },
  { value: "perf on", description: "enable editor profiler" },
  { value: "perf off", description: "disable editor profiler" },
  { value: "perf dump", description: "log compact profiler report" },
  { value: "perf where", description: "show profiler log file location" },
  { value: "perf clear", description: "clear profiler samples" },
];

export function listEditorProfilerCommandSuggestions(
  rawInput: string,
): EditorProfilerCommandSuggestion[] {
  const normalized = normalizeCommand(rawInput);
  if (!normalized) return [];
  if (
    !normalized.startsWith("perf")
    && !normalized.startsWith("profile")
    && !normalized.startsWith("profiler")
  ) {
    return [];
  }
  return PROFILER_COMMAND_SUGGESTIONS.filter((entry) =>
    entry.value.startsWith(normalized)
  );
}

export function resetEditorProfilerForTests() {
  profilerEnabled = false;
  runtimeConfigLoaded = true;
  profilerBuffer.clear();
  profilerBuffer.resize(DEFAULT_CAPACITY);
}

declare global {
  interface Window {
    slateEditorProfiler?: {
      enable: () => void;
      disable: () => void;
      toggle: () => boolean;
      clear: () => void;
      dump: (topBuckets?: number) => string;
      status: () => EditorProfilerSummary;
      setCapacity: (capacity: number) => EditorProfilerSummary;
    };
  }
}
