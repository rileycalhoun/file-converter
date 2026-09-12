# Native ARM64 Office engine: bounded assessment

## Decision

**Go for a controlled native-engine prototype; no-go for replacing the production
WASM engine yet.** The existing native ARM64 development build converted the six
repository Office fixtures with lower first-use latency and substantially lower
process high-water RSS than the Node WASM comparison. The reused WASM worker was
much faster than restarting the native process. Visual equivalence, a supported
release build, signed packaging, target-device memory behavior, and a resident
native-worker design remain unproven. This issue changes no application engine,
routing, installer, or dependency.

## Reproduce the experiment

On ARM64 macOS, provide an existing native ARM64 LibreOffice executable:

```sh
node scripts/benchmark-native-office.mjs --soffice /absolute/path/LibreOffice.app/Contents/MacOS/soffice
```

No engine is downloaded or installed. The script checks the executable's Mach-O
architecture, records its version, and tests DOCX, PPTX, XLSX, ODT, RTF, and TXT.
Every native fixture uses a private profile and font cache, followed by a new
process reusing that profile. A separate Node child initializes the repository's
pinned LibreOffice WASM package and converts the same fixture twice in one
worker. Input fixtures remain untouched. PDFs and an incremental `report.json`
are retained in the printed temporary artifact directory for examination.

Each subprocess has a 30-second deadline; the experiment has a five-minute
overall budget. Timeout kills only the detached process group created for that
sample. Captured stdout/stderr are capped at 16 KiB each. The version query is
bounded to five seconds. No UNO listener or network service is started.

