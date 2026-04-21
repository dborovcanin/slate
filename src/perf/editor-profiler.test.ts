import test from "node:test";
import assert from "node:assert/strict";
import {
  RollingProfilerBuffer,
  percentile,
  summarizeEditorProfilerSamples,
  type EditorProfilerSample,
} from "./editor-profiler.ts";

function sample(
  name: string,
  reason: string,
  durationMs: number,
  metrics: Record<string, number> | null = null,
): EditorProfilerSample {
  return {
    name,
    reason,
    durationMs,
    atMs: 0,
    metrics,
  };
}

test("percentile uses stable interpolation for p50/p95/p99", () => {
  const sorted = [1, 2, 3, 4, 5];
  assert.equal(percentile(sorted, 0), 1);
  assert.equal(percentile(sorted, 50), 3);
  assert.equal(percentile(sorted, 100), 5);
  assert.equal(Number(percentile(sorted, 95).toFixed(2)), 4.8);
  assert.equal(Number(percentile(sorted, 99).toFixed(2)), 4.96);
});

test("rolling buffer truncates oldest samples and tracks dropped count", () => {
  const buffer = new RollingProfilerBuffer(3);
  buffer.push(sample("a", "r1", 1));
  buffer.push(sample("b", "r2", 2));
  buffer.push(sample("c", "r3", 3));
  buffer.push(sample("d", "r4", 4));
  buffer.push(sample("e", "r5", 5));

  const snapshot = buffer.snapshot();
  assert.equal(snapshot.length, 3);
  assert.deepEqual(snapshot.map((entry) => entry.name), ["c", "d", "e"]);
  assert.equal(buffer.droppedCount, 2);
});

test("summary buckets by name and reason with metric maxima and percentiles", () => {
  const summary = summarizeEditorProfilerSamples(
    [
      sample("markdown.decorations.safeBuild", "selectionSet", 2, { lineCount: 10 }),
      sample("markdown.decorations.safeBuild", "selectionSet", 4, { lineCount: 30 }),
      sample("markdown.decorations.safeBuild", "selectionSet", 6, { lineCount: 20 }),
      sample("markdown.decorations.safeBuild", "viewportChanged", 10, { lineCount: 100 }),
      sample("calc.decorations.safeBuild", "docChanged", 8, { lineCount: 50 }),
    ],
    {
      enabled: true,
      capacity: 64,
      dropped: 7,
      sortBy: "p95",
    },
  );

  assert.equal(summary.enabled, true);
  assert.equal(summary.capacity, 64);
  assert.equal(summary.sampleCount, 5);
  assert.equal(summary.dropped, 7);
  assert.equal(summary.buckets.length, 3);

  const selectionBucket = summary.buckets.find((bucket) =>
    bucket.name === "markdown.decorations.safeBuild" && bucket.reason === "selectionSet"
  );
  assert.ok(selectionBucket);
  assert.equal(selectionBucket!.count, 3);
  assert.equal(selectionBucket!.totalMs, 12);
  assert.equal(selectionBucket!.avgMs, 4);
  assert.equal(selectionBucket!.p50Ms, 4);
  assert.equal(Number(selectionBucket!.p95Ms.toFixed(1)), 5.8);
  assert.equal(Number(selectionBucket!.p99Ms.toFixed(2)), 5.96);
  assert.equal(selectionBucket!.metricMax.lineCount, 30);
  assert.equal(Number(selectionBucket!.metricAvg.lineCount.toFixed(2)), 20);
});
