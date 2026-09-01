import { spawn } from "node:child_process";
import { readFile } from "node:fs/promises";
import { extname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { createWorkerConverter } from "@matbee/libreoffice-converter/server";

const root = fileURLToPath(new URL("..", import.meta.url));
const scriptPath = fileURLToPath(import.meta.url);
const fixtures = ["sample.docx", "sample.pptx", "sample.xlsx", "sample.odt", "sample.rtf", "sample.txt"];
const fixtureFlag = process.argv.indexOf("--fixture");

if (fixtureFlag >= 0) {
  const fixture = process.argv[fixtureFlag + 1];
  if (!fixtures.includes(fixture)) {
    throw new Error(`Unknown LibreOffice fixture: ${fixture || "(missing)"}`);
  }
  await convertFixture(fixture);
} else {
  for (const fixture of fixtures) {
    await runIsolatedFixture(fixture);
  }
}

async function convertFixture(fixture) {
  const converter = await createWorkerConverter();
  try {
    const input = await readFile(join(root, "tests", "fixtures", fixture));
    const result = await converter.convert(input, {
      inputFormat: extname(fixture).slice(1),
      outputFormat: "pdf",
    }, fixture);
    if (result.data.length < 5 || Buffer.from(result.data.subarray(0, 5)).toString() !== "%PDF-") {
      throw new Error(`${fixture} did not produce a PDF`);
    }
    console.log(`${fixture} -> PDF (${result.data.length} bytes)`);
  } finally {
    await converter.destroy();
  }
}

function runIsolatedFixture(fixture) {
  return new Promise((resolve, reject) => {
    const child = spawn(process.execPath, [scriptPath, "--fixture", fixture], {
      stdio: "inherit",
    });
    let settled = false;
    const timer = setTimeout(() => {
      if (settled) return;
      settled = true;
      child.kill("SIGKILL");
      reject(new Error(`${fixture} exceeded the 90-second LibreOffice smoke-test limit`));
    }, 90_000);

    child.once("error", (error) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      reject(error);
    });
    child.once("exit", (code, signal) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      if (code === 0) {
        resolve();
      } else {
        reject(new Error(`${fixture} smoke test exited with ${signal || `code ${code}`}`));
      }
    });
  });
}
