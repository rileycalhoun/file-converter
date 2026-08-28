import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";

const state = {
  selectedPath: null,
  conversions: [],
  polling: new Set(),
};

const elements = {
  content: document.querySelector("#content"),
  historyView: document.querySelector("#history-view"),
  searchButton: document.querySelector("#search-button"),
  backButton: document.querySelector("#back-button"),
  searchInput: document.querySelector("#search-input"),
  chooseFile: document.querySelector("#choose-file"),
  selectedFile: document.querySelector("#selected-file"),
  outputFormat: document.querySelector("#output-format"),
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
};

elements.chooseFile.addEventListener("click", async () => {
  const selected = await open({
    multiple: false,
    directory: false,
    filters: [{
      name: "Documents and images",
      extensions: ["jpg", "jpeg", "png", "ppt", "pptx", "doc", "docx", "pdf"],
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
      outputFormat: elements.outputFormat.value,
    });
    state.conversions.unshift(conversion);
    showStatus("Your file is converting. It will be saved here when finished.", true);
    beginPolling(conversion.id);
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
elements.refreshButton.addEventListener("click", refreshAll);
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
    if (action === "open") await invoke("open_conversion", { id });
    if (action === "check") beginPolling(id, true);
    if (action === "delete") {
      await invoke("delete_conversion", { id });
      state.conversions = state.conversions.filter((item) => item.id !== id);
      renderHistory();
    }
  } catch (error) {
    showStatus(String(error), false);
  }
});

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
    state.conversions
      .filter((conversion) => conversion.status === "processing")
      .forEach((conversion) => beginPolling(conversion.id));
  } catch (error) {
    showStatus(String(error), false);
  }
}

async function refreshAll() {
  const pending = state.conversions.filter((item) => item.status === "processing");
  await Promise.all(pending.map((item) => refreshOne(item.id)));
}

function beginPolling(id, immediate = false) {
  if (state.polling.has(id)) return;
  state.polling.add(id);
  const poll = async () => {
    const terminal = await refreshOne(id);
    if (terminal) {
      state.polling.delete(id);
      return;
    }
    window.setTimeout(poll, 3000);
  };
  window.setTimeout(poll, immediate ? 0 : 1500);
}

async function refreshOne(id) {
  try {
    const update = await invoke("refresh_conversion", { id });
    const index = state.conversions.findIndex((item) => item.id === id);
    if (index >= 0) state.conversions[index] = update.conversion;
    renderHistory();
    if (update.changed && update.conversion.status === "finished") {
      showStatus(`${update.conversion.sourceName} is ready.`, true);
    }
    return update.conversion.status !== "processing";
  } catch (error) {
    showStatus(String(error), false);
    return true;
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
    const action = conversion.status === "finished" ? "open" : "check";
    const actionText = conversion.status === "finished" ? "Open" : "Check";
    return `<li>
      <button class="file-link" data-action="${action}" data-id="${conversion.id}">
        ${escapeHtml(conversion.sourceName)} → ${escapeHtml(conversion.outputFormat.toUpperCase())}
      </button>
      <span class="status-label">${escapeHtml(conversion.status)}</span>
      <button data-action="${action}" data-id="${conversion.id}">${actionText}</button>
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
