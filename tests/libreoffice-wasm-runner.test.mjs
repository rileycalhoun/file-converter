import assert from "node:assert/strict";
import test from "node:test";
import { LibreOfficeWasmRunner, failWasmConversion, WasmCompletionError } from "../src/libreoffice-wasm-runner.js";
import { ConversionGuard } from "../src/conversion-guard.js";

const task = { conversionId: "job-1", inputFormat: "docx", fileName: "sample.docx" };
const pdf = new TextEncoder().encode("%PDF-test");
const tick = () => new Promise((resolve) => setImmediate(resolve));

class FakeWorker {
  messages = [];
  terminated = false;
  postMessage(message) { this.messages.push(message); }
  terminate() { this.terminated = true; }
  emit(data) { this.onmessage?.({ data }); }
  respond(type, data, request = this.messages.at(-1)) { this.emit({ type, id: request.id, data }); }
}

function setup(options = {}) {
  const workers = [];
  const calls = [];
  const runner = new LibreOfficeWasmRunner({
    invoke: async (command, ...args) => {
      calls.push([command, ...args]);
      return command === "read_conversion_input" ? new Uint8Array([1, 2, 3]) : { status: "finished" };
    },
    createWorker: () => { const worker = new FakeWorker(); workers.push(worker); return worker; },
    isIsolated: () => true,
    wasmPaths: { sofficeJs: "/soffice.js" },
    initializationTimeoutMs: 1000,
    conversionTimeoutMs: 1000,
    ...options,
  });
  return { runner, workers, calls };
}

async function initialize(worker) {
  worker.emit({ type: "loaded" });
  await tick();
  assert.equal(worker.messages.at(-1).type, "init");
  worker.respond("ready");
  await tick();
}

for (const phase of ["loading", "initializing", "converting"]) {
  test(`fatal error during ${phase} rejects promptly, releases guard and recreates worker`, async () => {
    const { runner, workers, calls } = setup();
    const guard = new ConversionGuard();
    const token = guard.begin();
    const pending = runner.convert(task).finally(() => guard.finish(token));
    const rejected = assert.rejects(pending, /worker crashed/);
    const first = workers[0];
    if (phase === "initializing") { first.emit({ type: "loaded" }); await tick(); }
    if (phase === "converting") await initialize(first);
    first.onerror({ message: "worker crashed" });
    await rejected;
    assert.equal(guard.isActive, false);
    assert.equal(first.terminated, true);
    assert.equal(calls.some(([name]) => name === "complete_wasm_conversion"), false);
    const next = runner.convert({ ...task, conversionId: "job-2" });
    assert.equal(workers.length, 2);
    await initialize(workers[1]);
    workers[1].respond("result", pdf);
    assert.deepEqual(await next, { status: "finished" });
  });
}

for (const phase of ["loading", "initializing", "converting"]) {
  test(`${phase} deadline terminates an unresponsive worker`, async () => {
    const { runner, workers } = setup({ initializationTimeoutMs: 30, conversionTimeoutMs: 30 });
    const pending = runner.convert(task);
    const rejected = assert.rejects(pending, /timed out/);
    if (phase === "initializing") { workers[0].emit({ type: "loaded" }); await tick(); }
    if (phase === "converting") await initialize(workers[0]);
    await rejected;
    assert.equal(workers[0].terminated, true);
  });
}

test("cancellation rejects immediately and ignores old worker messages after retry", async () => {
  const { runner, workers, calls } = setup();
  const progress = [];
  const first = runner.convert(task, (value) => progress.push(value));
  const rejected = assert.rejects(first, { name: "AbortError" });
  await initialize(workers[0]);
  const oldMessage = workers[0].onmessage;
  const oldRequest = workers[0].messages.at(-1);
  runner.cancel();
  await rejected;
  assert.equal(workers[0].terminated, true);
  const next = runner.convert({ ...task, conversionId: "job-2" });
  await initialize(workers[1]);
  oldMessage({ data: { type: "result", id: oldRequest.id, data: pdf } });
  oldMessage({ data: { type: "progress", id: oldRequest.id, progress: { message: "stale" } } });
  assert.equal(progress.length, 0);
  assert.equal(calls.some(([name]) => name === "complete_wasm_conversion"), false);
  workers[1].respond("result", pdf);
  await next;
  assert.equal(calls.filter(([name]) => name === "complete_wasm_conversion").length, 1);
  assert.equal(calls.at(-1)[2].headers["x-conversion-id"], "job-2");
});

