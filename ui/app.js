const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const editor = document.getElementById("editor");
const status = document.getElementById("status");
const strip = document.getElementById("strip");

/** Matches the `![[name]]` embeds the app writes into a note. */
const EMBED = /!\[\[([^\]]+)\]\]/g;

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
  refreshStrip();
}

/** Names of the images the buffer currently embeds, in order, deduped. */
function embeddedNames() {
  return [...new Set(Array.from(editor.value.matchAll(EMBED), (m) => m[1]))];
}

async function refreshStrip() {
  const names = embeddedNames();
  if (!names.length) {
    strip.hidden = true;
    strip.replaceChildren();
    return;
  }

  let found = [];
  try {
    found = await invoke("thumbnails", { names });
  } catch (e) {
    setStatus(String(e), "error");
  }

  const byName = new Map(found.map((p) => [p.name, p.thumb]));
  strip.replaceChildren(
    ...names.map((name) => {
      const thumb = byName.get(name);
      if (!thumb) {
        // An embed pointing at a file that is not there: better shown than hidden.
        const el = document.createElement("span");
        el.className = "missing";
        el.textContent = name;
        return el;
      }
      const img = document.createElement("img");
      img.src = thumb;
      img.alt = name;
      img.title = name;
      return img;
    })
  );
  strip.hidden = false;
}

/** Put an embed on its own line at the cursor. */
function insertEmbed(name) {
  const { selectionStart: start, selectionEnd: end, value } = editor;
  const before = value.slice(0, start);
  const needsBreak = before.length > 0 && !before.endsWith("\n");
  const text = `${needsBreak ? "\n" : ""}![[${name}]]\n`;

  editor.value = before + text + value.slice(end);
  const caret = start + text.length;
  editor.setSelectionRange(caret, caret);
  editor.focus();
}

async function pasteImage(announce) {
  try {
    const { name } = await invoke("paste_image");
    insertEmbed(name);
    queueDraftSave();
    refreshStrip();
    setStatus(`added ${name}`, "saved");
    return true;
  } catch (e) {
    // Only complain when the clipboard looked like it held an image; otherwise
    // this was an ordinary paste that simply had nothing for us.
    if (announce) setStatus(String(e), "error");
    return false;
  }
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
    refreshStrip();
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

editor.addEventListener("input", () => {
  queueDraftSave();
  refreshStrip();
});

editor.addEventListener("paste", (e) => {
  const types = Array.from(e.clipboardData?.types ?? []);
  const looksLikeImage = types.some((t) => t.startsWith("image/"));
  // WebKitGTK and WKWebView do not agree on how an image paste is advertised,
  // so anything without plain text is also worth offering to the image path
  // rather than dropping on the floor.
  if (!looksLikeImage && types.includes("text/plain")) return;

  e.preventDefault();
  pasteImage(looksLikeImage);
});

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
