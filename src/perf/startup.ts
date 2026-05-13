import { appendStartupLog } from "../api.ts";

interface StartupMark {
  name: string;
  ms: number;
}

const hasWindow = typeof window !== "undefined";
const hasPerf = typeof performance !== "undefined";
const start = hasWindow && hasPerf ? performance.now() : 0;
const marks: StartupMark[] = [];
let flushed = false;

const markAlias: Record<string, string> = {
  ui_bootstrap_start: "loading_screen",
  ui_theme_load_requested: "loading_theme",
  ui_init_app_start: "loading_app",
  ui_data_loaded: "loading_data",
  ui_editor_mounted: "loading_editor",
  ui_codemirror_ready: "loading_editor_ready",
  ui_editor_hydrated: "loading_editor_hydrated",
  ui_wasm_ready: "loading_calc_engine",
  ui_app_ready: "ready",
};

function nowMs(): number {
  if (!hasWindow || !hasPerf) return 0;
  return performance.now() - start;
}

function prettyDuration(ms: number): string {
  if (ms >= 1000) {
    const seconds = ms / 1000;
    const text = seconds >= 10 ? seconds.toFixed(1) : seconds.toFixed(2);
    const normalized = text.replace(/\.0+$/, "").replace(/(\.\d*[1-9])0+$/, "$1");
    return `${normalized}s`;
  }
  return `${Math.max(0, Math.round(ms))}ms`;
}

function normalizeMode(mode: string): string {
  if (mode === "gui") return "ui";
  if (mode === "terminal") return "tui";
  return mode;
}

export function startupMark(name: string): void {
  if (!hasWindow || !hasPerf || flushed) return;
  const label = markAlias[name] ?? name;
  marks.push({ name: label, ms: Number(nowMs().toFixed(3)) });
}

export function startupFlush(mode = "ui"): void {
  if (!hasWindow || !hasPerf || flushed) return;
  flushed = true;
  const ts = new Date().toISOString();
  const parts = marks.map((mark) => `${mark.name}:${prettyDuration(mark.ms)}`);
  const line = `time:${ts} ${parts.join(" ")}`.trimEnd();

  void appendStartupLog(normalizeMode(mode), line).catch((error) => {
    console.error("Failed to append UI startup log:", error);
  });
}
