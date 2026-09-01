import { copyFile, mkdir, stat } from "node:fs/promises";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = dirname(dirname(fileURLToPath(import.meta.url)));
const packageRoot = join(root, "node_modules", "@matbee", "libreoffice-converter");
const destination = join(root, "public", "libreoffice-wasm");
const noticesSource = join(root, "THIRD_PARTY_NOTICES.md");
const noticesDestination = join(root, "public", "THIRD_PARTY_NOTICES.md");
const assets = [
  ["wasm/soffice.js", "soffice.js"],
  ["wasm/soffice.wasm", "soffice.wasm"],
  ["wasm/soffice.data", "soffice.data"],
  ["wasm/soffice.worker.js", "soffice.worker.js"],
  ["dist/browser.worker.global.js", "browser.worker.global.js"],
];

await mkdir(destination, { recursive: true });
for (const [relativeSource, outputName] of assets) {
  const source = join(packageRoot, relativeSource);
  const output = join(destination, outputName);
  const sourceInfo = await stat(source);
  let outputInfo;
  try {
    outputInfo = await stat(output);
  } catch {
    outputInfo = null;
  }
  if (!outputInfo || outputInfo.size !== sourceInfo.size || outputInfo.mtimeMs < sourceInfo.mtimeMs) {
    await copyFile(source, output);
  }
}

await copyFile(noticesSource, noticesDestination);

console.log(`Prepared ${assets.length} local LibreOffice WASM assets and dependency notices.`);
