# Third-party notices

File Converter includes open-source software. This notice highlights the
conversion components shipped in the installer; the JavaScript and Cargo lock
files contain the complete dependency versions.

## LibreOffice WASM document converter

- Component: `@matbee/libreoffice-converter` 2.7.2
- Project: <https://github.com/matbeedotcom/libreoffice-document-converter>
- License: Mozilla Public License 2.0 (MPL-2.0)
- License text: <https://www.mozilla.org/MPL/2.0/>
- Source code: <https://github.com/matbeedotcom/libreoffice-document-converter/tree/v2.7.2>

The package bundles a WebAssembly build of LibreOffice. LibreOffice is made
available under MPL-2.0 and incorporates components under additional compatible
open-source licenses. LibreOffice's complete licensing and third-party notices
are available at <https://www.libreoffice.org/licenses/> and
<https://api.libreoffice.org/share/readme/LICENSE.html>.

No modifications are made to the package's LibreOffice WASM binaries. The build
copies them byte-for-byte from the installed npm package into the application.

## Rust image and PDF components

- `printpdf` 0.12.7 — MIT — <https://github.com/fschutt/printpdf>
- `image` 0.25.10 — MIT OR Apache-2.0 — <https://github.com/image-rs/image>

These libraries implement the native image-to-PDF engine. File Converter uses
only their PNG, JPEG, BMP, GIF, TIFF, and WebP decoding features.

## Other dependencies

Other dependencies retain their respective copyright and license notices. See
`Cargo.lock`, `package-lock.json`, and the installed package metadata for exact
versions and SPDX license identifiers.
