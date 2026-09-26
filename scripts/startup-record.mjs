import { mkdirSync, writeFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { spawnSync } from "node:child_process";

const MODES = ["tui"];
const RUNS = Number.parseInt(process.env.NOTE_STARTUP_RUNS ?? "5", 10);
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

  try {
    return JSON.parse(output);
  } catch (error) {
    throw new Error(`invalid JSON from '${mode}' probe: ${error}`);
  }
}

function summarize(samples) {
  const byName = new Map();
  for (const sample of samples) {
    for (const mark of sample.marks ?? []) {
      const key = mark.name;
      if (!byName.has(key)) byName.set(key, []);
      byName.get(key).push(mark.ms);
    }
  }

  const medians = {};
  for (const [name, values] of byName) {
    medians[name] = Number(median(values).toFixed(3));
  }

  return medians;
}

const baseline = {
  version: 1,
  recorded_at: new Date().toISOString(),
  threshold_pct: 15,
  runs: RUNS,
  modes: {},
};

for (const mode of MODES) {
  const samples = [];
  for (let i = 0; i < RUNS; i += 1) {
    samples.push(runProbe(mode));
  }
  baseline.modes[mode] = summarize(samples);
}

mkdirSync(dirname(BASELINE_PATH), { recursive: true });
writeFileSync(BASELINE_PATH, `${JSON.stringify(baseline, null, 2)}\n`, "utf8");

console.log(`Wrote startup baseline: ${BASELINE_PATH}`);
console.log(JSON.stringify(baseline, null, 2));
