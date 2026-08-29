import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";

const state = {
  selectedPath: null,
  conversions: [],
};

const elements = {
  content: document.querySelector("#content"),
  historyView: document.querySelector("#history-view"),
  searchButton: document.querySelector("#search-button"),
  backButton: document.querySelector("#back-button"),
  searchInput: document.querySelector("#search-input"),
  chooseFile: document.querySelector("#choose-file"),
  selectedFile: document.querySelector("#selected-file"),
  convertForm: document.querySelector("#convert"),
  convertButton: document.querySelector("#convert-button"),
  history: document.querySelector("#history"),
  status: document.querySelector("#status"),
  statusMessage: document.querySelector("#status-message"),
  refreshButton: document.querySelector("#refresh-button"),
  settingsButton: document.querySelector("#settings-button"),
  settingsDialog: document.querySelector("#settings-dialog"),
  settingsForm: document.querySelector("#settings-form"),
  cancelSettings: document.querySelector("#cancel-settings"),
  gatewayUrl: document.querySelector("#gateway-url"),
  gatewayToken: document.querySelector("#gateway-token"),
  tokenState: document.querySelector("#token-state"),
  restoreDialog: document.querySelector("#restore-dialog"),
  restoreMessage: document.querySelector("#restore-message"),
};

elements.chooseFile.addEventListener("click", async () => {
  const selected = await open({
    multiple: false,
    directory: false,
    filters: [{
      name: "Documents and presentations",
      extensions: ["doc", "docx", "ppt", "pptx", "xls", "xlsx", "odt", "odp", "ods", "rtf", "txt"],
    }],
  });
  if (!selected) return;
  state.selectedPath = selected;
  elements.selectedFile.textContent = selected.split(/[\\/]/).pop();
  elements.convertButton.disabled = false;
});

elements.convertForm.addEventListener("submit", async (event) => {
  event.preventDefault();
  if (!state.selectedPath) return;
  setBusy(true);
  try {
    const conversion = await invoke("start_conversion", {
      inputPath: state.selectedPath,
    });
    state.conversions.unshift(conversion);
    renderHistory();
    if (conversion.status === "finished") {
      showStatus(`${conversion.sourceName} was converted to PDF.`, true);
      try {
        await openConversion(conversion);
      } catch (error) {
        showStatus(`The PDF was saved, but it could not be opened: ${String(error)}`, false);
      }
    } else {
      showStatus(conversion.error || "The document could not be converted.", false);
    }
    state.selectedPath = null;
    elements.selectedFile.textContent = "No file selected";
  } catch (error) {
    showStatus(String(error), false);
  } finally {
    setBusy(false);
  }
});

elements.searchButton.addEventListener("click", () => {
  elements.content.classList.add("hidden");
  elements.historyView.classList.remove("hidden");
  renderHistory();
});

elements.backButton.addEventListener("click", () => {
  elements.historyView.classList.add("hidden");
  elements.content.classList.remove("hidden");
});

elements.searchInput.addEventListener("input", renderHistory);
elements.refreshButton.addEventListener("click", loadHistory);
elements.settingsButton.addEventListener("click", showSettings);
elements.cancelSettings.addEventListener("click", () => elements.settingsDialog.close());

elements.settingsForm.addEventListener("submit", async (event) => {
  event.preventDefault();
  try {
    await invoke("save_settings", {
      gatewayUrl: elements.gatewayUrl.value,
      gatewayToken: elements.gatewayToken.value || null,
    });
    elements.gatewayToken.value = "";
    elements.settingsDialog.close();
    showStatus("Gateway settings saved.", true);
  } catch (error) {
    showStatus(String(error), false);
  }
});

elements.history.addEventListener("click", async (event) => {
  const button = event.target.closest("button[data-action]");
  if (!button) return;
  const { action, id } = button.dataset;
  try {
    if (action === "open") {
      const conversion = state.conversions.find((item) => item.id === id);
      if (conversion) await openConversion(conversion);
    }
    if (action === "delete") {
      await invoke("delete_conversion", { id });
      state.conversions = state.conversions.filter((item) => item.id !== id);
      renderHistory();
    }
  } catch (error) {
    showStatus(String(error), false);
  }
});

async function openConversion(conversion) {
  const result = await invoke("open_conversion", { id: conversion.id });
  if (!result.missing) return;
  if (!result.restorable) {
    throw new Error("The file is missing and this older history entry has no saved copy.");
  }
  if (!await confirmRestore(conversion.sourceName)) return;
  await invoke("restore_conversion", { id: conversion.id });
  showStatus(`${conversion.sourceName} was recreated from its saved copy.`, true);
}

function confirmRestore(sourceName) {
  elements.restoreMessage.textContent = `${sourceName} is no longer at its saved location. Would you like File Converter to recreate and open it?`;
  elements.restoreDialog.returnValue = "cancel";
  elements.restoreDialog.showModal();
  return new Promise((resolve) => {
    elements.restoreDialog.addEventListener("close", () => {
      resolve(elements.restoreDialog.returnValue === "restore");
    }, { once: true });
  });
}

async function showSettings() {
  try {
    const settings = await invoke("get_settings");
    elements.gatewayUrl.value = settings.gatewayUrl;
    elements.gatewayToken.value = "";
    elements.tokenState.textContent = settings.tokenConfigured
      ? "An access token is saved locally."
      : "No access token is configured yet.";
    elements.settingsDialog.showModal();
  } catch (error) {
    showStatus(String(error), false);
  }
}

async function loadHistory() {
  try {
    state.conversions = await invoke("list_conversions");
    renderHistory();
  } catch (error) {
    showStatus(String(error), false);
  }
}

function renderHistory() {
  const search = elements.searchInput.value.trim().toLowerCase();
  const conversions = state.conversions.filter((conversion) =>
    conversion.sourceName.toLowerCase().includes(search));

  if (conversions.length === 0) {
    const message = search
      ? `There were no files found containing '${escapeHtml(search)}'!`
      : "There were no files found!";
    elements.history.innerHTML = `<div id="no-files"><h3>${message}</h3></div>`;
    return;
  }

  elements.history.innerHTML = `<ul id="files">${conversions.map((conversion) => {
    const canOpen = conversion.status === "finished";
    return `<li>
      <button class="file-link" ${canOpen ? `data-action="open" data-id="${conversion.id}"` : "disabled"}>
        ${escapeHtml(conversion.sourceName)} → ${escapeHtml(conversion.outputFormat.toUpperCase())}
      </button>
      <span class="status-label">${escapeHtml(conversion.status)}</span>
      ${canOpen ? `<button data-action="open" data-id="${conversion.id}">Open</button>` : ""}
      <button class="remove-button" data-action="delete" data-id="${conversion.id}">Remove</button>
    </li>`;
  }).join("")}</ul>`;
}

function showStatus(message, success) {
  elements.statusMessage.textContent = message;
  elements.status.style.display = "block";
  elements.status.style.backgroundColor = success ? "var(--success-color)" : "var(--error-color)";
  elements.status.style.borderColor = success ? "var(--success-border-color)" : "var(--error-border-color)";
}

function setBusy(busy) {
  elements.convertButton.disabled = busy || !state.selectedPath;
  elements.convertButton.textContent = busy ? "Converting…" : "Convert";
}

function escapeHtml(value) {
  return String(value).replace(/[&<>'"]/g, (character) => ({
    "&": "&amp;", "<": "&lt;", ">": "&gt;", "'": "&#39;", '"': "&quot;",
  })[character]);
}

loadHistory();
