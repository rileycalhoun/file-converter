import { performance } from "node:perf_hooks";
import { prepareWorkerInput } from "../src/worker-input.js";

// Fixed bounds: two sizes, five samples each, one buffer transfer per sample.
// Run with --expose-gc to collect discarded samples outside the measured region.
const results = [];
structuredClone(new Uint8Array([1]));
for (const sizeMiB of [8, 32]) {
  for (const transferOwnership of [false, true]) {
    const samples = [];
    for (let index = 0; index < 5; index++) {
      globalThis.gc?.();
      const source = new Uint8Array(sizeMiB * 1024 * 1024).fill(7);
      const started = performance.now();
      const input = prepareWorkerInput(source, { transferOwnership });
      const prepared = performance.now();
      const received = structuredClone(input, { transfer: [input.buffer] });
      const finished = performance.now();
      if (received.length !== sizeMiB * 1024 * 1024 || received[0] !== 7 || received.at(-1) !== 7) throw new Error("Transfer corrupted input.");
      if (transferOwnership && source.byteLength !== 0) throw new Error("Owned input was not transferred.");
      samples.push({ prepareMs: prepared - started, transferMs: finished - prepared, totalMs: finished - started });
    }
    const median = (field) => [...samples].sort((a, b) => a[field] - b[field])[2][field];
    results.push({ sizeMiB, mode: transferOwnership ? "owned-transfer" : "copy-transfer",
      medianPrepareMs: median("prepareMs"), medianTransferMs: median("transferMs"), medianTotalMs: median("totalMs"), samples });
  }
}
console.log(JSON.stringify({ node: process.version, platform: process.platform, arch: process.arch,
  note: "Buffer preparation/structured-clone microbenchmark; excludes file IPC, WASM, and PDF conversion. Not a packaged-WebView memory measurement.", results }, null, 2));
