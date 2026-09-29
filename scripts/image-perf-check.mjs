import { spawnSync } from "node:child_process";

const thresholdMs = Number(process.env.SLATE_IMAGE_PERF_P95_THRESHOLD_MS ?? 5);
const run = spawnSync("cargo", ["run", "--quiet", "-p", "slate", "--bin", "image-perf"], {
  encoding: "utf8",
  env: { ...process.env },
});
const output = `${run.stdout ?? ""}${run.stderr ?? ""}`.trim();
if (run.status !== 0) {
  console.error(output || "image performance probe failed");
  process.exit(run.status ?? 1);
}

console.log(output);
const p95 = output.match(/p95_ms=([0-9.]+)/)?.[1];
if (p95 === undefined || !Number.isFinite(Number(p95))) {
  console.error("image performance probe returned an invalid p95 measurement");
  process.exit(1);
}
if (Number(p95) > thresholdMs) {
  console.error(`viewport image tokenization p95 ${p95}ms exceeds ${thresholdMs}ms`);
  process.exit(1);
}
