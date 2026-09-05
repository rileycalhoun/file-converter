import assert from "node:assert/strict";
import test from "node:test";
import { runBounded } from "../scripts/benchmark-native-office.mjs";

test("native benchmark captures normal exit and bounded output", async () => {
  const result = await runBounded(process.execPath, ["-e", "process.stdout.write('ok'); process.stderr.write('diagnostic')"], 2000);
  assert.equal(result.code, 0);
  assert.equal(result.stdout, "ok");
  assert.equal(result.stderr, "diagnostic");
  assert.equal(result.timedOut, false);
});

test("native benchmark deadline kills a hung subprocess", async () => {
  const result = await runBounded(process.execPath, ["-e", "setInterval(() => {}, 1000)"], 30);
  assert.equal(result.timedOut, true);
  assert.equal(result.signal, "SIGKILL");
  assert.ok(result.wallMs < 2000);
});

test("native benchmark reports a missing executable without waiting for its deadline", async () => {
  await assert.rejects(runBounded("/does-not-exist/native-office", [], 2000), { code: "ENOENT" });
});
