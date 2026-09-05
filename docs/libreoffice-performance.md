# LibreOffice startup measurements and policy

## Reproduce

Run `npm run dev -- --host 127.0.0.1` and open
`http://127.0.0.1:1420/scripts/libreoffice-benchmark.html` in a browser supporting
cross-origin isolation. Click **Run benchmark**. The development-only page uses
the production browser worker and the repository's generated DOCX fixture. It
does not invoke Tauri, save PDFs, or populate history.

The benchmark creates one worker, performs three sequential conversions, checks
each PDF signature, and terminates the worker in `finally`, including on errors.
An overall two-minute deadline forces termination. The reusable harness accepts
2–5 samples and an overall deadline no greater than five minutes. This avoids
unbounded warm-up loops and multiple retained WASM instances.

Record the displayed JSON alongside hardware/RAM, browser version, power mode,
fixture size, cache state, and other workload. Repeat with a freshly launched
browser and with cached assets. “Cold-worker” means a new worker and WASM runtime;
it does not mean the operating system or HTTP cache was cleared. Compare full
sample times and the separate initialization/conversion phases.

## Initial observation: September 4, 2026

The actual browser harness completed in local Chrome 152 on macOS with
cross-origin isolation. Each conversion returned a 10,798-byte PDF.

| Sample | Initialization | Conversion | Total |
| --- | ---: | ---: | ---: |
| Fresh worker | 748.8 ms | 529.2 ms | 1,278.5 ms |
| Same worker, second document | 0.035 ms | 66.7 ms | 66.7 ms |
| Same worker, third document | 0.040 ms | 48.9 ms | 48.9 ms |

A second run on the same page, with a newly created worker and previously
requested assets, measured 384.7 ms initialization and 103.6 ms first conversion
(488.3 ms total), then 34.7 ms and 33.1 ms warm totals. Its PDFs also contained
10,798 bytes. The difference reinforces the need to record cache state rather
than label every fresh worker an uncached application startup.

These are fixture measurements, not a broad performance claim. This run did not
measure retained process memory, an empty asset cache, or the packaged WebView
on the 8 GB target device. The `conversion` phase includes input handling and
completion; the harness uses in-memory substitutes for application IPC/disk I/O.

## Current cold, warm, and idle policy

- Cold: create LibreOffice only when an Office conversion starts. Selecting a
  file does not initialize WASM. PDF passthrough and native image conversion
  remain lazy and never allocate this worker.
- Warm: reuse one healthy worker. The measured fixture benefits substantially
  from reuse, including document/font work beyond the initial runtime setup.
  Do not add a worker pool.
- Idle: keep that worker until the app window closes. There is no new arbitrary
  idle timeout; quantify memory pressure and repeat-use latency before selecting
  a retention period. The benchmark explicitly disposes its worker afterward.
- Failure/cancellation: forcibly terminate and discard the worker, reject active
  work, and create a new instance on the next conversion. Deadlines remain
  bounded. Telemetry callbacks cannot interfere with this behavior.

## Office-selection prewarming decision

Automatic prewarming is **not enabled** by this change. The measured ~385–749 ms
initialization is the maximum portion these samples could hide by initializing
after an Office file is selected. It cannot hide the entire first conversion,
and the local result alone does not establish an acceptable idle-memory cost on
an 8 GB machine.

Before enabling prewarming, benchmark the packaged app on the target device.
Compare Convert-click-to-PDF latency for immediate clicks and realistic
selection-to-click delays, measure worker/process memory before and after
initialization and during idle, and test selecting an Office file followed by a
PDF/image or cancellation. Enable it only if the measured latency benefit and
memory cost justify it. Any future policy must trigger only after inspection
confirms `libreoffice-wasm`, share a single in-flight initialization with Convert,
force termination on cancellation/failure, reject stale callbacks, and avoid
prewarming on launch or for PDF/image-only usage.

## Instrumentation

`LibreOfficeWasmRunner` accepts optional `onTiming` and `now` hooks. Timing events
contain `phase`, `durationMs`, `reusedRuntime`, and `outcome` only; no source
filename, path, document content, or conversion identifier is emitted. The app
does not enable logging or transmit telemetry by default. `dispose()` forcibly
releases both active and idle workers for bounded benchmark ownership.
