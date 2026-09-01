import assert from "node:assert/strict";
import test from "node:test";

import { ConversionGuard } from "../src/conversion-guard.js";

test("a nested conversion cannot replace or clear the active conversion", () => {
  const guard = new ConversionGuard();
  const active = guard.begin();

  assert.ok(active);
  assert.equal(guard.isActive, true);
  assert.equal(guard.begin(), null);
  assert.equal(guard.finish(Symbol("other conversion")), false);
  assert.equal(guard.isActive, true);
  assert.equal(guard.finish(active), true);
  assert.equal(guard.isActive, false);
});
