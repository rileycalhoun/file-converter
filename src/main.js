import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import { createWasmPaths } from "@matbee/libreoffice-converter/browser";
import { ConversionGuard } from "./conversion-guard.js";
import { HistoryPager } from "./history-pager.js";
import { LibreOfficeWasmRunner, failWasmConversion } from "./libreoffice-wasm-runner.js";

const state = {
  selectedPath: null,
  detected: null,
  supportedFormats: [],
  conversions: [],
  activeConversionId: null,
  activeConversionToken: null,
  cancellationRequested: false,
};

const elements = Object.fromEntries([
  "history-button", "settings-button", "choose-file", "selected-file", "convert", "convert-button",
  "status", "status-message", "history-dialog", "history-summary", "history-list", "settings-dialog",
  "supported-list", "history-more",
].map((id) => [camelize(id), document.querySelector(`#${id}`)]));

// Each read returns private IPC bytes used only by this worker request.
const wasmRunner = new LibreOfficeWasmRunner({ invoke, wasmPaths: createWasmPaths("/libreoffice-wasm/"), transferInputOwnership: true });
const conversionGuard = new ConversionGuard();
const historyPager = new HistoryPager((parameters) => invoke("list_conversions", parameters));

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
elements.historyButton.addEventListener("click", async () => {
  elements.historyDialog.showModal();
  await loadHistory();
});
elements.historyList.addEventListener("click", handleHistoryAction);
elements.historyMore.addEventListener("click", () => loadHistory({ append: true }));

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
  const token = conversionGuard.begin();
  if (!token) {
    showStatus("Another conversion is already running. Wait for it to finish or cancel it.", "error");
    return;
  }
  state.activeConversionToken = token;
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
      if (error?.name === "AbortError" || state.cancellationRequested) return;
      conversion = await failWasmConversion(invoke, start.wasmTask.conversionId, error);
    }
    finishUiConversion(conversion);
  } catch (error) {
    if (!state.cancellationRequested) showStatus(String(error), "error");
  } finally {
    if (conversionGuard.finish(token)) {
      state.activeConversionId = null;
      state.cancellationRequested = false;
      setBusy(false);
    }
    if (elements.historyDialog.open) await loadHistory();
  }
}