The native invocation uses the documented `--headless`, `--convert-to`,
`--outdir`, and `-env:UserInstallation` options.
[LibreOffice command-line documentation](https://help.libreoffice.org/latest/en-US/text/shared/guide/start_parameters.html)

## Observed results: September 4, 2026

Host: macOS ARM64, Node 26.7.0. Native build:
`LibreOfficeDev 26.8.0.0.alpha0 2c87e51eeaa2b413ff4ae097b2705eea1995d8e5`.
WASM: the repository's pinned `@matbee/libreoffice-converter` 2.7.2 package.
The successful run had no nonzero engine exits, zero timeouts, and 24 valid PDF-header
outputs: two per engine for each of six formats. Results are also recorded in
[the machine-readable observation](benchmarks/native-office-arm64-2026-09-04.json).

| Fixture | Native fresh profile | Native process restart | WASM init + first conversion | WASM same-worker conversion | Native peak RSS | Node/WASM peak RSS |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| DOCX | 746 ms | 462 ms | 988 ms | 55.6 ms | 151 MiB | 1,472 MiB |
| PPTX | 649 ms | 505 ms | 1,001 ms | 128.1 ms | 128 MiB | 1,744 MiB |
| XLSX | 602 ms | 429 ms | 799 ms | 27.4 ms | 109 MiB | 1,508 MiB |
| ODT | 548 ms | 436 ms | 828 ms | 28.1 ms | 147 MiB | 1,652 MiB |
| RTF | 527 ms | 402 ms | 741 ms | 13.8 ms | 142 MiB | 1,614 MiB |
| TXT | 504 ms | 405 ms | 688 ms | 11.6 ms | 139 MiB | 1,450 MiB |

Native timing includes launch and shutdown. WASM first-use timing is worker
initialization plus first conversion, excluding Node launch/shutdown; its RSS
covers the whole child, including initialization and both conversions. Native
RSS is the larger of its two process runs. `/usr/bin/time -l` high-water RSS is
not summed concurrent process-tree RSS, idle retention, or packaged-WebView
memory. These are single local observations on small generated fixtures, not
statistical estimates or an 8 GB target-device certification. Fresh profiles and
workers do not imply an empty operating-system cache. Native repeat is **not** a
resident warm engine.

The initial restricted run produced PDFs but could not read RSS because macOS
denied `sysctl kern.clockrate`; its wrapper exited nonzero. Those memory-less
samples are excluded from the table. The successful rerun allowed measurement
and used a private writable font cache. The native development build still
emitted fontconfig configuration warnings, which belong in packaging validation.

For context, the separately exercised Chrome browser worker produced DOCX
first-use totals of 488–1,278 ms and reused-worker totals of 33–67 ms under
different cache conditions. The Node memory numbers must not be attributed to
that browser run.

## Output and fidelity

Pypdf extraction found matching page counts, page dimensions, and extracted text
for all six native/WASM fixture pairs. Every output was one page. PPTX used
720 × 540 points; the other formats used 612 × 792 points. Output sizes differed:
native 13,887–18,130 bytes versus WASM 8,090–10,798 bytes. File-size differences
alone do not establish fidelity or quality.

Rendered DOCX and XLSX examples were legible with the same visible content and
overall placement. A concrete PPTX difference was visible: the native output
placed the body sentence toward the left edge of its textbox; WASM centered
that sentence. The title and extracted text matched. This is enough to reject
an assumption of identical rendering even on the simple fixture. The ODT fixture
contains the DOCX sample sentence and adds no independent complex layout case.

These fixtures cover format families, not representative production complexity.
Before a replacement decision, compare user-approved multipage documents,
charts, images, formulas, print areas, embedded/missing fonts, tracked changes,
RTL/CJK text, and page breaks against expected reference output. Record visual
differences and parsing failures rather than relying on `%PDF-` or text equality.

## Packaging, licensing, sandboxing, and maintenance

The locally available native development app occupied about **410 MiB** on disk
(`du -sk`: 419,588 KiB). This is not a compressed installer size. Shipping it
alongside WASM would add another large runtime; an ARM64-only bundle also needs
an explicit story for Intel macOS and other supported platforms. LibreOffice
publishes Apple Silicon builds, but this measured alpha build is not evidence
that a selected supported release has equivalent behavior.
[LibreOffice system requirements](https://www.libreoffice.org/system-requirements/)

Local `codesign` inspection showed an **ad-hoc** signature, no TeamIdentifier,
and no sealed resources on the measured executable. It is not a distribution
candidate as measured. A shipping nested helper/runtime needs a reviewed signing
layout, compatible hardened-runtime entitlements, Developer ID signing, and
notarization/Gatekeeper validation. No signature, entitlement, or system policy
was changed for this experiment.
[Apple distribution signing](https://developer.apple.com/documentation/xcode/creating-distribution-signed-code-for-the-mac),
[Apple notarization](https://developer.apple.com/documentation/security/notarizing-macos-software-before-distribution)

LibreOffice's licensing material includes MPL 2.0 and third-party notices; its
contribution policy also discusses LGPLv3+. A distribution plan must inventory
the exact native build, bundled fonts and libraries, preserve required notices,
and review applicable source/modification obligations. The existing npm package
notices should not be assumed to cover a newly bundled native distribution.
No native runtime is redistributed by this PR.
[LibreOffice licenses](https://www.libreoffice.org/licenses/),
[LibreOffice licensing and third-party information](https://api.libreoffice.org/share/readme/LICENSE.html)

Private profiles isolate configuration and avoid attaching to a user's existing
Office session; **headless mode is not a security sandbox**. A production helper
needs explicit filesystem/network restrictions, macro/external-resource policy,
and tests using staged input/output paths. If App Sandbox is used, embedded
helpers must inherit the containing app's sandbox configuration; selected-file
access and helper entitlements must be verified in the actual signed app.
[Apple sandboxed helper guidance](https://developer.apple.com/documentation/xcode/embedding-a-helper-tool-in-a-sandboxed-app),
[Apple App Sandbox](https://developer.apple.com/documentation/security/protecting-user-data-with-app-sandbox)

The bounded harness's real native DOCX cancellation check stopped the detached
process group after a 50 ms deadline and returned `SIGKILL` at 53.8 ms. Unit tests
also cover normal exit, a hanging child, and a missing executable. This supports
the prototype termination mechanism, but is not an end-to-end app cancellation
test. A production implementation must clean staged output/profile files, avoid
publishing after cancellation, reconcile history, and reject late responses.

Maintaining a resident native engine could improve repeat latency but introduces
profile ownership, IPC, crash recovery, retained memory, and update testing. Do
not expose an unrestricted UNO listener to obtain warm performance. Keep any
future native integration behind a feature flag until those contracts are
tested, and keep WASM available while the candidate is evaluated.

## Gate for a later engine decision

Proceed with a supported ARM64 build and a small supervised native-helper
prototype, then measure launch/first-use, actual resident warm conversion, idle
footprint, and cancellation on the 8 GB target. Require acceptable visual
fidelity on a richer corpus and a signed/notarized packaged-app test before
proposing any default-engine switch. The present evidence justifies that next
experiment, not a production replacement.
