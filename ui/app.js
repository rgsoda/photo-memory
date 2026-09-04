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
/** Matches a `[[name]]` reference to another note. */
const LINK = /\[\[([^\]!]+?)\]\]/g;
/**
 * Both open the same picker. `//` is the one key to remember; `[[` matches the
 * on-disk format and carries Obsidian muscle memory. See DESIGN.md §6.
 */
const TRIGGERS = ["//", "[["];

const DRAFT_DEBOUNCE_MS = 400;
const SEARCH_DEBOUNCE_MS = 90;
/** Long enough to read the confirmation, short enough not to be in the way. */
const CONFIRM_MS = 450;

const HINTS = {
  capture: "<kbd>Ctrl</kbd><kbd>↵</kbd> save &nbsp; <kbd>//</kbd> search &amp; cite &nbsp; <kbd>Esc</kbd> dismiss",
  search: "<kbd>↑</kbd><kbd>↓</kbd> move &nbsp; <kbd>↵</kbd> open &nbsp; <kbd>Esc</kbd> back",
  viewer: "<kbd>Tab</kbd><kbd>↵</kbd> follow link &nbsp; <kbd>V</kbd> view image &nbsp; <kbd>Esc</kbd> back to results",
  pick: "<kbd>↑</kbd><kbd>↓</kbd> move &nbsp; <kbd>↵</kbd> cite &nbsp; <kbd>Ctrl</kbd><kbd>↵</kbd> supersedes &nbsp; <kbd>Esc</kbd> cancel",
};

let mode = "capture";
let hits = [];
let selected = 0;
/** Image names the open note embeds, for stepping through in the image window. */
let gallery = [];
/**
 * While the picker is up: the trigger's character range in the editor, which
 * the chosen reference replaces. Null in every other mode.
 */
let pick = null;
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
  // The picker is the search pane with a different answer to Enter, exactly as
  // designed: same index, same widget, same keystroke.
  const pane = next === "pick" ? "search" : next;
  for (const [name, el] of Object.entries(panes)) el.hidden = name !== pane;
  hints.innerHTML = HINTS[next];
}

/* ── images ──────────────────────────────────────────────────────────────── */

/** Open the image window on `name`, with the note's other images alongside it. */
function openImage(name, names) {
  invoke("open_image", { names, index: names.indexOf(name) }).catch((e) =>
    setStatus(String(e), "error")
  );
}