async function requestCancellation() {
  if (!state.activeConversionId || state.cancellationRequested) return;
  state.cancellationRequested = true;
  elements.convertButton.disabled = true;
  const id = state.activeConversionId;
  const token = state.activeConversionToken;
  // Stop synchronously: cancelling must not wait for a blocked WASM thread.
  wasmRunner.cancel();
  showStatus("Conversion cancelled. No output file was saved.", "working");
  try {
    await invoke("cancel_conversion", { id });
  } catch (error) {
    if (state.activeConversionToken === token) showStatus(String(error), "error");
  } finally {
    if (elements.historyDialog.open) await loadHistory();
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

async function loadHistory({ append = false } = {}) {
  const pending = historyPager.load({ reset: !append });
  setHistoryLoading();
  try {
    const page = await pending;
    if (!page) return;
    state.conversions = historyPager.items;
    if (append && state.conversions.length > page.entries.length) {
      elements.historyList.insertAdjacentHTML("beforeend", page.entries.map(historyEntryHtml).join(""));
      renderHistorySummary();
    } else {
      renderHistory();
    }
  } catch (error) {
    elements.historySummary.textContent = `History is temporarily unavailable: ${String(error)}`;
    // Keep already loaded rows and their actions available when a later page fails.
  } finally {
    setHistoryLoading();
  }
}

function setHistoryLoading() {
  elements.historyList.setAttribute("aria-busy", String(historyPager.loading));
  elements.historyMore.disabled = historyPager.loading;
  elements.historyMore.textContent = historyPager.loading ? "Loading…" : "Load more";
  if (historyPager.loading && !historyPager.loaded) elements.historySummary.textContent = "Loading recent conversions…";
}

function renderHistorySummary() {
  const count = state.conversions.length;
  elements.historySummary.textContent = count === 0
    ? historyPager.nextCursor ? "More history is available." : "Completed conversions will appear here."
    : `Showing ${count} saved ${count === 1 ? "conversion" : "conversions"}, newest first${historyPager.nextCursor ? "; more available" : ""}.`;
  elements.historyButton.title = count === 0 ? "Conversion history" : `${count}${historyPager.nextCursor ? "+" : ""} saved conversions`;
  elements.historyMore.hidden = !historyPager.nextCursor;
}

function renderHistory() {
  renderHistorySummary();
  const count = state.conversions.length;
  if (count === 0) {
    elements.historyList.innerHTML = `
      <div class="history-empty">
        <strong>${historyPager.nextCursor ? "No entries currently shown" : "No conversions yet"}</strong>
        <span>${historyPager.nextCursor ? "Load more to see older conversions." : "Choose a file to create your first local PDF."}</span>
      </div>
    `;
    return;
  }
  elements.historyList.innerHTML = state.conversions.map(historyEntryHtml).join("");
}

function replaceHistoryEntry(conversion) {
  historyPager.update(conversion);
  state.conversions = historyPager.items;
  const article = elements.historyList.querySelector(`[data-entry-id="${conversion.id}"]`);
  if (article) article.outerHTML = historyEntryHtml(conversion);
}

function historyEntryHtml(conversion) {
  const finished = conversion.status === "finished";
  const terminal = ["finished", "failed", "cancelled"].includes(conversion.status);
  const missingMessage = finished && conversion.missing
    ? conversion.restorable || conversion.reconvertible
      ? "The saved PDF is missing. Choose an available recovery option."
      : "The saved PDF is missing, and the original source is unavailable."
    : "";
  const detail = conversion.error && !finished
    ? conversion.error
    : conversion.outputPath || conversion.sourcePath || "No file path is available.";
  const actions = [];
  if (finished && !conversion.missing) {
    actions.push(historyButtonHtml("open", conversion.id, "Open PDF", "primary"));
  }
  if (conversion.missing && conversion.restorable) {
    actions.push(historyButtonHtml("restore", conversion.id, "Restore saved copy", "primary"));
  }
  if (conversion.missing && conversion.reconvertible) {
    actions.push(historyButtonHtml("reconvert", conversion.id, "Reconvert", "primary", conversionGuard.isActive));
  }
  if (terminal) {
    actions.push(historyButtonHtml("delete", conversion.id, "Delete", "danger"));
  }
  return `
    <article class="history-entry" data-entry-id="${conversion.id}" data-status="${escapeHtml(conversion.status)}">
      <header>
        <div>
          <h4 title="${escapeHtml(conversion.sourceName)}">${escapeHtml(conversion.sourceName)}</h4>
          <p>${escapeHtml(formatHistoryMeta(conversion))}</p>
        </div>
        <span class="history-status">${escapeHtml(titleCase(conversion.status))}</span>
      </header>
      ${missingMessage ? `<p class="history-warning">${escapeHtml(missingMessage)}</p>` : ""}
      <p class="history-detail" title="${escapeHtml(detail)}">${escapeHtml(detail)}</p>
      <div class="history-actions">${actions.join("")}</div>
    </article>
  `;
}

function historyButtonHtml(action, id, label, kind, disabled = false) {
  return `<button type="button" class="history-action ${kind}" data-history-action="${action}" data-conversion-id="${id}"${disabled ? " disabled" : ""}>${label}</button>`;
}

async function handleHistoryAction(event) {
  const button = event.target.closest("[data-history-action]");
  if (!button) return;
  const conversion = state.conversions.find((entry) => entry.id === button.dataset.conversionId);
  if (!conversion) return;
  const action = button.dataset.historyAction;
  if (action === "reconvert" && conversionGuard.isActive) {
    showStatus("Another conversion is already running. Wait for it to finish or cancel it.", "error");
    return;
  }
  if (action === "delete" && !confirm(`Delete the history entry for ${conversion.sourceName}? Its converted PDF will also be deleted if it still exists.`)) {
    return;
  }
  button.disabled = true;
  try {
    if (action === "open") {
      const availability = await invoke("open_conversion", { id: conversion.id });
      Object.assign(conversion, availability);
      if (availability.missing) {
        replaceHistoryEntry(conversion);
        showStatus("The converted PDF is missing. Choose an available recovery option in History.", "error");
      }
    } else if (action === "restore") {
      await invoke("restore_conversion", { id: conversion.id });
      showStatus(`${conversion.sourceName} was restored from its legacy saved copy.`, "success");
      replaceHistoryEntry(await invoke("get_history_entry", { id: conversion.id }));
    } else if (action === "reconvert") {
      elements.historyDialog.close();
      await beginConversion(() => invoke("reconvert", { id: conversion.id }));
    } else if (action === "delete") {
      await invoke("delete_conversion", { id: conversion.id });
      showStatus(`Removed ${conversion.sourceName} from conversion history.`, "success");
      historyPager.remove(conversion.id);
      state.conversions = historyPager.items;
      elements.historyList.querySelector(`[data-entry-id="${conversion.id}"]`)?.remove();
      if (state.conversions.length) renderHistorySummary();
      else renderHistory();
    }
  } catch (error) {
    showStatus(String(error), "error");
    await loadHistory();
  } finally {
    if (button.isConnected) button.disabled = false;
  }
}

function formatHistoryMeta(conversion) {
  const format = conversion.detectedFormat ? conversion.detectedFormat.toUpperCase() : "Unknown";
  return `${format} → PDF · ${formatDate(conversion.createdAt)}`;
}

function formatDate(value) {
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return "Unknown date";
  return new Intl.DateTimeFormat(undefined, {
    dateStyle: "medium",
    timeStyle: "short",
  }).format(date);
}

function setBusy(busy) {
  elements.chooseFile.disabled = busy;
  elements.historyList.querySelectorAll('[data-history-action="reconvert"]').forEach((button) => {
    button.disabled = busy;
  });
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
