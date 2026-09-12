import { spawn, spawnSync } from "node:child_process";
import { mkdtemp, mkdir, readFile, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, extname, join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { performance } from "node:perf_hooks";

const script = fileURLToPath(import.meta.url);
const root = resolve(dirname(script), "..");
const fixtures = ["sample.docx", "sample.pptx", "sample.xlsx", "sample.odt", "sample.rtf", "sample.txt"];

// Unix process groups let the deadline stop the engine's descendants as well as
// the launcher. Only the detached child group created by this call is signalled.
export function runBounded(command, args, timeoutMs, env = process.env) {
  return new Promise((resolveRun, reject) => {
    const started = performance.now();
    const child = spawn(command, args, { detached: true, stdio: ["ignore", "pipe", "pipe"], env });
    let stdout = "";
    let stderr = "";
    let timedOut = false;
    const capture = (previous, bytes) => (previous + bytes.toString()).slice(-16_384);
    child.stdout.on("data", (bytes) => { stdout = capture(stdout, bytes); });
    child.stderr.on("data", (bytes) => { stderr = capture(stderr, bytes); });
    const timer = setTimeout(() => {
      timedOut = true;
      if (child.pid) {
        try { process.kill(-child.pid, "SIGKILL"); } catch (error) { if (error.code !== "ESRCH") child.kill("SIGKILL"); }
      }
    }, timeoutMs);
    child.once("error", (error) => { clearTimeout(timer); reject(error); });
    child.once("close", (code, signal) => {
      clearTimeout(timer);
      const rss = stderr.match(/(\d+)\s+maximum resident set size/);
      resolveRun({ code, signal, timedOut, wallMs: performance.now() - started,
        maxRssBytes: rss ? Number(rss[1]) : null, stdout, stderr });
    });
  });
}

async function inspectPdf(path) {
  const data = await readFile(path);
  if (data.subarray(0, 5).toString() !== "%PDF-") throw new Error(`Invalid PDF: ${path}`);
  return { path, bytes: data.length };
}

async function wasmChild(fixture, output) {
  if (!fixtures.includes(fixture)) throw new Error("Unknown fixture");
  const { createWorkerConverter } = await import("@matbee/libreoffice-converter/server");
  const input = await readFile(join(root, "tests/fixtures", fixture));
  const started = performance.now();
  const converter = await createWorkerConverter();
  const initializationMs = performance.now() - started;
  const samples = [];
  try {
    for (let index = 0; index < 2; index++) {
      const conversionStarted = performance.now();
      const result = await converter.convert(input, { inputFormat: extname(fixture).slice(1), outputFormat: "pdf" }, fixture);
      const conversionMs = performance.now() - conversionStarted;
      const path = join(output, `wasm-${index}.pdf`);
      await writeFile(path, result.data);
      samples.push({ runtime: index === 0 ? "first-conversion" : "same-worker", conversionMs, ...await inspectPdf(path) });
    }
  } finally { await converter.destroy(); }
  console.log(`BENCH_RESULT ${JSON.stringify({ initializationMs, samples })}`);
}

async function benchmarkNative(soffice) {
  if (process.platform !== "darwin" || process.arch !== "arm64") throw new Error("Run this comparison on a native ARM64 macOS host.");
  if (!soffice || !soffice.startsWith("/")) throw new Error("Pass --soffice /absolute/path/to/Contents/MacOS/soffice.");
  const architecture = spawnSync("/usr/bin/file", [soffice], { encoding: "utf8", timeout: 5000 });
  if (architecture.status !== 0 || !architecture.stdout.includes("arm64")) throw new Error("The selected soffice executable is not confirmed ARM64.");
  const artifacts = await mkdtemp(join(tmpdir(), "file-converter-native-bench-"));
  const deadline = performance.now() + 300_000;
  const version = await runBounded(soffice, [`-env:UserInstallation=${pathToFileURL(join(artifacts, "version-profile"))}`, "--version"], 5000);
  const report = { platform: process.platform, arch: process.arch, node: process.version, architecture: architecture.stdout.trim(),
    nativeVersion: version.stdout.trim(), artifacts, results: [],
    caveats: ["Native repeat starts a new process with the same profile; it is not a resident daemon.",
      "WASM samples use the Node worker with the same pinned WASM binary, not a packaged WebView.",
      "maxRssBytes is /usr/bin/time process high-water RSS; not summed concurrent tree RSS, physical footprint, or idle memory.",
      "Fresh profile/worker does not imply cold OS or HTTP caches. PDF header checks are not fidelity proof."] };
  for (const fixture of fixtures) {
    const directory = join(artifacts, fixture);
    await mkdir(directory);
    const row = { fixture, native: [], wasm: null };
    for (let round = 0; round < 2; round++) {
      const output = join(directory, `native-${round}`);
      await mkdir(output);
      const remaining = deadline - performance.now();
      if (remaining <= 0) throw new Error(`Overall five-minute deadline exceeded; artifacts: ${artifacts}`);
      const result = await runBounded("/usr/bin/time", ["-l", soffice,
        `-env:UserInstallation=${pathToFileURL(join(directory, "profile"))}`,
        "--headless", "--nologo", "--nodefault", "--nofirststartwizard", "--convert-to", "pdf", "--outdir", output,
        join(root, "tests/fixtures", fixture)], Math.min(30_000, remaining), { ...process.env, XDG_CACHE_HOME: join(directory, "cache") });
      const sample = { runtime: round === 0 ? "fresh-profile" : "restarted-same-profile", ...result };
      try { sample.pdf = await inspectPdf(join(output, "sample.pdf")); } catch (error) { sample.outputError = String(error); }
      row.native.push(sample);
    }
    const remaining = deadline - performance.now();
    if (remaining <= 0) throw new Error(`Overall five-minute deadline exceeded; artifacts: ${artifacts}`);
    const wasm = await runBounded("/usr/bin/time", ["-l", process.execPath, script, "--wasm-child", fixture, directory], Math.min(30_000, remaining));
    const payload = wasm.stdout.split("\n").findLast((line) => line.startsWith("BENCH_RESULT "));
    row.wasm = { ...wasm, measurement: payload ? JSON.parse(payload.slice("BENCH_RESULT ".length)) : null };
    report.results.push(row);
    await writeFile(join(artifacts, "report.json"), JSON.stringify(report, null, 2));
    console.error(`${fixture}: native ${row.native.map((sample) => sample.pdf && sample.code === 0 ? `${Math.round(sample.wallMs)}ms` : "FAILED").join(" / ")}; WASM ${row.wasm.measurement && wasm.code === 0 ? "measured" : "FAILED"}`);
  }
  console.log(JSON.stringify(report, null, 2));
}

if (process.argv[1] && resolve(process.argv[1]) === script) {
  if (process.argv[2] === "--wasm-child") await wasmChild(process.argv[3], process.argv[4]);
  else if (process.argv[2] === "--soffice") await benchmarkNative(process.argv[3]);
  else throw new Error("Usage: node scripts/benchmark-native-office.mjs --soffice /absolute/path/to/soffice");
}
