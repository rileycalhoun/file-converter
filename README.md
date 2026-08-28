# Simple File Converter

A Tauri desktop application with local conversion history and a private Rust gateway for CloudConvert. The CloudConvert API key exists only on the gateway machine; it is never compiled into or sent to the desktop application.

## Architecture

```text
Tauri desktop app              Private home gateway          CloudConvert
-----------------             --------------------          ------------
Bundled HTML/CSS/JS  HTTPS    Axum API             HTTPS    Jobs API
Rust commands         ----->  API key                 ---->  Conversion
SQLite history        <-----  Status/download proxy   <----  Output
Local output files
```

The desktop polls the gateway for completion. PostgreSQL, Redis, browser sessions, WebSockets, and inbound desktop webhooks are not required.

## Gateway setup

The root Rust package is the gateway.

1. Copy `.env.example` to `.env` on the gateway machine.
2. Set `CLOUDCONVERT_API_KEY`.
3. Generate `GATEWAY_TOKEN` with `openssl rand -hex 32`.
4. Start the service:

   ```sh
   cargo run --release -p file-converter-gateway
   ```

For local development, the default address is `http://127.0.0.1:8080`. In production, keep that listener private and publish it through an HTTPS reverse proxy or secure tunnel. Only the gateway HTTP port should be exposed; the gateway has no database.

The gateway API is intentionally small:

```text
GET  /health
POST /v1/conversions
GET  /v1/conversions/{cloudconvert_job_id}
GET  /v1/conversions/{cloudconvert_job_id}/download
```

All `/v1` routes require `Authorization: Bearer <GATEWAY_TOKEN>`. Configure upload-size and rate limits at the reverse proxy as well as the gateway's built-in 25 MiB request limit.

## Desktop development

Requirements: a current Rust toolchain, Node.js, npm, and the normal [Tauri platform prerequisites](https://v2.tauri.app/start/prerequisites/).

```sh
npm install
npm run tauri dev
```

On first launch, open **Settings** and enter the public HTTPS gateway URL and `GATEWAY_TOKEN`. HTTP is accepted only for `localhost` development. The URL is stored in SQLite; the token is stored in the operating system credential vault (macOS Keychain, Windows Credential Manager, or Linux Secret Service). Existing installations automatically migrate and remove a legacy plaintext SQLite token after the credential-vault write succeeds.

The app creates its SQLite database and `converted-files` folder in the operating system's application-data directory. Converted file contents are not stored in SQLite.

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

The tests do not call CloudConvert. A live end-to-end conversion requires the gateway configuration above.
