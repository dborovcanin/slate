import { statSync } from "node:fs";
import { resolve } from "node:path";

// The wasm bundle is compiled on every cold start of the UI, so its size is a
// startup cost, not just a download cost. This guards against silently linking
// a large dependency into the shared core again.
const WASM_PATH = resolve("pkg/editor-core/editor_core_bg.wasm");

const maxBytes = Number(process.env.NOTE_WASM_MAX_BYTES ?? 1_600_000);

let size;
try {
  size = statSync(WASM_PATH).size;
} catch {
  console.error(`wasm bundle not found at ${WASM_PATH}`);
  console.error("run `npm run build:wasm` first");
  process.exit(1);
}

const kib = (bytes) => `${(bytes / 1024).toFixed(1)} KiB`;
const headroomPct = ((1 - size / maxBytes) * 100).toFixed(1);

console.log(`wasm: ${WASM_PATH}`);
console.log(`size: ${size} bytes (${kib(size)})`);
console.log(`budget: ${maxBytes} bytes (${kib(maxBytes)})`);

if (size > maxBytes) {
  console.error(
    `FAIL wasm bundle is ${kib(size - maxBytes)} over budget. ` +
      "Check for a newly linked dependency, or raise NOTE_WASM_MAX_BYTES deliberately.",
  );
  process.exit(1);
}

console.log(`PASS ${headroomPct}% headroom`);
