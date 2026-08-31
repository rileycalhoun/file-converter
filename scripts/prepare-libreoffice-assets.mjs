import { copyFile, mkdir, stat } from "node:fs/promises";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = dirname(dirname(fileURLToPath(import.meta.url)));
const packageRoot = join(root, "node_modules", "@matbee", "libreoffice-converter");
const destination = join(root, "public", "libreoffice-wasm");
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

console.log(`Prepared ${assets.length} local LibreOffice WASM assets.`);
