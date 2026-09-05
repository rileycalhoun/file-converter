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

### Image PDF quality

Image conversion always uses the preserve-detail policy: it embeds the full
pixel width and height. Compatible baseline 8-bit JFIF JPEGs with no required
orientation or special color handling retain their original compressed stream;
other images use lossless Flate compression after decoding. There is no decoded-size
budget, downsampling, additional lossy JPEG compression, automatic grayscale
conversion, or dithering. A 4000 × 3000 scan therefore stays 4000 × 3000 in the PDF
(with dimensions exchanged when its EXIF orientation requires a quarter turn).

Images are centered on A4 with 10 mm margins, using landscape pages for landscape
images. The nominal 300 DPI sets physical placement for smaller images; larger
images fit inside the margins using a PDF transform while retaining every source
pixel. It is not a resolution cap. There is currently no reduced-quality mode or
user-selectable DPI setting. Output files can be substantially larger than a
source JPEG because preserving decoded detail avoids a second JPEG encoding.
JPEGs requiring EXIF transforms or carrying ICC/Adobe color metadata use the
decoder; transparency in other formats also remains on the decoding path.
The PDF writer currently uses 8-bit image
channels, so this policy preserves spatial detail but does not promise archival
preservation of higher source bit depths or color profiles.

To bound image conversion work, source files are limited to 64 MiB and images to
40 million pixels. The decoding path additionally limits its pixel buffer and
requests a decoder allocation limit of 64 MiB; direct JPEG embedding avoids that
pixel buffer. Limits are checked before pixel decoding, and oversized files fail
with an error asking for a smaller export, rather than silently losing detail.
The source read is bounded even if a file grows during conversion. These are
per-image limits, not a guarantee that total process memory stays below 64 MiB:
decoder scratch space, PDF serialization, and concurrent work use additional
memory. The native PDF serializer still buffers its output.

## Storage and history

Converted PDFs are saved in the same directory as their selected source files.
Collision-safe filenames prevent an existing PDF from being overwritten. The
application stores its SQLite database in the operating system's application-data
directory. New history rows store metadata and the output path, not a second PDF
BLOB. Schema upgrades add source path, detected format, engine, sizes, and
completion time without recreating the database.

History loads when its dialog opens, initially showing 50 entries. **Load more**
appends older entries without rebuilding already displayed rows. Pages use an
indexed timestamp-and-ID cursor, so equal timestamps and deletion of a boundary
row do not skip or repeat older conversions; reopening History refreshes the
newest page. Each request is capped at 100 entries. Database and file-availability
work runs in the bounded background worker, with probes only for returned rows,
and file actions recheck availability when used. Legacy saved-copy flags are read
with the page metadata, without transferring stored PDF contents to the UI.

On macOS, the application home is `~/Library/Application Support/FileConverter`.
Existing history is copied there once from the older identifier-based directory.
Windows and Linux use a `FileConverter` folder under their standard application-data
location.

Legacy `output_data` and `output_base64` columns remain readable so older output
copies can still be restored. New conversions never populate them. If a new
output disappears, the **History** dialog offers **Reconvert** when its original
source path still exists. Legacy entries with stored PDF data can be restored;
otherwise the dialog reports that the output is unavailable. Existing PDFs can
also be opened and history entries can be deleted from the same dialog.

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

The browser build requires `SharedArrayBuffer`. Vite development responses set
COOP/COEP headers. Packaged builds serve the embedded frontend on a random
localhost port with the same isolation headers because macOS WebKit does not
reliably expose `SharedArrayBuffer` to Tauri's custom-protocol origin. Tauri IPC
access is scoped to that exact loopback origin, and the CSP permits local WASM
and local workers without permitting remote network destinations.

## Testing

```sh
cargo test --workspace
npm run build
npm run test:frontend
npm run test:libreoffice
cargo clippy --workspace --all-targets -- -D warnings
```

Rust tests cover detection, registry uniqueness, routing, safe filenames,
metadata-only migration behavior, image conversion, PDF passthrough, failed
history, missing-source reconversion, and legacy BLOB compatibility.
Frontend tests cover non-reentrant conversion state so history actions cannot
replace or clear a conversion that is already running.

`npm run test:libreoffice` is the heavier local smoke suite. It initializes the
same bundled LibreOffice WASM binary and converts generated DOCX, PPTX, XLSX,
ODT, RTF, and TXT fixtures to PDF. Fixtures under `tests/fixtures` were generated
for this repository and contain no downloaded document content.

For bounded measurements using the actual browser worker, see
[LibreOffice startup measurements and policy](docs/libreoffice-performance.md).
The harness distinguishes fresh-worker initialization from warm reuse and does
not save files or history.

## Building installers

```sh
npm install
npm run build
cargo test --workspace
npm run tauri build
```

Installers are written below `target/release/bundle`. Version 1.0 supports macOS
and is distributed as a universal app for Apple Silicon and Intel Macs. The
package includes all LibreOffice WASM runtime assets. End users do not need
Node.js, Rust, Python, Docker, Gotenberg, or LibreOffice. Version 1.0 release
artifacts use an ad-hoc signature and are not notarized, so macOS may require the
app to be approved in **System Settings > Privacy & Security** before its first
launch.

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

## Releases

Pull requests targeting `main` or `master` must pass frontend compilation, the
LibreOffice WASM conversion suite, Rust formatting, compilation, tests, and
Clippy. A tag matching the application version, such as `v1.0.0`, automatically
builds a universal macOS bundle and publishes it to GitHub Releases. The tagged
commit must already belong to `main` or `master`.

## License

File Converter is licensed under the [Apache License 2.0](LICENSE). Bundled
third-party components remain under their respective licenses; see
[THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).
