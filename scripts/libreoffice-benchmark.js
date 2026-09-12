import { LibreOfficeWasmRunner } from "../src/libreoffice-wasm-runner.js";

// Runs against the actual browser worker; no Tauri IPC, output files, or history.
export async function benchmarkLibreOffice({ input, inputFormat = "docx", wasmPaths,
  runs = 3, deadlineMs = 120_000, onSample = () => {},
  createRunner = (options) => new LibreOfficeWasmRunner(options) }) {
  if (!Number.isInteger(runs) || runs < 2 || runs > 5) throw new Error("Use 2–5 runs to compare cold and warm workers.");
  if (!Number.isFinite(deadlineMs) || deadlineMs <= 0 || deadlineMs > 300_000) throw new Error("Use a deadline of up to five minutes.");
  const samples = [];
  let phases = [];
  let expired = false;
  const started = performance.now();
  const runner = createRunner({ wasmPaths, initializationTimeoutMs: deadlineMs, conversionTimeoutMs: deadlineMs, transferInputOwnership: true,
    onTiming: (timing) => phases.push(timing),
    invoke: async (command, data) => {
      if (command === "read_conversion_input") return new Uint8Array(input).slice();
      if (command === "complete_wasm_conversion") {
        const bytes = new Uint8Array(data);
        if (new TextDecoder().decode(bytes.subarray(0, 5)) !== "%PDF-") throw new Error("Worker did not return a PDF.");
        return { outputBytes: bytes.byteLength };
      }
      throw new Error(`Unexpected benchmark command: ${command}`);
    },
  });
  const timer = setTimeout(() => { expired = true; runner.dispose(); }, deadlineMs);
  try {
    for (let index = 0; index < runs; index++) {
      if (expired) throw new Error("LibreOffice benchmark exceeded its overall deadline.");
      phases = [];
      const sampleStarted = performance.now();
      const result = await runner.convert({ conversionId: `benchmark-${index}`, inputFormat });
      const sample = { runtime: index === 0 ? "cold-worker" : "warm-worker", phases,
        totalMs: performance.now() - sampleStarted, outputBytes: result.outputBytes };
      samples.push(sample);
      onSample(sample);
    }
    return { totalMs: performance.now() - started, samples,
      note: "Cold-worker timing may use cached HTTP/filesystem assets. Memory and packaged WebView performance require separate measurements." };
  } catch (error) {
    if (expired) throw new Error("LibreOffice benchmark exceeded its overall deadline.", { cause: error });
    throw error;
  } finally {
    clearTimeout(timer);
    runner.dispose();
  }
}
