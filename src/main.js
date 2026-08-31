import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import { WorkerBrowserConverter, createWasmPaths } from "@matbee/libreoffice-converter/browser";

const state = {
  selectedPath: null,
  detected: null,
  conversions: [],
  supportedFormats: [],
  activeConversionId: null,
  cancellationRequested: false,
  missingConversion: null,
};

const elements = Object.fromEntries([
  "content", "history-view", "history-button", "supported-button", "back-button", "search-input",
  "choose-file", "selected-file", "detected-format", "convert", "convert-button",
  "cancel-conversion", "history", "status", "status-message", "refresh-button",
  "supported-dialog", "supported-list", "missing-dialog", "missing-message", "restore-button",
  "reconvert-button",
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
  if (!state.selectedPath) return;
  await beginConversion(() => invoke("start_conversion", { inputPath: state.selectedPath }));
});
elements.cancelConversion.addEventListener("click", requestCancellation);
elements.historyButton.addEventListener("click", () => showView("history"));
elements.backButton.addEventListener("click", () => showView("convert"));
elements.searchInput.addEventListener("input", renderHistory);
elements.refreshButton.addEventListener("click", loadHistory);
elements.supportedButton.addEventListener("click", () => elements.supportedDialog.showModal());
elements.history.addEventListener("click", handleHistoryAction);
elements.restoreButton.addEventListener("click", restoreMissingOutput);
elements.reconvertButton.addEventListener("click", reconvertMissingOutput);

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
    elements.detectedFormat.textContent = "Inspecting file…";
    elements.convertButton.disabled = true;
    const detected = await invoke("inspect_source", { inputPath: selected });
    state.detected = detected;
    elements.detectedFormat.textContent = `${detected.format.toUpperCase()} · ${formatBytes(detected.sourceSize)} · ${engineLabel(detected.engine)}`;
    elements.convertButton.disabled = false;
    hideStatus();
  } catch (error) {
    state.selectedPath = null;
    state.detected = null;
    elements.selectedFile.textContent = "Unsupported file";
    elements.detectedFormat.textContent = String(error);
    showStatus(String(error), "error");
  }
}