function thumbFor(name, thumb, names = null) {
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
  img.title = `${name} — click to view full size`;
  img.addEventListener("click", () => openImage(name, names ?? [name]));
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

/* ── draft ───────────────────────────────────────────────────────────────── */

/** Put the caret at the end, which is where typing should continue. */
function focusEditor() {
  editor.focus();
  const end = editor.value.length;
  editor.setSelectionRange(end, end);
}

/** Write the buffer out shortly after typing stops, so Escape never loses it. */
function queueDraftSave() {
  clearTimeout(draftTimer);
  draftTimer = setTimeout(async () => {
    try {
      await invoke("save_draft", { text: editor.value });
    } catch (e) {
      setStatus(String(e), "error");
    }
  }, DRAFT_DEBOUNCE_MS);
}

/** Bring back whatever was in the buffer when the window was last dismissed. */
async function restoreDraft() {
  try {
    editor.value = await invoke("load_draft");
  } catch (e) {
    setStatus(String(e), "error");
  }
  focusEditor();
  refreshStrip();
}

/** Show a thumbnail for each `![[name]]` the buffer currently embeds.
 *
 * The strip mirrors the embeds rather than tracking pastes: an embed line the
 * user deleted by hand should take its picture with it.
 */
async function refreshStrip() {
  const names = Array.from(editor.value.matchAll(EMBED), (m) => m[1]);
  strip.hidden = names.length === 0;
  if (!names.length) return strip.replaceChildren();

  let thumbs = [];
  try {
    thumbs = await invoke("thumbnails", { names });
  } catch (e) {
    setStatus(String(e), "error");
  }
  const found = new Map(thumbs.map((t) => [t.name, t.thumb]));
  strip.replaceChildren(...names.map((n) => thumbFor(n, found.get(n), names)));
}

/* ── search ──────────────────────────────────────────────────────────────── */

function openSearch() {
  pick = null;
  setMode("search");
  query.placeholder = "Search notes…";
  query.value = "";
  query.focus();
  runSearch();
}

function closeSearch() {
  pick = null;
  setMode("capture");
  focusEditor();
}

/* ── citing another note ─────────────────────────────────────────────────── */

/**
 * A trigger only fires at the start of a word, which is what keeps `https://`
 * and a pasted `// comment` from opening the picker mid-sentence. Returns the
 * range the trigger occupies, or null.
 */
function triggerAt(value, caret) {
  for (const t of TRIGGERS) {
    const start = caret - t.length;
    if (start < 0 || value.slice(start, caret) !== t) continue;
    if (start > 0 && !/\s/.test(value[start - 1])) continue;
    return { start, end: caret };
  }
  return null;
}

/** Open the picker over the trigger the user just typed. */
function openPicker(range) {
  pick = range;
  setMode("pick");
  query.placeholder = "Cite a note…";
  query.value = "";
  query.focus();
  runSearch();
}

/** Put `text` where the trigger was and go back to typing. */
function replaceTrigger(text) {
  const { start, end } = pick;
  const value = editor.value;
  editor.value = value.slice(0, start) + text + value.slice(end);
  const caret = start + text.length;
  pick = null;
  setMode("capture");
  editor.focus();
  editor.setSelectionRange(caret, caret);
  queueDraftSave();
}

/**
 * Cite the selected note at the cursor.
 *
 * `supersedes` gets its own line because it is a claim about the note as a
 * whole, not about the sentence it happens to sit in; the index reads it from
 * the line. See DESIGN.md §6.
 */
function cite(supersedes) {
  const hit = hits[selected];
  if (!hit) return;
  const link = `[[${hit.name}]]`;
  if (!supersedes) return replaceTrigger(link);

  const before = editor.value.slice(0, pick.start);
  const lead = before.length && !before.endsWith("\n") ? "\n" : "";
  replaceTrigger(`${lead}supersedes: ${link}\n`);
}

/** Esc leaves the literal trigger behind — it may well have been real text. */
function cancelPicker() {
  const caret = pick.end;
  pick = null;
  setMode("capture");
  editor.focus();
  editor.setSelectionRange(caret, caret);
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
        if (pick) cite(false);
        else openSelected();
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

/** Follow a `[[link]]` to the note it names. */
async function openLink(name) {
  try {
    showNote(await invoke("open_link", { name }));
  } catch (e) {
    setStatus(String(e), "error");
  }
}

/**
 * Rebuild the body with its `[[links]]` as clickable nodes.
 *
 * Built as nodes rather than markup because note text must never reach
 * innerHTML — the same reason `markSnippet` works this way.
 */
function linkedBody(body) {
  const out = [];
  let at = 0;
  for (const m of body.matchAll(LINK)) {
    out.push(document.createTextNode(body.slice(at, m.index)));
    const a = document.createElement("a");
    a.className = "link";
    a.textContent = m[1];
    a.title = `open ${m[1]}`;
    // Reachable by Tab, because nothing else in this app needs a mouse.
    a.tabIndex = 0;
    a.addEventListener("click", () => openLink(m[1]));
    a.addEventListener("keydown", (e) => {
      if (e.key !== "Enter") return;
      e.preventDefault();
      openLink(m[1]);
    });
    out.push(a);
    at = m.index + m[0].length;
  }
  out.push(document.createTextNode(body.slice(at)));
  return out;
}

function showNote(note) {
  document.getElementById("viewer-title").replaceChildren(...linkedBody(note.title));
  document.getElementById("viewer-when").textContent = note.when;
  document.getElementById("viewer-body").replaceChildren(...linkedBody(note.body));

  gallery = note.images.map((i) => i.name);
  document
    .getElementById("viewer-images")
    .replaceChildren(...note.images.map((i) => thumbFor(i.name, i.thumb, gallery)));

  setMode("viewer");
  panes.viewer.focus();
}

/* ── keys ────────────────────────────────────────────────────────────────── */

editor.addEventListener("input", () => {
  const range = triggerAt(editor.value, editor.selectionStart);
  if (range) {
    // Same trigger, two meanings, decided by whether there is a note in
    // progress: an empty buffer means "find my old notes", anywhere else means
    // "cite one here".
    const rest = editor.value.slice(0, range.start) + editor.value.slice(range.end);
    if (rest.trim() === "") {
      editor.value = "";
      openSearch();
    } else {
      openPicker(range);
    }
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
      if (pick) cite(e.ctrlKey || e.metaKey);
      else openSelected();
      break;
    case "Escape":
      e.preventDefault();
      if (pick) cancelPicker();
      else closeSearch();
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
  } else if (e.key === "v" && gallery.length) {
    e.preventDefault();
    openImage(gallery[0], gallery);
  }
});

// Focus returns to the window without the page being told which element should
// have it — after the image window closes, or after a click on the frame. The
// editor is the only thing here worth typing into, so it always takes it back.
window.addEventListener("focus", () => {
  if (mode === "search" || mode === "pick") query.focus();
  else if (mode === "capture") focusEditor();
});

// The window is reused rather than recreated, so every hotkey press after the
// first arrives as this event instead of a page load.
listen("photomem://present", () => {
  invoke("close_image").catch(() => {});
  setMode("capture");
  restoreDraft();
});

setMode("capture");
restoreDraft();
