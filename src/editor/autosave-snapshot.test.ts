import assert from "node:assert/strict";
import test from "node:test";
import { EditorState } from "@codemirror/state";
import {
  AutosaveSnapshotTracker,
  type AutosaveSnapshotSample,
} from "./autosave-snapshot.ts";

test("autosave snapshot serializes small docs inline and caches result", async () => {
  const samples: AutosaveSnapshotSample[] = [];
  const doc = EditorState.create({ doc: "alpha\nbeta" }).doc;
  const tracker = new AutosaveSnapshotTracker({
    inlineMaxLength: 100,
    onSample: (sample) => samples.push(sample),
  });
  tracker.markDirty(doc);

  const first = await tracker.bodyForDoc(doc);
  const second = await tracker.bodyForDoc(doc);

  assert.equal(first.body, "alpha\nbeta");
  assert.equal(first.source, "inline");
  assert.equal(second.body, "alpha\nbeta");
  assert.equal(second.source, "cache");
  assert.equal(samples.length, 1);
  assert.equal(samples[0]!.name, "editor.autosave.snapshot");
  assert.equal(samples[0]!.reason, "inline");
  assert.equal(samples[0]!.metrics.lineCount, 2);
});

test("autosave snapshot chunks large docs through the background path", async () => {
  const samples: AutosaveSnapshotSample[] = [];
  let yields = 0;
  const doc = EditorState.create({ doc: "one\ntwo\nthree\nfour\nfive" }).doc;
  const tracker = new AutosaveSnapshotTracker({
    inlineMaxLength: 5,
    chunkLines: 2,
    yieldToMain: async () => {
      yields += 1;
    },
    onSample: (sample) => samples.push(sample),
  });
  tracker.markDirty(doc);

  const snapshot = await tracker.bodyForDoc(doc);
  const cached = await tracker.bodyForDoc(doc);

  assert.equal(snapshot.body, "one\ntwo\nthree\nfour\nfive");
  assert.equal(snapshot.source, "background");
  assert.equal(cached.source, "cache");
  assert.equal(yields, 2);
  assert.equal(samples.length, 1);
  assert.equal(samples[0]!.reason, "background");
  assert.equal(samples[0]!.metrics.chunkLines, 2);
});

test("autosave snapshot ignores stale background work after a newer edit", async () => {
  const firstDoc = EditorState.create({ doc: "old\nold\nold" }).doc;
  const secondDoc = EditorState.create({ doc: "new\nnew\nnew" }).doc;
  const tracker = new AutosaveSnapshotTracker({
    inlineMaxLength: 1,
    chunkLines: 1,
    yieldToMain: async () => {},
  });

  tracker.markDirty(firstDoc);
  tracker.markDirty(secondDoc);

  const snapshot = await tracker.bodyForDoc(secondDoc);
  const cached = await tracker.bodyForDoc(secondDoc);

  assert.equal(snapshot.body, "new\nnew\nnew");
  assert.equal(snapshot.version, tracker.version);
  assert.equal(cached.source, "cache");
  assert.equal(cached.body, "new\nnew\nnew");
});
