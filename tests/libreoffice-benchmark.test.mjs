import assert from "node:assert/strict";
import test from "node:test";
import { benchmarkLibreOffice } from "../scripts/libreoffice-benchmark.js";

test("benchmark uses one worker for cold/warm samples and never saves files", async () => {
  let creates = 0;
  let disposals = 0;
  const result = await benchmarkLibreOffice({ input: [1, 2], wasmPaths: {},
    createRunner: (options) => {
      creates++;
      let count = 0;
      return {
        dispose() { disposals++; },
        async convert(task) {
          assert.deepEqual(await options.invoke("read_conversion_input", { id: task.conversionId }), new Uint8Array([1, 2]));
          options.onTiming({ phase: "initialization", reusedRuntime: count++ > 0 });
          return options.invoke("complete_wasm_conversion", new TextEncoder().encode("%PDF-test"));
        },
      };
    },
  });
  assert.equal(creates, 1);
  assert.equal(disposals, 1);
  assert.deepEqual(result.samples.map((s) => s.runtime), ["cold-worker", "warm-worker", "warm-worker"]);
  assert.deepEqual(result.samples.map((s) => s.phases[0].reusedRuntime), [false, true, true]);
  assert.ok(result.samples.every((s) => s.outputBytes === 9));
});

test("the overall deadline forcibly disposes a hung benchmark", async () => {
  let rejectConversion;
  let disposed = false;
  await assert.rejects(benchmarkLibreOffice({ input: [1], wasmPaths: {}, deadlineMs: 20,
    createRunner: () => ({
      convert: () => new Promise((_, reject) => { rejectConversion = reject; }),
      dispose: () => { disposed = true; rejectConversion?.(new Error("stopped")); },
    }),
  }), /overall deadline/);
  assert.equal(disposed, true);
});

test("invalid output and invalid benchmark bounds fail without retaining a worker", async () => {
  let disposed = false;
  await assert.rejects(benchmarkLibreOffice({ input: [1], wasmPaths: {},
    createRunner: (options) => ({
      convert: () => options.invoke("complete_wasm_conversion", new Uint8Array([1, 2])),
      dispose: () => { disposed = true; },
    }),
  }), /did not return a PDF/);
  assert.equal(disposed, true);
  for (const options of [{ runs: 1 }, { runs: 6 }, { deadlineMs: 0 }, { deadlineMs: 300001 }]) {
    await assert.rejects(benchmarkLibreOffice(options), /runs|deadline/);
  }
});
