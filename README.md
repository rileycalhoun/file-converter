# File Converter

File Converter is a self-contained Tauri desktop application that converts
documents and images to PDF locally. Files are converted on your device and are
not uploaded to a conversion server. Normal operation requires no Internet
connection, account, token, Docker service, or separately installed copy of
LibreOffice.

## Architecture

```text
Tauri UI
   │ raw binary IPC (never JSON/base64 file payloads)
   ▼
Rust ConversionService
   │
   ├── format detection and supported-format registry
   ├── LibreOffice WASM engine ──► isolated reusable Web Worker
   ├── native image-to-PDF engine
   └── PDF passthrough engine
   │
   ▼
Local PDF + SQLite metadata history
```

Rust owns format detection, routing, job lifecycle, cancellation state, output
paths, temporary files, and history. The frontend is a thin Tauri client and
hosts the browser worker required by LibreOffice WASM. Large file payloads cross
the Tauri boundary as raw binary bodies rather than JSON arrays or base64.

LibreOffice work is serialized through one lazily initialized
`WorkerBrowserConverter`. It stays off the UI thread and is reused between jobs.
The package does not currently expose an abort primitive. Cancellation therefore
marks the backend job cancelled immediately and guarantees that a late worker
result cannot be saved; the underlying WASM operation may continue until it
returns.

## Supported inputs

The app displays this list from the backend registry used for routing.

LibreOffice WASM 2.7.2:

- Word and text: DOC, DOCX, ODT, RTF, TXT, HTML/HTM, EPUB
- Spreadsheets: XLS, XLSX, ODS, CSV
- Presentations: PPT, PPTX, ODP
- Drawings/formulas: ODG, ODF

Rust-native image engine:

- PNG, JPEG/JPG, BMP, GIF, TIFF/TIF, WEBP

PDF files use a copy/passthrough engine and are not re-rendered.

This list is intentionally narrower than desktop LibreOffice's import list. A
format is not advertised merely because another LibreOffice build may support
it. In particular, macro-enabled Office files, templates, WPS, WordPerfect,
Visio, Publisher, Pages, Numbers, Keynote, and SVG input are not claimed by the
selected WASM package's declared browser API.

## Storage and history

The application stores its SQLite database and `converted-files` directory in
the operating system's application-data directory. New history rows store
metadata and the output path, not a second PDF BLOB. Schema upgrades add source
path, detected format, engine, sizes, and completion time without recreating the
database.

Legacy `output_data` and `output_base64` columns remain readable so older output
copies can still be restored. New conversions never populate them. If a new
output disappears, the UI offers **Reconvert** when its original source path
still exists; otherwise it reports the file as unavailable.

Each job gets a dedicated application-cache work directory. The original is
never modified. Job artifacts are removed after success, failure, or
cancellation, and stale work directories are cleaned at startup.

## Development

Requirements are a current Rust toolchain, Node.js/npm, and the normal
[Tauri 2 platform prerequisites](https://v2.tauri.app/start/prerequisites/).

```sh
npm install
npm run tauri dev
```

`npm run prepare:libreoffice` copies the package's five required browser assets
into an ignored `public/libreoffice-wasm` directory. `predev` and `prebuild` run
this automatically. The files are served from the application itself; no CDN or
runtime download is used.

The browser build requires `SharedArrayBuffer`. Vite development responses and
packaged Tauri responses set COOP/COEP headers, while the CSP permits local WASM
and local workers without permitting remote network destinations.

## Testing

```sh
cargo test --workspace
npm run build
npm run test:libreoffice
cargo clippy --workspace --all-targets -- -D warnings
```

Rust tests cover detection, registry uniqueness, routing, safe filenames,
metadata-only migration behavior, image conversion, PDF passthrough, failed
history, missing-source reconversion, and legacy BLOB compatibility.

`npm run test:libreoffice` is the heavier local smoke suite. It initializes the
same bundled LibreOffice WASM binary and converts generated DOCX, PPTX, XLSX,
ODT, RTF, and TXT fixtures to PDF. Fixtures under `tests/fixtures` were generated
for this repository and contain no downloaded document content.

## Building installers

```sh
npm install
npm run build
cargo test --workspace
npm run tauri build
```

Installers are written below `target/release/bundle`. Tauri installers are built
on their target operating system; produce separate macOS, Windows, and Linux
artifacts in a platform matrix. The package includes all LibreOffice WASM
runtime assets. End users do not need Node.js, Rust, Python, Docker, Gotenberg,
or LibreOffice. Distribution builds should be signed and, on macOS, notarized
with the platform credentials for the release organization; local development
builds are intentionally unsigned.

The uncompressed LibreOffice runtime adds approximately 236 MiB to the app
bundle (`soffice.wasm` is about 141 MiB and `soffice.data` about 95 MiB).
Installer compression and platform overhead determine final download size.

## Privacy and security

- Source bytes are read from disk by an allowlisted Tauri command and sent only
  to the local application Web Worker.
- The conversion path contains no HTTP client, upload code, gateway URL, token,
  analytics, telemetry, or remote fallback.
- The worker and WASM/data files are same-origin packaged assets.
- LibreOffice runs headlessly in a WASM sandbox; the app does not execute macros
  or expose arbitrary shell commands.
- The CSP does not allow arbitrary remote connections.
- Source documents should still be treated as untrusted input. Keep the pinned
  WASM package updated when security fixes are released.

See [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md) for LibreOffice/MPL and
image/PDF dependency licensing.

## Known limitations

- LibreOffice WASM has a large lazy initialization and memory footprint.
- One LibreOffice conversion runs at a time. Native image and PDF work does not
  depend on that queue.
- WASM conversion receives the source as an `ArrayBuffer`; Tauri raw IPC avoids
  JSON/base64 expansion, but the browser and converter still need memory for the
  input and output buffers.
- The upstream worker API offers coarse lifecycle messages, not true document
  progress, so the UI shows an indeterminate converting state.
- Upstream does not expose active-job abort. Cancelled results are safely
  discarded, but CPU/memory may remain occupied until the current call returns.
- The native image engine converts the primary image/frame to one PDF page.
- Font substitution can change layout when a document's fonts are absent from
  the WASM bundle. CJK font bundles are intentionally not included because of
  their roughly 250 MiB additional size.
- Browser-worker compatibility must be verified on each supported OS WebView as
  part of release testing.
