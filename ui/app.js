const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const editor = document.getElementById("editor");
const strip = document.getElementById("strip");
const status = document.getElementById("status");
const hints = document.getElementById("hints");
const query = document.getElementById("query");
const results = document.getElementById("results");

const panes = {
  capture: document.getElementById("capture"),
  search: document.getElementById("search"),
  viewer: document.getElementById("viewer"),
};

/** Matches the `![[name]]` embeds the app writes into a note. */
const EMBED = /!\[\[([^\]]+)\]\]/g;
/** Typed into an empty buffer, this opens search. See DESIGN.md §6. */
const SEARCH_TRIGGER = "//";

const DRAFT_DEBOUNCE_MS = 400;
const SEARCH_DEBOUNCE_MS = 90;
/** Long enough to read the confirmation, short enough not to be in the way. */
const CONFIRM_MS = 450;

const HINTS = {
  capture: "<kbd>Ctrl</kbd><kbd>↵</kbd> save &nbsp; <kbd>//</kbd> search &nbsp; <kbd>Esc</kbd> dismiss",
  search: "<kbd>↑</kbd><kbd>↓</kbd> move &nbsp; <kbd>↵</kbd> open &nbsp; <kbd>Esc</kbd> back",
  viewer: "<kbd>Esc</kbd> back to results",
};

let mode = "capture";
let hits = [];
let selected = 0;
let draftTimer = null;
let searchTimer = null;
let statusTimer = null;

function setStatus(text, kind = "") {
  clearTimeout(statusTimer);
  status.textContent = text;
  status.className = kind;
  if (text) statusTimer = setTimeout(() => setStatus(""), 2600);
}

function setMode(next) {
  mode = next;
  for (const [name, pane] of Object.entries(panes)) pane.hidden = name !== next;
  hints.innerHTML = HINTS[next];
}

/* ── capture ─────────────────────────────────────────────────────────────── */

function focusEditor() {
  editor.focus();
  // Cursor at the end, so resuming a draft continues where it stopped.
  editor.setSelectionRange(editor.value.length, editor.value.length);
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
  strip.replaceChildren(...names.map((name) => thumbFor(name, byName.get(name))));
  strip.hidden = false;
}

function thumbFor(name, thumb) {
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
  } catch (e) {
    // Only complain when the clipboard looked like it held an image; otherwise
    // this was an ordinary paste that simply had nothing for us.
    if (announce) setStatus(String(e), "error");
  }
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

/* ── search ──────────────────────────────────────────────────────────────── */

function openSearch() {
  setMode("search");
  query.value = "";
  query.focus();
  runSearch();
}

function closeSearch() {
  setMode("capture");
  focusEditor();
}

function queueSearch() {
  clearTimeout(searchTimer);
  searchTimer = setTimeout(runSearch, SEARCH_DEBOUNCE_MS);
}

async function runSearch() {
  try {
    hits = await invoke("search", { query: query.value });
  } catch (e) {
    hits = [];
    setStatus(String(e), "error");
  }
  selected = 0;
  renderResults();
}

function renderResults() {
  if (!hits.length) {
    const li = document.createElement("li");
    li.className = "empty";
    li.textContent = query.value.trim() ? "No notes match." : "No notes yet.";
    results.replaceChildren(li);
    return;
  }

  results.replaceChildren(
    ...hits.map((hit, i) => {
      const li = document.createElement("li");
      li.setAttribute("aria-selected", String(i === selected));

      const row = document.createElement("div");
      row.className = "row";
      const title = document.createElement("span");
      title.className = "title";
      title.textContent = hit.title;
      const when = document.createElement("span");
      when.className = "when";
      when.textContent = hit.when;
      row.append(title, when);

      const snippet = document.createElement("div");
      snippet.className = "snippet";
      snippet.append(...markSnippet(hit.snippet));

      li.append(row, snippet);
      li.addEventListener("click", () => {
        selected = i;
        openSelected();
      });
      return li;
    })
  );
  results.children[selected]?.scrollIntoView({ block: "nearest" });
}

/**
 * SQLite marks matches with »…«, chosen because they cannot appear in a note by
 * accident. Rebuilding the runs as nodes keeps note text out of innerHTML.
 */
function markSnippet(snippet) {
  return snippet.split("»").flatMap((part, i) => {
    if (i === 0) return [document.createTextNode(part)];
    const [hit, ...rest] = part.split("«");
    const mark = document.createElement("mark");
    mark.textContent = hit;
    return [mark, document.createTextNode(rest.join("«"))];
  });
}

function move(delta) {
  if (!hits.length) return;
  selected = (selected + delta + hits.length) % hits.length;
  renderResults();
}

async function openSelected() {
  const hit = hits[selected];
  if (!hit) return;
  try {
    const note = await invoke("open_note", { path: hit.path });
    showNote(note);
  } catch (e) {
    setStatus(String(e), "error");
  }
}

/* ── viewer ──────────────────────────────────────────────────────────────── */

function showNote(note) {
  document.getElementById("viewer-title").textContent = note.title;
  document.getElementById("viewer-when").textContent = note.when;
  document.getElementById("viewer-body").textContent = note.body;
  document
    .getElementById("viewer-images")
    .replaceChildren(...note.images.map((i) => thumbFor(i.name, i.thumb)));
  setMode("viewer");
  panes.viewer.focus();
}

/* ── keys ────────────────────────────────────────────────────────────────── */

editor.addEventListener("input", () => {
  // `//` in an empty buffer is the search trigger, not text.
  if (editor.value === SEARCH_TRIGGER) {
    editor.value = "";
    openSearch();
    return;
  }
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

query.addEventListener("input", queueSearch);

query.addEventListener("keydown", (e) => {
  switch (e.key) {
    case "ArrowDown":
      e.preventDefault();
      move(1);
      break;
    case "ArrowUp":
      e.preventDefault();
      move(-1);
      break;
    case "Enter":
      e.preventDefault();
      openSelected();
      break;
    case "Escape":
      e.preventDefault();
      closeSearch();
      break;
  }
});

// The viewer has no input of its own, so its keys are caught on the way up.
document.addEventListener("keydown", (e) => {
  if (mode !== "viewer") return;
  if (e.key === "Escape") {
    e.preventDefault();
    setMode("search");
    query.focus();
  }
});

// The window is reused rather than recreated, so every hotkey press after the
// first arrives as this event instead of a page load.
listen("photomem://present", () => {
  setMode("capture");
  restoreDraft();
  invoke("refresh").catch(() => {});
});

setMode("capture");
restoreDraft();
invoke("refresh").catch((e) => setStatus(String(e), "error"));
