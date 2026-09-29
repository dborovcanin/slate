import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { spawnSync } from "node:child_process";

const CONFIG_PATH = resolve("perf/config.json");

function defaultConfig() {
  return {
    checks: {
      startup: { enabled: true, runs: 3, threshold_pct: 15 },
      table: { enabled: true, profile: "ci" },
      images: {
        enabled: true,
        lines: 100000,
        viewport_lines: 24,
        iterations: 1000,
        threshold_p95_ms: 5,
      },
    },
  };
}

function loadConfig() {
  try {
    const parsed = JSON.parse(readFileSync(CONFIG_PATH, "utf8"));
    const defaults = defaultConfig();
    return {
      ...defaults,
      ...parsed,
      checks: {
        startup: { ...defaults.checks.startup, ...(parsed.checks?.startup ?? {}) },
        table: { ...defaults.checks.table, ...(parsed.checks?.table ?? {}) },
        images: { ...defaults.checks.images, ...(parsed.checks?.images ?? {}) },
      },
    };
  } catch (error) {
    throw new Error(`failed to load ${CONFIG_PATH}: ${error}`);
  }
}

function bool(value, fallback = false) {
  if (typeof value === "boolean") return value;
  return fallback;
}

function runNodeScript(scriptPath, extraEnv = {}) {
  const started = Date.now();
  const command = spawnSync(process.execPath, [scriptPath], {
    encoding: "utf8",
    env: { ...process.env, ...extraEnv },
  });
  const elapsedMs = Date.now() - started;
  return {
    ok: command.status === 0,
    elapsedMs,
    output: `${command.stdout ?? ""}${command.stderr ?? ""}`.trim(),
  };
}

function loadRuntimePerfConfig() {
  const command = spawnSync(
    "cargo",
    ["run", "--quiet", "-p", "slate", "--bin", "perf-config"],
    { encoding: "utf8", env: { ...process.env } },
  );
  if (command.status !== 0) {
    const detail = (command.stderr || command.stdout || "unknown error").trim();
    throw new Error(`failed to read runtime perf config: ${detail}`);
  }
  const output = command.stdout.trim();
  if (!output) {
    throw new Error("runtime perf config probe returned empty output");
  }
  return JSON.parse(output);
}

function printSection(title) {
  console.log(`\n== ${title} ==`);
}

function printIndented(text) {
  if (!text) return;
  for (const line of text.split(/\r?\n/)) {
    console.log(`  ${line}`);
  }
}

const cfg = loadConfig();
const runtimePerf = loadRuntimePerfConfig();
const enabled = bool(runtimePerf.enabled, false);
const logPath = runtimePerf.log_path ?? "";
const startedIso = new Date().toISOString();

console.log("Slate unified perf check");
console.log(`time: ${startedIso}`);
console.log(`config: ${CONFIG_PATH}`);
console.log("runtime: config.toml [perf]");
console.log(`enabled: ${enabled ? "true" : "false"} (${enabled ? "on" : "off"})`);
console.log(`log: ${logPath}`);

if (!enabled) {
  printSection("Result");
  console.log("Perf checks are disabled by config (enabled=false).");
  process.exit(0);
}

const results = [];

if (bool(cfg.checks?.startup?.enabled, true)) {
  const env = {
    NOTE_STARTUP_RUNS: String(cfg.checks.startup.runs ?? 3),
    NOTE_STARTUP_THRESHOLD_PCT: String(cfg.checks.startup.threshold_pct ?? 15),
  };
  const run = runNodeScript(resolve("scripts/startup-check.mjs"), env);
  results.push({ name: "startup", ...run });
}

if (bool(cfg.checks?.table?.enabled, true)) {
  const env = {
    NOTE_TABLE_PERF_PROFILE: String(cfg.checks.table.profile ?? "ci"),
  };
  const run = runNodeScript(resolve("scripts/table-perf-check.mjs"), env);
  results.push({ name: "table", ...run });
}

if (bool(cfg.checks?.images?.enabled, true)) {
  const env = {
    SLATE_IMAGE_PERF_LINES: String(cfg.checks.images.lines ?? 100000),
    SLATE_IMAGE_PERF_VIEWPORT_LINES: String(cfg.checks.images.viewport_lines ?? 24),
    SLATE_IMAGE_PERF_ITERATIONS: String(cfg.checks.images.iterations ?? 1000),
    SLATE_IMAGE_PERF_P95_THRESHOLD_MS: String(cfg.checks.images.threshold_p95_ms ?? 5),
  };
  const run = runNodeScript(resolve("scripts/image-perf-check.mjs"), env);
  results.push({ name: "images", ...run });
}

printSection("Checks");
for (const result of results) {
  console.log(
    `${result.ok ? "PASS" : "FAIL"} ${result.name} (${result.elapsedMs}ms)`,
  );
  printIndented(result.output);
}

const failed = results.filter((result) => !result.ok);
printSection("Summary");
console.log(`total: ${results.length}`);
console.log(`passed: ${results.length - failed.length}`);
console.log(`failed: ${failed.length}`);

if (failed.length > 0) {
  process.exit(1);
}