async function beginConversion(createStart) {
  setBusy(true);
  state.cancellationRequested = false;
  showStatus("Preparing local conversion…", "working");
  try {
    const start = await createStart();
    upsertConversion(start.conversion);
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
    upsertConversion(conversion);
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
  elements.cancelConversion.disabled = true;
  showStatus("Cancellation requested. The current WASM operation will be discarded safely when it stops.", "working");
  try {
    const conversion = await invoke("cancel_conversion", { id: state.activeConversionId });
    upsertConversion(conversion);
  } catch (error) {
    showStatus(String(error), "error");
  }
}

function finishUiConversion(conversion) {
  upsertConversion(conversion);
  if (conversion.status === "finished") {
    showStatus(`${conversion.sourceName} was converted locally to PDF.`, "success");
    state.selectedPath = null;
    state.detected = null;
    elements.selectedFile.textContent = "No file selected";
    elements.detectedFormat.textContent = "Choose a supported document or image.";
    openCompletedConversion(conversion).catch((error) => {
      showStatus(`The PDF was saved, but it could not be opened: ${String(error)}`, "error");
    });
  } else if (conversion.status === "cancelled") {
    showStatus("Conversion cancelled. No output file was saved.", "working");
  } else {
    showStatus(conversion.error || "The document could not be converted.", "error");
  }
}

async function handleHistoryAction(event) {
  const button = event.target.closest("button[data-action]");
  if (!button) return;
  const conversion = state.conversions.find((item) => item.id === button.dataset.id);
  if (!conversion) return;
  try {
    if (button.dataset.action === "open") await openCompletedConversion(conversion);
    if (button.dataset.action === "reconvert") {
      showView("convert");
      await beginConversion(() => invoke("reconvert", { id: conversion.id }));
    }
    if (button.dataset.action === "delete") {
      await invoke("delete_conversion", { id: conversion.id });
      state.conversions = state.conversions.filter((item) => item.id !== conversion.id);
      renderHistory();
    }
  } catch (error) {
    showStatus(String(error), "error");
  }
}

async function openCompletedConversion(conversion) {
  const result = await invoke("open_conversion", { id: conversion.id });
  if (!result.missing) return;
  state.missingConversion = conversion;
  elements.missingMessage.textContent = result.restorable
    ? `${conversion.sourceName}'s output is missing. This legacy history entry has a stored database copy.`
    : result.reconvertible
      ? `${conversion.sourceName}'s output is missing. The original still exists and can be converted again.`
      : `${conversion.sourceName}'s output is missing, and neither a legacy backup nor the original source is available.`;
  elements.restoreButton.classList.toggle("hidden", !result.restorable);
  elements.reconvertButton.classList.toggle("hidden", !result.reconvertible);
  elements.missingDialog.showModal();
}

async function restoreMissingOutput() {
  const conversion = state.missingConversion;
  if (!conversion) return;
  elements.missingDialog.close();
  await invoke("restore_conversion", { id: conversion.id });
  showStatus(`${conversion.sourceName} was restored from its legacy database copy.`, "success");
}

async function reconvertMissingOutput() {
  const conversion = state.missingConversion;
  if (!conversion) return;
  elements.missingDialog.close();
  showView("convert");
  await beginConversion(() => invoke("reconvert", { id: conversion.id }));
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

async function loadHistory() {
  try {
    state.conversions = await invoke("list_conversions");
    renderHistory();
  } catch (error) {
    showStatus(String(error), "error");
  }
}

function renderHistory() {
  const search = elements.searchInput.value.trim().toLowerCase();
  const conversions = state.conversions.filter((conversion) =>
    conversion.sourceName.toLowerCase().includes(search));
  if (conversions.length === 0) {
    elements.history.innerHTML = `<div id="no-files"><h3>${search ? "No matching conversions." : "No conversions yet."}</h3></div>`;
    return;
  }
  elements.history.innerHTML = `<ul id="files">${conversions.map((conversion) => {
    const canOpen = conversion.status === "finished";
    const detail = [
      conversion.detectedFormat?.toUpperCase(),
      conversion.engine ? engineLabel(conversion.engine) : null,
      conversion.outputSize ? formatBytes(conversion.outputSize) : null,
    ].filter(Boolean).join(" · ");
    return `<li>
      <div class="history-file">
        <strong>${escapeHtml(conversion.sourceName)} → PDF</strong>
        <small>${escapeHtml(detail || conversion.error || "Legacy history entry")}</small>
      </div>
      <span class="status-label status-${escapeHtml(conversion.status)}">${escapeHtml(conversion.status)}</span>
      ${canOpen ? `<button data-action="open" data-id="${conversion.id}">Open</button>` : ""}
      ${conversion.sourcePath ? `<button data-action="reconvert" data-id="${conversion.id}" ${state.activeConversionId ? "disabled" : ""}>Reconvert</button>` : ""}
      <button class="remove-button" data-action="delete" data-id="${conversion.id}">Remove</button>
    </li>`;
  }).join("")}</ul>`;
}

function upsertConversion(conversion) {
  state.conversions = [conversion, ...state.conversions.filter((item) => item.id !== conversion.id)];
  renderHistory();
}

function setBusy(busy) {
  elements.chooseFile.disabled = busy;
  elements.convertButton.disabled = busy || !state.selectedPath || !state.detected;
  elements.convertButton.textContent = busy ? "Converting…" : "Convert";
  elements.cancelConversion.classList.toggle("hidden", !busy || !state.activeConversionId);
  elements.cancelConversion.disabled = state.cancellationRequested;
}

function showView(view) {
  elements.content.classList.toggle("hidden", view !== "convert");
  elements.historyView.classList.toggle("hidden", view !== "history");
  if (view === "history") renderHistory();
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

await Promise.all([loadSupportedFormats(), loadHistory()]);
