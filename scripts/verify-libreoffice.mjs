import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { basename, extname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { createWorkerConverter } from "@matbee/libreoffice-converter/server";

const root = fileURLToPath(new URL("..", import.meta.url));
const fixtures = ["sample.docx", "sample.pptx", "sample.xlsx", "sample.odt", "sample.rtf", "sample.txt"];
const outputDirectory = await mkdtemp(join(tmpdir(), "file-converter-wasm-"));
const converter = await createWorkerConverter();

try {
  for (const fixture of fixtures) {
    const input = await readFile(join(root, "tests", "fixtures", fixture));
    const result = await converter.convert(input, {
      inputFormat: extname(fixture).slice(1),
      outputFormat: "pdf",
    }, fixture);
    if (result.data.length < 5 || Buffer.from(result.data.subarray(0, 5)).toString() !== "%PDF-") {
      throw new Error(`${fixture} did not produce a PDF`);
    }
    await writeFile(join(outputDirectory, `${basename(fixture, extname(fixture))}.pdf`), result.data);
    console.log(`${fixture} -> PDF (${result.data.length} bytes)`);
  }
} finally {
  await converter.destroy();
  await rm(outputDirectory, { recursive: true, force: true });
}
