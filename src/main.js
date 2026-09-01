import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import { WorkerBrowserConverter, createWasmPaths } from "@matbee/libreoffice-converter/browser";

const state = {
  selectedPath: null,
  detected: null,
  supportedFormats: [],
  activeConversionId: null,
  cancellationRequested: false,
};

const elements = Object.fromEntries([
  "settings-button", "choose-file", "selected-file", "convert", "convert-button", "status",
  "status-message", "settings-dialog", "supported-list",
].map((id) => [camelize(id), document.querySelector(`#${id}`)]));

class LibreOfficeWasmRunner {
  converter = null;
  initialization = null;

  async initialize(onProgress) {
    if (this.converter?.isReady()) return;
    if (this.initialization) return this.initialization;
    if (!globalThis.crossOriginIsolated || typeof SharedArrayBuffer === "undefined") {
      throw new Error("The local LibreOffice runtime requires cross-origin isolation, but this app window is not isolated.");
    }
    this.converter = new WorkerBrowserConverter({
      ...createWasmPaths("/libreoffice-wasm/"),
      browserWorkerJs: "/libreoffice-wasm/browser.worker.global.js",
      verbose: false,
      onProgress,
    });
    this.initialization = this.converter.initialize().finally(() => {
      this.initialization = null;
    });
    return this.initialization;
  }

  async convert(task, onProgress) {
    await this.initialize(onProgress);
    const input = new Uint8Array(await invoke("read_conversion_input", {
      id: task.conversionId,
    }));
    const result = await this.converter.convert(input, {
      inputFormat: task.inputFormat,
      outputFormat: "pdf",
    }, task.fileName);
    return invoke("complete_wasm_conversion", result.data, {
      headers: { "x-conversion-id": task.conversionId },
    });
  }
}

const wasmRunner = new LibreOfficeWasmRunner();

elements.chooseFile.addEventListener("click", chooseFile);
elements.convert.addEventListener("submit", async (event) => {
  event.preventDefault();
  if (state.activeConversionId) {
    await requestCancellation();
    return;
  }
  if (!state.selectedPath) return;
  await beginConversion(() => invoke("start_conversion", { inputPath: state.selectedPath }));
});
elements.settingsButton.addEventListener("click", () => elements.settingsDialog.showModal());

document.querySelectorAll("[data-close]").forEach((button) => {
  button.addEventListener("click", () => document.querySelector(`#${button.dataset.close}`).close());
});

async function chooseFile() {
  try {
    const extensions = [...new Set(state.supportedFormats.flatMap((format) => format.extensions))];
    const selected = await open({
      multiple: false,
      directory: false,
      filters: [{ name: "Supported files", extensions }],
    });
    if (!selected) return;
    state.selectedPath = selected;
    state.detected = null;
    elements.selectedFile.textContent = fileName(selected);
    elements.selectedFile.removeAttribute("title");
    elements.convertButton.disabled = true;
    showStatus("Inspecting file…", "working");
    const detected = await invoke("inspect_source", { inputPath: selected });
    state.detected = detected;
    elements.selectedFile.textContent = `${fileName(selected)} · ${detected.format.toUpperCase()}`;
    elements.selectedFile.title = `${formatBytes(detected.sourceSize)} · ${engineLabel(detected.engine)}`;
    elements.convertButton.disabled = false;
    hideStatus();
  } catch (error) {
    state.selectedPath = null;
    state.detected = null;
    elements.selectedFile.textContent = "Unsupported file";
    elements.selectedFile.removeAttribute("title");
    showStatus(String(error), "error");
  }
}

async function beginConversion(createStart) {
  setBusy(true);
  state.cancellationRequested = false;
  showStatus("Preparing local conversion…", "working");
  try {
    const start = await createStart();
    if (!start.wasmTask) {
      finishUiConversion(start.conversion);
      return;
    }
    state.activeConversionId = start.wasmTask.conversionId;
    setBusy(true);
    showStatus("Initializing the local LibreOffice engine…", "working");
    let conversion;
    try {
      conversion = await wasmRunner.convert(start.wasmTask, (progress) => {
        if (!state.cancellationRequested) {
          showStatus(progress.phase === "converting" ? "Converting locally…" : progress.message, "working");
        }
      });
    } catch (error) {
      if (state.cancellationRequested) return;
      conversion = await invoke("fail_wasm_conversion", {
        id: start.wasmTask.conversionId,
        error: safeEngineError(error),
      });
    }
    finishUiConversion(conversion);
  } catch (error) {
    if (!state.cancellationRequested) showStatus(String(error), "error");
  } finally {
    state.activeConversionId = null;
    state.cancellationRequested = false;
    setBusy(false);
  }
}

