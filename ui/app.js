const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const editor = document.getElementById("editor");
const status = document.getElementById("status");

const DRAFT_DEBOUNCE_MS = 400;
/** Long enough to read the confirmation, short enough not to be in the way. */
const CONFIRM_MS = 450;

let draftTimer = null;
let statusTimer = null;

function setStatus(text, kind = "") {
  clearTimeout(statusTimer);
  status.textContent = text;
  status.className = kind;
  if (text) statusTimer = setTimeout(() => setStatus(""), 2600);
}

/** Drafts are what make Escape safe, so they are written on a short debounce. */
function queueDraftSave() {
  clearTimeout(draftTimer);
  draftTimer = setTimeout(() => {
    invoke("save_draft", { text: editor.value }).catch(() => {});
  }, DRAFT_DEBOUNCE_MS);
}

async function restoreDraft() {
  try {
    editor.value = await invoke("load_draft");
  } catch (e) {
    setStatus(String(e), "error");
  }
  focusEditor();
}

function focusEditor() {
  editor.focus();
  // Cursor at the end, so resuming a draft continues where it stopped.
  editor.setSelectionRange(editor.value.length, editor.value.length);
}

async function save() {
  if (!editor.value.trim()) return dismiss();
  clearTimeout(draftTimer);

  try {
    const path = await invoke("save_note", { body: editor.value });
    setStatus(`saved ${path.split("/").pop()}`, "saved");
    editor.value = "";
    // Stay visible just long enough to confirm, then get out of the way.
    setTimeout(dismiss, CONFIRM_MS);
  } catch (e) {
    setStatus(String(e), "error");
  }
}

async function dismiss() {
  clearTimeout(draftTimer);
  try {
    await invoke("dismiss", { text: editor.value });
  } catch (e) {
    setStatus(String(e), "error");
  }
}

editor.addEventListener("input", queueDraftSave);

editor.addEventListener("keydown", (e) => {
  if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) {
    e.preventDefault();
    save();
  } else if (e.key === "Escape") {
    e.preventDefault();
    dismiss();
  }
});

// The window is reused rather than recreated, so every hotkey press after the
// first arrives as this event instead of a page load.
listen("photomem://present", () => {
  restoreDraft();
});

restoreDraft();
