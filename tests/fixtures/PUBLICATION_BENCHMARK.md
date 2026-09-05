# PDF publication benchmark

Run the deterministic file-transport benchmark from the repository root:

```sh
cargo test -p file-converter-desktop --lib --offline benchmark_pdf_publication -- --ignored --nocapture
```

The ignored test generates a 32 MiB synthetic PDF-like payload in a temporary
directory, executes three warm-cache trials, verifies the resulting bytes, and
removes its artifacts. The payload tests file transport, not PDF rendering. It
times the previous source → cache → destination pipeline, direct passthrough
publication, and publication of an already generated native cache file. Fixture
generation and byte verification are outside the timed sections; each path
synchronizes the final output before returning.

To measure an external volume, set `FILE_CONVERTER_BENCH_DESTINATION` to an
existing writable directory on that volume before running the command. The
test creates and cleans up its own temporary directory there. Its source/cache
remain in the system temporary directory.

## Local sample

September 4, 2026, local macOS filesystem, debug test build, three trials:

| Path | Logical bytes copied | Median time |
| --- | ---: | ---: |
| Previous two-copy passthrough pipeline | 67,108,864 | 3.563 ms |
| Direct destination passthrough | 33,554,432 | 4.295 ms |
| Already-staged native PDF publication | 0 | 4.046 ms |

These results establish fewer logical copies, not a local latency speedup. APFS
can implement file copies as inexpensive clones, so directory creation,
synchronization, atomic publication, and normal timing variation can dominate.
The native-publication row excludes generating its cache file and is not an
end-to-end conversion time. Logical byte totals are not measurements of physical
disk traffic. No MacBook Neo or external-volume performance claim is made.

Passthrough creates an independent destination file or filesystem clone; it
never hard-links the user's original. The publisher uses an exclusively owned
destination-side staging directory so the copy target starts absent, preserving
APFS clone support. Browser PDF bytes go directly to destination-side staging.
Native staged PDFs use a no-clobber hard link when supported, with a safe
destination-side copy fallback on cross-device or unsupported-link filesystems.
All paths use the same atomic no-overwrite publication and owned-artifact
rollback logic.
