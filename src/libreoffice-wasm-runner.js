// This small adapter owns the pinned LibreOffice worker's init/convert protocol.
// The package's browser client does not reject requests on fatal worker errors,
// and its graceful destroy waits for a response that a stuck worker cannot send.
export class WasmCompletionError extends Error {
  constructor(cause) {
    super(cause instanceof Error ? cause.message : String(cause), { cause });
    this.name = "WasmCompletionError";
  }
}

export async function failWasmConversion(invoke, id, error) {
  // Completion consumes the pending backend job and owns its terminal status.
  // A second failure command would hide the save error behind a stale-job error.
  if (error instanceof WasmCompletionError) throw error.cause;
  const message = error instanceof Error ? error.message : String(error);
  return invoke("fail_wasm_conversion", { id, error: message.replace(/\n.*/s, "").slice(0, 600) });
}

export class LibreOfficeWasmRunner {
  #session = null;
  #active = null;
  #nextId = 0;

  constructor({ invoke, wasmPaths, createWorker = () => new Worker("/libreoffice-wasm/browser.worker.global.js"),
    isIsolated = () => globalThis.crossOriginIsolated && typeof SharedArrayBuffer !== "undefined",
    initializationTimeoutMs = 180_000, conversionTimeoutMs = 180_000,
    onTiming = () => {}, now = () => performance.now() }) {
    this.invoke = invoke;
    this.wasmPaths = wasmPaths;
    this.createWorker = createWorker;
    this.isIsolated = isIsolated;
    this.initializationTimeoutMs = initializationTimeoutMs;
    this.conversionTimeoutMs = conversionTimeoutMs;
    this.onTiming = onTiming;
    this.now = now;
  }

  async convert(task, onProgress = () => {}) {
    if (this.#active) throw new Error("Another conversion is already running.");
    if (!this.isIsolated()) {
      throw new Error("The local LibreOffice runtime requires cross-origin isolation, but this app window is not isolated.");
    }
    const job = { onProgress, error: null, reject: null, completing: false };
    const interrupted = new Promise((_, reject) => { job.reject = reject; });
    job.interrupted = interrupted;
    this.#active = job;
    const work = async () => {
      await this.#withDeadline(async () => {
        if (!this.#session) this.#createSession();
        const session = this.#session;
        await session.loaded;
        this.#assertActive(job);
        if (!session.ready) {
          await this.#request(session, "init", { ...this.wasmPaths, verbose: false }, "ready");
          this.#assertActive(job);
          session.ready = true;
        }
      }, this.initializationTimeoutMs, "initialization", job);
      this.#assertActive(job);
      return this.#withDeadline(async () => {
        const input = new Uint8Array(await this.invoke("read_conversion_input", { id: task.conversionId }));
        this.#assertActive(job);
        const data = await this.#request(this.#session, "convert", {
          inputData: input, inputExt: task.inputFormat, outputFormat: "pdf", filterOptions: "",
        }, "result", [input.buffer]);
        // A cancellation, timeout or replaced worker must never publish a late PDF.
        this.#assertActive(job);
        job.completing = true;
        const result = await this.invoke("complete_wasm_conversion", data, {
          headers: { "x-conversion-id": task.conversionId },
        });
        this.#assertActive(job);
        return result;
      }, this.conversionTimeoutMs, "conversion", job);
    };
    try {
      return await Promise.race([work(), interrupted]);
    } catch (error) {
      this.#stop(error);
      if (job.completing && error?.name !== "AbortError") throw new WasmCompletionError(error);
      throw error;
    } finally {
      if (this.#active === job) this.#active = null;
    }
  }

  cancel() {
    if (!this.#active) return;
    const error = new Error("Conversion cancelled. No output file was saved.");
    error.name = "AbortError";
    this.#stop(error);
  }

  dispose() {
    const error = new Error("The local LibreOffice runtime was disposed.");
    error.name = "AbortError";
    this.#stop(error);
  }

  #assertActive(job) {
    if (job.error) throw job.error;
    if (this.#active !== job) throw new Error("Discarded stale conversion result.");
  }

  async #withDeadline(operation, timeoutMs, phase, job) {
    const started = this.now();
    const reusedRuntime = Boolean(this.#session?.ready);
    let outcome = "failed";
    const timer = setTimeout(() => {
      if (this.#active === job) this.#stop(new Error(`The local LibreOffice ${phase} timed out. Please try again.`));
    }, timeoutMs);
    try {
      const result = await Promise.race([operation(), job.interrupted]);
      outcome = "completed";
      return result;
    } finally {
      clearTimeout(timer);
      // Timing observers must never affect conversion or expose document names.
      try {
        this.onTiming({ phase, durationMs: this.now() - started, reusedRuntime,
          outcome: job.error?.name === "AbortError" ? "cancelled" : outcome });
      } catch { /* Diagnostics are optional. */ }
    }
  }

  #createSession() {
    const worker = this.createWorker();
    const session = { worker, ready: false, pending: new Map() };
    session.loaded = new Promise((resolve, reject) => {
      session.resolveLoaded = resolve;
      session.rejectLoaded = reject;
    });
    this.#session = session;
    worker.onmessage = ({ data }) => {
      if (this.#session !== session || !data) return;
      if (data.type === "loaded") {
        session.resolveLoaded();
        return;
      }
      const request = session.pending.get(data.id);
      if (!request) return;
      if (data.type === "progress") {
        this.#active?.onProgress({ ...data.progress, phase: session.ready ? "converting" : "initializing" });
      } else if (data.type === "error") {
        this.#stop(new Error(data.error || "The local LibreOffice worker failed."));
      } else if (data.type === request.responseType) {
        session.pending.delete(data.id);
        request.resolve(data.data);
      }
    };
    worker.onerror = (event) => {
      if (this.#session !== session) return;
      event.preventDefault?.();
      this.#stop(new Error(event.message || "The local LibreOffice worker stopped unexpectedly."));
    };
    worker.onmessageerror = () => {
      if (this.#session === session) this.#stop(new Error("The local LibreOffice worker returned an unreadable message."));
    };
  }

  #request(session, type, payload, responseType, transfer = []) {
    return new Promise((resolve, reject) => {
      const id = ++this.#nextId;
      session.pending.set(id, { resolve, reject, responseType });
      session.worker.postMessage({ type, id, ...payload }, transfer);
    });
  }

  #stop(error) {
    const job = this.#active;
    if (job && !job.error) {
      job.error = error;
      job.reject(error);
    }
    const session = this.#session;
    this.#session = null;
    if (!session) return;
    session.worker.onmessage = null;
    session.worker.onerror = null;
    session.worker.onmessageerror = null;
    session.worker.terminate();
    session.rejectLoaded(error);
    for (const request of session.pending.values()) request.reject(error);
    session.pending.clear();
  }
}
