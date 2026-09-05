// Ownership transfer detaches the source buffer. Opt in only for a buffer that
// the caller owns exclusively and will never read or reuse after postMessage.
export function prepareWorkerInput(input, { transferOwnership = false } = {}) {
  let bytes;
  if (ArrayBuffer.isView(input)) {
    bytes = new Uint8Array(input.buffer, input.byteOffset, input.byteLength);
  } else if (input instanceof ArrayBuffer || (typeof SharedArrayBuffer !== "undefined" && input instanceof SharedArrayBuffer)) {
    bytes = new Uint8Array(input);
  } else if (Array.isArray(input)) {
    bytes = new Uint8Array(input);
    if (bytes.byteLength === 0) throw new Error("The conversion input is empty or detached.");
    return bytes;
  } else {
    throw new TypeError("The conversion input must contain binary document bytes.");
  }
  if (bytes.byteLength === 0) throw new Error("The conversion input is empty or detached.");
  // Partial views must be isolated even with ownership: transferring their full
  // backing buffer would expose unrelated bytes. Shared buffers cannot transfer.
  if (transferOwnership === true && bytes.buffer instanceof ArrayBuffer && bytes.byteOffset === 0 && bytes.byteLength === bytes.buffer.byteLength) {
    return bytes;
  }
  return new Uint8Array(bytes);
}
