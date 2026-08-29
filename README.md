# File Converter

A Tauri desktop application that converts Office and OpenDocument files to PDF through a private Gotenberg server. Conversion history and output files stay on the desktop.

## Architecture

```text
Tauri desktop app              Private home gateway          Gotenberg
-----------------             --------------------          ----------
Bundled HTML/CSS/JS  HTTPS    Authenticated Axum API  HTTP   LibreOffice
Rust commands         ----->  Upload proxy             ----> PDF conversion
SQLite history        <-----  PDF response              <---- PDF output
Local output files
```

The conversion request returns the PDF directly. Hosted conversion APIs, PostgreSQL, Redis, browser sessions, WebSockets, polling, and webhooks are not required.

## Server setup

The root Rust package is the authenticated gateway. Docker Compose builds it and runs it alongside the pinned Gotenberg image.

1. Copy `.env.example` to `.env` on the server.
2. Generate `GATEWAY_TOKEN` with `openssl rand -hex 32` and put it in `.env`. Compose passes only this value into the gateway; unrelated values in `.env` are not injected into either container.
3. Build and start both services:

   ```sh
   docker compose up --build -d
   ```

The gateway is published at `http://127.0.0.1:8080` by default. Gotenberg has no host port and is reachable only by the gateway through Compose's private network. In production, publish the gateway through an HTTPS reverse proxy or secure tunnel. Never expose Gotenberg directly to the internet.

Useful server commands:

```sh
docker compose logs -f gateway gotenberg
docker compose down
```

The gateway API is intentionally small:

```text
GET  /health
POST /v1/convert
```

`POST /v1/convert` requires `Authorization: Bearer <GATEWAY_TOKEN>`, accepts one multipart field named `file`, and returns a PDF. Configure upload-size and rate limits at the reverse proxy as well as the gateway's built-in 25 MiB request limit.

Gotenberg 8.34.0 is pinned because it includes linked-content and outbound-request protections for untrusted Office documents. Keep the container updated when newer patched releases are available.

## Desktop development

Requirements: a current Rust toolchain, Node.js, npm, and the normal [Tauri platform prerequisites](https://v2.tauri.app/start/prerequisites/).

```sh
npm install
npm run tauri dev
```

On first launch, open **Settings** and enter the public HTTPS gateway URL and `GATEWAY_TOKEN`. HTTP is accepted only for `localhost` development. The URL is stored in SQLite; the token is stored in the operating system credential vault (macOS Keychain, Windows Credential Manager, or Linux Secret Service). Existing installations automatically migrate and remove a legacy plaintext SQLite token after the credential-vault write succeeds.

The app creates its SQLite database and `converted-files` folder in the operating system's application-data directory. Each converted PDF is also stored as a SQLite BLOB so a missing output file can be recreated after confirmation. The BLOB uses Zstandard compression only when that makes the file smaller; otherwise its exact raw bytes are stored. On upgrade, base64 backups are migrated and existing successful conversions are backed up when their output files still exist. Removing a history entry removes both the output file and its database copy. Supported inputs include DOC, DOCX, PPT, PPTX, XLS, XLSX, ODT, ODP, ODS, RTF, and plain text; output is PDF.

Google Docs and Google Slides are cloud resources rather than uploadable file formats. Export them from Google Drive as DOCX/PPTX and select the exported file in the app. Direct Google Drive export can be added separately later without adding a hosted conversion service.

## Build an installer

```sh
npm run tauri build
```

Installers are written beneath `target/release/bundle`. Build on each target operating system or add a platform matrix to CI when Windows and Linux packages are needed.

## Verification

```sh
npm run build
cargo test --workspace
```

The tests do not call Gotenberg. A live end-to-end conversion requires the container and gateway configuration above.