async function requestCancellation() {
  if (!state.activeConversionId || state.cancellationRequested) return;
  state.cancellationRequested = true;
  elements.convertButton.disabled = true;
  showStatus("Cancellation requested. The current WASM operation will be discarded safely when it stops.", "working");
  try {
    await invoke("cancel_conversion", { id: state.activeConversionId });
  } catch (error) {
    showStatus(String(error), "error");
  }
}

function finishUiConversion(conversion) {
  if (conversion.status === "finished") {
    showStatus(`${conversion.sourceName} was converted locally to PDF.`, "success");
    state.selectedPath = null;
    state.detected = null;
    elements.selectedFile.textContent = "No file selected";
    elements.selectedFile.removeAttribute("title");
    openCompletedConversion(conversion).catch((error) => {
      showStatus(`The PDF was saved, but it could not be opened: ${String(error)}`, "error");
    });
  } else if (conversion.status === "cancelled") {
    showStatus("Conversion cancelled. No output file was saved.", "working");
  } else {
    showStatus(conversion.error || "The document could not be converted.", "error");
  }
}

async function openCompletedConversion(conversion) {
  const result = await invoke("open_conversion", { id: conversion.id });
  if (result.missing) throw new Error("The converted PDF could not be found at its saved location.");
}

async function loadSupportedFormats() {
  state.supportedFormats = await invoke("get_supported_formats");
  const groups = state.supportedFormats.reduce((result, format) => {
    const formats = result.get(format.family) || [];
    formats.push(format);
    result.set(format.family, formats);
    return result;
  }, new Map());
  elements.supportedList.innerHTML = [...groups.entries()].map(([family, formats]) => `
    <section class="format-group">
      <h4>${escapeHtml(titleCase(family))}</h4>
      <p>${formats.map((format) => `<span title="${escapeHtml(format.label)}">.${escapeHtml(format.extensions.join(" / ."))}</span>`).join("")}</p>
    </section>
  `).join("");
}

function setBusy(busy) {
  elements.chooseFile.disabled = busy;
  if (busy && state.activeConversionId) {
    elements.convertButton.disabled = state.cancellationRequested;
    elements.convertButton.textContent = state.cancellationRequested ? "Cancelling…" : "Cancel";
  } else {
    elements.convertButton.disabled = busy || !state.selectedPath || !state.detected;
    elements.convertButton.textContent = busy ? "Converting…" : "Convert";
  }
}

function showStatus(message, kind) {
  elements.statusMessage.textContent = message;
  elements.status.dataset.kind = kind;
  elements.status.style.display = "block";
}

function hideStatus() {
  elements.status.style.display = "none";
}

function safeEngineError(error) {
  const message = error instanceof Error ? error.message : String(error);
  return message.replace(/\n.*/s, "").slice(0, 600);
}

function fileName(path) {
  return path.split(/[\\/]/).pop();
}

function formatBytes(value) {
  if (!Number.isFinite(value)) return "";
  const units = ["B", "KiB", "MiB", "GiB"];
  let amount = Number(value);
  let unit = 0;
  while (amount >= 1024 && unit < units.length - 1) {
    amount /= 1024;
    unit += 1;
  }
  return `${amount.toFixed(unit === 0 ? 0 : 1)} ${units[unit]}`;
}

function engineLabel(engine) {
  return ({
    "libreoffice-wasm": "LibreOffice WASM",
    "image-pdf": "Native image engine",
    "pdf-passthrough": "PDF copy",
  })[engine] || engine;
}

function camelize(value) {
  return value.replace(/-([a-z])/g, (_, character) => character.toUpperCase());
}

function titleCase(value) {
  return value.charAt(0).toUpperCase() + value.slice(1);
}

function escapeHtml(value) {
  return String(value).replace(/[&<>'"]/g, (character) => ({
    "&": "&amp;", "<": "&lt;", ">": "&gt;", "'": "&#39;", '"': "&quot;",
  })[character]);
}

await loadSupportedFormats();
