import assert from "node:assert/strict";
import test from "node:test";
import { prepareWorkerInput } from "../src/worker-input.js";

test("ownership transfer uses the exact owned ArrayBuffer and detaches it", () => {
  for (const wrap of [(buffer) => buffer, (buffer) => new Uint8Array(buffer)]) {
    const buffer = new Uint8Array([1, 2, 3]).buffer;
    const bytes = prepareWorkerInput(wrap(buffer), { transferOwnership: true });
    assert.equal(bytes.buffer, buffer);
    const received = structuredClone(bytes, { transfer: [bytes.buffer] });
    assert.equal(buffer.byteLength, 0);
    assert.deepEqual(received, new Uint8Array([1, 2, 3]));
    assert.throws(() => prepareWorkerInput(buffer, { transferOwnership: true }), /detached/);
  }
});

test("safe default retains caller input for repeats", () => {
  const source = new Uint8Array([1, 2, 3]);
  for (let index = 0; index < 2; index++) {
    const bytes = prepareWorkerInput(source);
    assert.notEqual(bytes.buffer, source.buffer);
    structuredClone(bytes, { transfer: [bytes.buffer] });
    assert.deepEqual(source, new Uint8Array([1, 2, 3]));
  }
});

test("partial typed arrays and DataViews isolate unrelated backing bytes", () => {
  for (const wrap of [(buffer) => new Uint8Array(buffer, 2, 3), (buffer) => new DataView(buffer, 2, 3)]) {
    const backing = new Uint8Array([99, 98, 1, 2, 3, 97]);
    const bytes = prepareWorkerInput(wrap(backing.buffer), { transferOwnership: true });
    assert.equal(bytes.byteOffset, 0);
    assert.equal(bytes.buffer.byteLength, 3);
    const received = structuredClone(bytes, { transfer: [bytes.buffer] });
    assert.deepEqual(received, new Uint8Array([1, 2, 3]));
    assert.deepEqual(backing, new Uint8Array([99, 98, 1, 2, 3, 97]));
  }
});

test("shared and array input become private transferable buffers", () => {
  const shared = new Uint8Array(new SharedArrayBuffer(3));
  shared.set([1, 2, 3]);
  for (const input of [shared, shared.buffer, [1, 2, 3]]) {
    const bytes = prepareWorkerInput(input, { transferOwnership: true });
    assert.ok(bytes.buffer instanceof ArrayBuffer);
    assert.deepEqual(structuredClone(bytes, { transfer: [bytes.buffer] }), new Uint8Array([1, 2, 3]));
  }
  assert.deepEqual([...shared], [1, 2, 3]);
});

test("empty, detached views and nonbinary values are rejected", () => {
  const view = new Uint8Array([1, 2]);
  structuredClone(view, { transfer: [view.buffer] });
  for (const input of [view, new Uint8Array(), null, undefined, "document"]) {
    assert.throws(() => prepareWorkerInput(input, { transferOwnership: true }));
  }
});
