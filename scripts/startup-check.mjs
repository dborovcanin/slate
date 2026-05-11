import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { spawnSync } from "node:child_process";

const RUNS = Number.parseInt(process.env.NOTE_STARTUP_RUNS ?? "3", 10);
const BASELINE_PATH = resolve("perf/baselines/startup.json");

function median(values) {
  if (values.length === 0) return 0;
  const sorted = [...values].sort((a, b) => a - b);
  const mid = Math.floor(sorted.length / 2);
  return sorted.length % 2 === 0
    ? (sorted[mid - 1] + sorted[mid]) / 2
    : sorted[mid];
}

function runProbe(mode) {
  const command = spawnSync(
    "cargo",
    ["run", "--quiet", "--bin", "note-startup", "--", mode],
    { encoding: "utf8" },
  );

  if (command.status !== 0) {
    const detail = (command.stderr || command.stdout || "unknown error").trim();
    throw new Error(`startup probe '${mode}' failed: ${detail}`);
  }

  const output = command.stdout.trim();
  if (!output) throw new Error(`startup probe '${mode}' returned empty output`);

  return JSON.parse(output);
}

function summarize(samples) {
  const byName = new Map();
  for (const sample of samples) {
    for (const mark of sample.marks ?? []) {
      if (!byName.has(mark.name)) byName.set(mark.name, []);
      byName.get(mark.name).push(mark.ms);
    }
  }

  const medians = {};
  for (const [name, values] of byName) {
    medians[name] = median(values);
  }
  return medians;
}

const baseline = JSON.parse(readFileSync(BASELINE_PATH, "utf8"));
const thresholdPct = Number.parseFloat(
  process.env.NOTE_STARTUP_THRESHOLD_PCT ?? `${baseline.threshold_pct ?? 15}`,
);
const thresholdFactor = 1 + thresholdPct / 100;

const regressions = [];
const modeSummaries = {};

for (const mode of Object.keys(baseline.modes ?? {})) {
  const samples = [];
  for (let i = 0; i < RUNS; i += 1) {
    samples.push(runProbe(mode));
  }

  const current = summarize(samples);
  modeSummaries[mode] = current;
  const expected = baseline.modes[mode] ?? {};

  for (const [metric, expectedMs] of Object.entries(expected)) {
    if (metric === "probe_start" || expectedMs <= 0) continue;
    const currentMs = current[metric];
    if (typeof currentMs !== "number") continue;

    const limit = expectedMs * thresholdFactor;
    if (currentMs > limit) {
      regressions.push({
        mode,
        metric,
        baseline_ms: Number(expectedMs.toFixed(3)),
        current_ms: Number(currentMs.toFixed(3)),
        limit_ms: Number(limit.toFixed(3)),
      });
    }
  }
}

function printModeSummaries() {
  for (const [mode, marks] of Object.entries(modeSummaries)) {
    console.log(`- ${mode} median marks:`);
    for (const [name, value] of Object.entries(marks).sort(([a], [b]) =>
      a.localeCompare(b),
    )) {
      console.log(`  ${name}: ${Number(value).toFixed(3)}ms`);
    }
  }
}

if (regressions.length > 0) {
  printModeSummaries();
  console.error("Startup regression detected:");
  for (const r of regressions) {
    console.error(
      `- [${r.mode}] ${r.metric}: current ${r.current_ms}ms > limit ${r.limit_ms}ms (baseline ${r.baseline_ms}ms)`,
    );
  }
  process.exit(1);
}

console.log(`Startup check passed (${thresholdPct}% threshold, ${RUNS} runs).`);
printModeSummaries();