test("cancel during input read releases guard and cannot start a late conversion", async () => {
  let resolveRead;
  const { runner, workers } = setup({ invoke: () => new Promise((resolve) => { resolveRead = resolve; }) });
  const guard = new ConversionGuard();
  const token = guard.begin();
  const pending = runner.convert(task).finally(() => guard.finish(token));
  const rejected = assert.rejects(pending, { name: "AbortError" });
  await initialize(workers[0]);
  runner.cancel();
  await rejected;
  assert.equal(guard.isActive, false);
  resolveRead([1, 2, 3]);
  await tick();
  assert.deepEqual(workers[0].messages.map(({ type }) => type), ["init"]);
});

test("cancel while loading rejects and permits a new attempt", async () => {
  const { runner, workers } = setup();
  const pending = runner.convert(task);
  const rejected = assert.rejects(pending, { name: "AbortError" });
  runner.cancel();
  await rejected;
  assert.equal(workers[0].terminated, true);
  const next = runner.convert(task);
  const nextRejected = assert.rejects(next, { name: "AbortError" });
  runner.cancel();
  await nextRejected;
  assert.equal(workers.length, 2);
});

test("message decoding errors and worker-reported errors reject the current conversion", async () => {
  for (const kind of ["messageerror", "error"]) {
    const { runner, workers } = setup();
    const pending = runner.convert(task);
    const rejected = assert.rejects(pending, kind === "error" ? /conversion failure/ : /unreadable message/);
    await initialize(workers[0]);
    if (kind === "error") workers[0].emit({ type: "error", id: workers[0].messages.at(-1).id, error: "conversion failure" });
    else workers[0].onmessageerror();
    await rejected;
    assert.equal(workers[0].terminated, true);
  }
});

test("successful conversions reuse the worker and route progress only to the current job", async () => {
  const { runner, workers, calls } = setup();
  const firstProgress = [];
  const first = runner.convert(task, (p) => firstProgress.push(p));
  await initialize(workers[0]);
  const firstRequest = workers[0].messages.at(-1);
  workers[0].respond("result", pdf);
  await first;
  const secondProgress = [];
  const second = runner.convert({ ...task, conversionId: "job-2" }, (p) => secondProgress.push(p));
  await tick();
  workers[0].emit({ type: "progress", id: firstRequest.id, progress: { message: "stale" } });
  workers[0].emit({ type: "progress", id: workers[0].messages.at(-1).id, progress: { message: "current" } });
  workers[0].respond("result", pdf);
  await second;
  assert.equal(workers.length, 1);
  assert.equal(firstProgress.length, 0);
  assert.equal(secondProgress[0].message, "current");
  assert.equal(calls.at(-1)[1], pdf);
});

test("an idle worker crash invalidates the cached runtime before the next job", async () => {
  const { runner, workers } = setup();
  const first = runner.convert(task);
  await initialize(workers[0]);
  workers[0].respond("result", pdf);
  await first;
  workers[0].onerror({ message: "idle crash" });
  assert.equal(workers[0].terminated, true);
  const next = runner.convert(task);
  await initialize(workers[1]);
  workers[1].respond("result", pdf);
  await next;
  assert.equal(workers.length, 2);
});

test("a synchronous postMessage failure rejects, terminates and clears the pending request", async () => {
  const { runner, workers } = setup();
  const pending = runner.convert(task);
  const rejected = assert.rejects(pending, /clone failed/);
  workers[0].postMessage = () => { throw new Error("clone failed"); };
  workers[0].emit({ type: "loaded" });
  await rejected;
  assert.equal(workers[0].terminated, true);
  const next = runner.convert(task);
  await initialize(workers[1]);
  workers[1].respond("result", pdf);
  await next;
});

