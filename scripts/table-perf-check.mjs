import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { spawnSync } from "node:child_process";

const BASELINE_PATH = resolve("perf/baselines/table.json");
const PROFILE = process.env.NOTE_TABLE_PERF_PROFILE ?? "ci";
const baseline = JSON.parse(readFileSync(BASELINE_PATH, "utf8"));

function runTablePerfJson(profile) {
  const command = spawnSync(
    "cargo",
    [
      "run",
      "--quiet",
      "-p",
      "slate",
      "--bin",
      "table-perf",
      "--release",
      "--",
      "--profile",
      profile,
      "--json",
    ],
    { encoding: "utf8" },
  );

  if (command.status !== 0) {
    const detail = (command.stderr || command.stdout || "unknown error").trim();
    throw new Error(`table-perf probe failed: ${detail}`);
  }
  const output = command.stdout.trim();
  if (!output) throw new Error("table-perf probe returned empty output");
  return JSON.parse(output);
}

function worstP95ByMetric(stats) {
  const out = new Map();
  for (const stat of stats ?? []) {
    const prev = out.get(stat.metric);
    if (prev == null || stat.p95_ms > prev) out.set(stat.metric, stat.p95_ms);
  }
  return out;
}

const report = runTablePerfJson(PROFILE);
const limits = baseline.limits_p95_ms ?? {};
const failures = [];
const worst = worstP95ByMetric(report.stats ?? []);

for (const [metric, limit] of Object.entries(limits)) {
  const observed = worst.get(metric);
  if (typeof observed !== "number") {
    failures.push(`missing metric '${metric}' in report`);
    continue;
  }
  if (observed > limit) {
    failures.push(
      `${metric}: p95 ${observed.toFixed(3)}ms exceeds limit ${Number(limit).toFixed(3)}ms`,
    );
  }
}

if (failures.length > 0) {
  console.error(
    `Table perf regression detected for profile '${PROFILE}' (${baseline.description ?? "no description"}):`,
  );
  for (const failure of failures) {
    console.error(`- ${failure}`);
  }
  process.exit(1);
}

console.log(`Table perf check passed for profile '${PROFILE}'.`);
for (const [metric, observed] of worst) {
  console.log(`- ${metric}: worst p95 ${observed.toFixed(3)}ms`);
}
