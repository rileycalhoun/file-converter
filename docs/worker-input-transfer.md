# Browser worker input ownership

The pinned `@matbee/libreoffice-converter` 2.7.2 browser client clones input in
`sendMessage` before transferring it. The application's supervised adapter uses
the pinned worker's `init`/`convert` protocol directly, so no package patch or
private converter fields are needed. Upgrading the dependency requires checking
that protocol and rerunning the browser fixture benchmark.

`prepareWorkerInput` defaults to a private copy. Passing
`{ transferOwnership: true }` reuses a full, exclusively owned `ArrayBuffer` or
view covering that entire buffer. `postMessage` transfers and detaches it. The
caller must not access or reuse that buffer afterward. This is an explicit
ownership contract; JavaScript cannot determine whether other references exist.

`LibreOfficeWasmRunner` exposes this as `transferInputOwnership`, defaulting to
false. The application opts in because each `read_conversion_input` call returns
fresh private IPC bytes used only by that conversion. The supervised adapter
already avoided copying a raw `ArrayBuffer`; this change additionally avoids
`new Uint8Array(existingTypedArray)` copying when the response is a typed array,
and makes ownership rules explicit and tested.

Partial views are always copied into an exact-length buffer, even with ownership
enabled, so leading/trailing backing bytes cannot reach the worker. Shared
buffers are copied into transferable ordinary buffers. Array input requires one
fresh byte allocation. Empty, detached, and unsupported input are rejected.

Cancellation is checked after the asynchronous IPC read and before preparing or
transferring input. If cancellation wins that race, a late response is neither
transferred nor detached. Every retry reads fresh input; it never reuses a
detached buffer. Worker failures/cancellation still terminate the worker and
reject stale results.

## Bounded benchmark

Run `node --expose-gc scripts/benchmark-input-transfer.mjs`. The fixed workload
compares five copy-and-transfer samples with five ownership-transfer samples at
8 MiB and 32 MiB. Allocation/filling and optional garbage collection happen
outside the measured region. It checks received bytes and actual detachment.

Observed September 4, 2026 on local macOS arm64, Node 26.7.0:

| Input | Median copy + transfer | Median owned transfer |
| --- | ---: | ---: |
| 8 MiB | 0.216 ms | 0.0158 ms |
| 32 MiB | 0.712 ms | 0.0100 ms |

This is a buffer-preparation/structured-clone microbenchmark, not an end-to-end
PDF or packaged-WebView measurement. Its main structural benefit is eliminating
one input-sized allocation for eligible typed-array responses. IPC reading and
LibreOffice's own virtual-filesystem copy remain. No process-memory reduction
percentage is inferred from these timings.

The separate `scripts/libreoffice-benchmark.html` harness uses the application's
opt-in mode, allocating fresh private fixture bytes for each simulated IPC read
so repeated benchmark runs cannot reuse detached source data.

The opt-in path was also exercised in local Chrome 152: one fresh and two reused
worker conversions all returned 10,798-byte PDFs. This verifies repeated real
browser-worker conversion after input detachment; it is not used as a claimed
end-to-end speedup because cache/runtime conditions differ between runs.