for (const saveError of ["Could not save PDF: No space left on device", new Error("Could not save PDF: Permission denied")]) {
  test(`completion preserves ${String(saveError)} without a second terminal transition`, async () => {
    const history = new Map();
    const calls = [];
    let failSave = true;
    const invoke = async (command, payload, options) => {
      calls.push(command);
      if (command === "read_conversion_input") return [1, 2, 3];
      if (command === "complete_wasm_conversion") {
        const id = options.headers["x-conversion-id"];
        history.set(id, { status: failSave ? "failed" : "finished", error: failSave ? String(saveError) : null });
        if (failSave) throw saveError;
        return history.get(id);
      }
      throw new Error("The conversion is no longer active.");
    };
    const { runner, workers } = setup({ invoke });
    const guard = new ConversionGuard();
    const token = guard.begin();
    const pending = runner.convert(task)
      .catch((error) => failWasmConversion(invoke, task.conversionId, error))
      .finally(() => guard.finish(token));
    const rejected = assert.rejects(pending, (error) => error === saveError);
    await initialize(workers[0]);
    workers[0].respond("result", pdf);
    await rejected;
    assert.equal(guard.isActive, false);
    assert.deepEqual(history.get(task.conversionId), { status: "failed", error: String(saveError) });
    assert.deepEqual(calls, ["read_conversion_input", "complete_wasm_conversion"]);
    failSave = false;
    const next = runner.convert({ ...task, conversionId: "job-2" });
    await initialize(workers[1]);
    workers[1].respond("result", pdf);
    assert.equal((await next).status, "finished");
  });
}

test("worker failures still persist one failed history record", async () => {
  const { runner, workers, calls } = setup();
  const pending = runner.convert(task)
    .catch((error) => failWasmConversion(runner.invoke, task.conversionId, error));
  await initialize(workers[0]);
  workers[0].onerror({ message: "Worker crashed\nprivate stack detail" });
  await pending;
  assert.deepEqual(calls.at(-1), ["fail_wasm_conversion", { id: task.conversionId, error: "Worker crashed" }]);
  assert.equal(calls.filter(([command]) => command === "fail_wasm_conversion").length, 1);
});

test("a deadline after completion begins never issues a second backend transition", async () => {
  const calls = [];
  const invoke = async (command) => {
    calls.push(command);
    if (command === "read_conversion_input") return [1, 2, 3];
    return new Promise(() => {});
  };
  const { runner, workers } = setup({ invoke, conversionTimeoutMs: 30 });
  const pending = runner.convert(task).catch(async (error) => {
    assert.ok(error instanceof WasmCompletionError);
    return failWasmConversion(invoke, task.conversionId, error);
  });
  const rejected = assert.rejects(pending, /timed out/);
  await initialize(workers[0]);
  workers[0].respond("result", pdf);
  await rejected;
  assert.deepEqual(calls, ["read_conversion_input", "complete_wasm_conversion"]);
});

test("phase telemetry distinguishes cold/warm reuse without document identifiers", async () => {
  const timings = [];
  const { runner, workers } = setup({ onTiming: (timing) => timings.push(timing) });
  const first = runner.convert(task);
  await initialize(workers[0]);
  workers[0].respond("result", pdf);
  await first;
  const next = runner.convert(task);
  await tick();
  workers[0].respond("result", pdf);
  await next;
  assert.deepEqual(timings.map(({ phase, reusedRuntime }) => [phase, reusedRuntime]), [
    ["initialization", false], ["conversion", true], ["initialization", true], ["conversion", true],
  ]);
  assert.ok(timings.every((timing) => timing.outcome === "completed" && timing.durationMs >= 0));
  assert.ok(timings.every((timing) => !JSON.stringify(timing).includes(task.conversionId)));
  runner.dispose();
  assert.equal(workers[0].terminated, true);
});

test("an optional telemetry observer cannot break conversion or cancellation", async () => {
  const { runner, workers } = setup({ onTiming: () => { throw new Error("observer failed"); } });
  const first = runner.convert(task);
  await initialize(workers[0]);
  workers[0].respond("result", pdf);
  await first;
  const next = runner.convert(task);
  const rejected = assert.rejects(next, { name: "AbortError" });
  runner.dispose();
  await rejected;
});
