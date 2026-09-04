const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const editor = document.getElementById("editor");
const strip = document.getElementById("strip");
const status = document.getElementById("status");
const hints = document.getElementById("hints");
const query = document.getElementById("query");
const results = document.getElementById("results");
const superseded = document.getElementById("viewer-superseded");
const backlinks = document.getElementById("viewer-backlinks");

const panes = {
  capture: document.getElementById("capture"),
  search: document.getElementById("search"),
  viewer: document.getElementById("viewer"),
  wall: document.getElementById("wall"),
  timeline: document.getElementById("timeline"),
  tags: document.getElementById("tags"),
};

/** The browse views `Tab` cycles through, search included. */
const VIEWS = ["search", "wall", "timeline"];
/** How the timeline groups its rows. `g` cycles these. */
const GROUPINGS = ["day", "week", "month"];

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
  search: "<kbd>↑</kbd><kbd>↓</kbd> move &nbsp; <kbd>↵</kbd> open &nbsp; <kbd>Tab</kbd> browse &nbsp; <kbd>Esc</kbd> back",
  viewer: "<kbd>↑</kbd><kbd>↓</kbd> scroll &nbsp; <kbd>Tab</kbd><kbd>↵</kbd> link &nbsp; <kbd>V</kbd> image &nbsp; <kbd>Esc</kbd> back",
  wall: "<kbd>←</kbd><kbd>→</kbd><kbd>↑</kbd><kbd>↓</kbd> move &nbsp; <kbd>↵</kbd> note &nbsp; <kbd>V</kbd> image &nbsp; <kbd>T</kbd> tag &nbsp; <kbd>Tab</kbd> next &nbsp; <kbd>Esc</kbd> back",
  timeline: "<kbd>↑</kbd><kbd>↓</kbd> move &nbsp; <kbd>↵</kbd> open &nbsp; <kbd>G</kbd> group &nbsp; <kbd>T</kbd> tag &nbsp; <kbd>Tab</kbd> next &nbsp; <kbd>Esc</kbd> back",
  tags: "<kbd>↑</kbd><kbd>↓</kbd> move &nbsp; <kbd>↵</kbd> filter &nbsp; <kbd>Esc</kbd> cancel",
  pickTag: "<kbd>↑</kbd><kbd>↓</kbd> move &nbsp; <kbd>↵</kbd> insert tag &nbsp; <kbd>Esc</kbd> keep the #",
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
/** Tiles on the thumbnail wall, and which one is under the cursor. */
let shots = [];
let shot = 0;
/** Rows of the timeline, which one is selected, and how it is grouped. */
let entries = [];
let entry = 0;
let grouping = "day";
/** The tag both browse views are filtered to, or null for everything. */
let filter = null;
/** The tag list, and where the picker sits in it. */
let tagList = [];
let tagAt = 0;
/** Which view the tag picker was opened from, to return to. */
let tagFrom = "timeline";
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
  hints.innerHTML = HINTS[next === "pick" && pick?.kind === "tag" ? "pickTag" : next];
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

/* ── the thumbnail wall ──────────────────────────────────────────────────── */

async function openWall() {
  setMode("wall");
  panes.wall.replaceChildren();
  try {
    shots = await invoke("wall", { tag: filter });
  } catch (e) {
    shots = [];
    setStatus(String(e), "error");
  }
  shot = 0;
  renderWall();
  panes.wall.scrollTop = 0;
}

function renderWall() {
  if (!shots.length) {
    const empty = document.createElement("p");
    empty.className = "empty";
    empty.textContent = filter
      ? `No pictures in notes tagged #${filter}.`
      : "No pictures captured yet.";
    panes.wall.replaceChildren(empty);
    return;
  }

  panes.wall.replaceChildren(
    ...(filter ? [filterChip()] : []),
    ...shots.map((s, i) => {
      const tile = document.createElement("figure");
      tile.setAttribute("aria-selected", String(i === shot));

      const img = document.createElement("img");
      img.src = s.thumb;
      img.alt = s.name;
      tile.append(img);

      const caption = document.createElement("figcaption");
      caption.textContent = s.title;
      caption.title = `${s.title} — ${s.when}`;
      tile.append(caption);

      // A click is a shortcut for "select this and open its note", which is
      // what Enter does — the mouse should not be able to do something the
      // keyboard cannot.
      tile.addEventListener("click", () => {
        shot = i;
        openShotNote();
      });
      return tile;
    })
  );
  // The chip, when present, is the first child — so tiles are offset by one.
  panes.wall.children[shot + (filter ? 1 : 0)]?.scrollIntoView({ block: "nearest" });
}

/**
 * How many tiles sit on one row, read back from the layout.
 *
 * The grid reflows with the window, so the column count is a fact about the
 * rendered page rather than a constant to keep in step with the CSS.
 */
function wallColumns() {
  const tiles = [...panes.wall.children].filter((el) => el.tagName === "FIGURE");
  if (tiles.length < 2) return 1;
  const top = tiles[0].offsetTop;
  let n = 1;
  while (n < tiles.length && tiles[n].offsetTop === top) n += 1;
  return n;
}

function moveShot(delta) {
  if (!shots.length) return;
  shot = Math.min(Math.max(shot + delta, 0), shots.length - 1);
  renderWall();
}

/** Open the note the selected picture belongs to. */
async function openShotNote() {
  const s = shots[shot];
  if (!s) return;
  try {
    showNote(await invoke("open_note", { path: s.path }));
  } catch (e) {
    setStatus(String(e), "error");
  }
}

/* ── tags ────────────────────────────────────────────────────────────────── */

/** Fetch the tag list, cached for the picker and the autocomplete alike. */
async function loadTags() {
  try {
    tagList = await invoke("tags");
  } catch (e) {
    tagList = [];
    setStatus(String(e), "error");
  }
  return tagList;
}

/** Open the tag filter over whichever browse view asked for it. */
async function openTagFilter() {
  tagFrom = mode;
  setMode("tags");
  await loadTags();
  // "All notes" is first and is what Escape-by-another-name looks like: there
  // has to be a way out of a filter that does not require remembering one.
  tagAt = filter ? 1 + tagList.findIndex((t) => t.tag === filter) : 0;
  if (tagAt < 1) tagAt = 0;
  renderTagFilter();
}

function renderTagFilter() {
  const rows = [{ tag: null, count: null }, ...tagList];
  panes.tags.querySelector("ul").replaceChildren(
    ...rows.map((row, i) => {
      const li = document.createElement("li");
      li.setAttribute("aria-selected", String(i === tagAt));

      const name = document.createElement("span");
      name.textContent = row.tag ? `#${row.tag}` : "All notes";
      const count = document.createElement("span");
      count.className = "count";
      count.textContent = row.count === null ? "" : `${row.count}`;

      li.append(name, count);
      li.addEventListener("click", () => {
        tagAt = i;
        applyTagFilter();
      });
      return li;
    })
  );
  if (!tagList.length) {
    const li = document.createElement("li");
    li.className = "count";
    li.textContent = "No tags yet — write #like-this in a note.";
    panes.tags.querySelector("ul").append(li);
  }
  panes.tags.children[0].children[tagAt]?.scrollIntoView({ block: "nearest" });
}

function moveTag(delta) {
  const count = tagList.length + 1;
  tagAt = Math.min(Math.max(tagAt + delta, 0), count - 1);
  renderTagFilter();
}

function applyTagFilter() {
  filter = tagAt === 0 ? null : tagList[tagAt - 1].tag;
  if (tagFrom === "wall") openWall();
  else openTimeline();
}

/** The chip that says a view is showing less than everything. */
function filterChip() {
  const chip = document.createElement("div");
  chip.className = "filter";
  chip.textContent = `#${filter} — T to change, Esc to clear`;
  return chip;
}

/* ── the timeline ────────────────────────────────────────────────────────── */

async function openTimeline() {
  setMode("timeline");
  panes.timeline.replaceChildren();
  try {
    entries = await invoke("timeline", { tag: filter });
  } catch (e) {
    entries = [];
    setStatus(String(e), "error");
  }
  entry = 0;
  renderTimeline();
  panes.timeline.scrollTop = 0;
}

function renderTimeline() {
  if (!entries.length) {
    const empty = document.createElement("p");
    empty.className = "empty";
    empty.textContent = filter ? `No notes tagged #${filter}.` : "No notes yet.";
    panes.timeline.replaceChildren(empty);
    return;
  }

  const out = filter ? [filterChip()] : [];
  let list = null;
  let heading = null;

  entries.forEach((item, i) => {
    // The rows arrive newest first and every grouping is coarser than the
    // ordering, so a group boundary is simply the label changing.
    if (item[grouping] !== heading) {
      heading = item[grouping];
      const h = document.createElement("h2");
      h.textContent = heading;
      list = document.createElement("ul");
      out.push(h, list);
    }

    const li = document.createElement("li");
    li.setAttribute("aria-selected", String(i === entry));

    const at = document.createElement("span");
    at.className = "at";
    at.textContent = item.at;
    const title = document.createElement("span");
    title.className = "title";
    title.textContent = item.title;

    li.append(at, title);
    li.addEventListener("click", () => {
      entry = i;
      openEntry();
    });
    list.append(li);
  });

  panes.timeline.replaceChildren(...out);
  selectedRow()?.scrollIntoView({ block: "nearest" });
}

/** The selected row, which is nested under its heading rather than a child of
 *  the pane — so it cannot be found by index the way the wall's tiles can. */
function selectedRow() {
  return panes.timeline.querySelector('li[aria-selected="true"]');
}

function moveEntry(delta) {
  if (!entries.length) return;
  entry = Math.min(Math.max(entry + delta, 0), entries.length - 1);
  renderTimeline();
}

async function openEntry() {
  const item = entries[entry];
  if (!item) return;
  try {
    showNote(await invoke("open_note", { path: item.path }));
  } catch (e) {
    setStatus(String(e), "error");
  }
}

/** Regroup, keeping the selected note selected: the point of switching is to
 *  see the same note in a wider or narrower context, not to lose your place. */
function cycleGrouping() {
  grouping = GROUPINGS[(GROUPINGS.indexOf(grouping) + 1) % GROUPINGS.length];
  renderTimeline();
  setStatus(`grouped by ${grouping}`);
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
    return { start, end: caret, kind: "note" };
  }
  // `#` at the start of a word offers the tags already in use, which is the
  // only thing that keeps a free-form tag from becoming five spellings of
  // itself. It is not consumed: what is typed stays in the note either way.
  const start = caret - 1;
  if (start >= 0 && value[start] === "#" && (start === 0 || /\s/.test(value[start - 1]))) {
    return { start, end: caret, kind: "tag" };
  }
  return null;
}

/** Open the picker over the trigger the user just typed. */
async function openPicker(range) {
  pick = range;
  setMode("pick");
  const tagging = range.kind === "tag";
  query.placeholder = tagging ? "Tag…" : "Cite a note…";
  query.value = "";
  query.focus();
  if (tagging) await loadTags();
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
  // A tag is text in the sentence, so it takes the trailing space that keeps
  // typing flowing — a citation is a reference, and gets none.
  if (pick.kind === "tag") return replaceTrigger(`#${hit.tag} `);
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
  if (pick?.kind === "tag") {
    // Filtered here rather than in SQL: the whole tag list is small, already
    // fetched, and a round trip per keystroke would be slower than the typing.
    const typed = query.value.trim().toLowerCase();
    hits = tagList
      .filter((t) => t.tag.includes(typed))
      .map((t) => ({ tag: t.tag, title: `#${t.tag}`, when: `${t.count}`, snippet: "" }));
  } else {
    try {
      hits = await invoke("search", { query: query.value });
    } catch (e) {
      hits = [];
      setStatus(String(e), "error");
    }
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

/** One arrow-key press of scrolling: a few lines, not one pixel. */
const LINE = 60;

/**
 * How far this key should scroll the open note, or null if it is not a scroll
 * key.
 *
 * The pane is scrolled explicitly rather than by focusing it and leaving this
 * to the browser: a `<section>` takes no focus of its own, and the moment a
 * `[[link]]` inside it is tabbed to, the focus that native scrolling depends on
 * belongs to the link instead.
 */
function scrollStep(e) {
  if (e.ctrlKey || e.altKey || e.metaKey) return null;
  const page = panes.viewer.clientHeight - LINE;
  switch (e.key) {
    case "ArrowDown":
    case "j":
      return LINE;
    case "ArrowUp":
    case "k":
      return -LINE;
    case "PageDown":
    case " ":
      return page;
    case "PageUp":
      return -page;
    // Home and End are absolute, so they are expressed as a step big enough to
    // reach either end whatever the note's length.
    case "Home":
      return -panes.viewer.scrollHeight;
    case "End":
      return panes.viewer.scrollHeight;
    default:
      return null;
  }
}

/** Follow a `[[link]]` to the note it names. */
async function openLink(name) {
  try {
    showNote(await invoke("open_link", { name }));
  } catch (e) {
    setStatus(String(e), "error");
  }
}

/**
 * A clickable reference to another note.
 *
 * Built as a node rather than markup because note text must never reach
 * innerHTML — the same reason `markSnippet` works this way. `label` differs
 * from `name` wherever there is a title worth showing instead of a filename.
 */
function noteLink(name, label = name) {
  const a = document.createElement("a");
  a.className = "link";
  a.textContent = label;
  a.title = `open ${name}`;
  // Reachable by Tab, because nothing else in this app needs a mouse.
  a.tabIndex = 0;
  a.addEventListener("click", () => openLink(name));
  a.addEventListener("keydown", (e) => {
    if (e.key !== "Enter") return;
    e.preventDefault();
    openLink(name);
  });
  return a;
}

/** Rebuild the body with its `[[links]]` as clickable nodes. */
function linkedBody(body) {
  const out = [];
  let at = 0;
  for (const m of body.matchAll(LINK)) {
    out.push(document.createTextNode(body.slice(at, m.index)));
    out.push(noteLink(m[1]));
    at = m.index + m[0].length;
  }
  out.push(document.createTextNode(body.slice(at)));
  return out;
}

/**
 * The banner that keeps append-only honest.
 *
 * A plain backlink is too quiet to carry "this was later proved wrong", which
 * is why supersession is a typed link and why this sits above the title rather
 * than in the list at the bottom. See DESIGN.md §6.
 */
function showSupersession(ref) {
  superseded.hidden = !ref;
  if (!ref) return;
  superseded.replaceChildren(
    document.createTextNode("⚠ Superseded by "),
    noteLink(ref.name, ref.title),
    document.createTextNode(` — ${ref.when}`)
  );
}

/** Notes pointing at this one, newest first. */
function showBacklinks(refs) {
  backlinks.hidden = refs.length === 0;
  if (!refs.length) return;
  backlinks.querySelector("ul").replaceChildren(
    ...refs.map((ref) => {
      const li = document.createElement("li");
      const when = document.createElement("span");
      when.className = "when";
      when.textContent = ref.when;
      li.append(noteLink(ref.name, ref.title), when);
      return li;
    })
  );
}

function showNote(note) {
  document.getElementById("viewer-title").replaceChildren(...linkedBody(note.title));
  document.getElementById("viewer-when").textContent = note.when;
  document.getElementById("viewer-body").replaceChildren(...linkedBody(note.body));

  showSupersession(note.superseded_by);
  showBacklinks(note.backlinks);

  gallery = note.images.map((i) => i.name);
  document
    .getElementById("viewer-images")
    .replaceChildren(...note.images.map((i) => thumbFor(i.name, i.thumb, gallery)));

  setMode("viewer");
  panes.viewer.scrollTop = 0;
}

/* ── keys ────────────────────────────────────────────────────────────────── */

editor.addEventListener("input", () => {
  const range = triggerAt(editor.value, editor.selectionStart);
  if (range) {
    // Same trigger, two meanings, decided by whether there is a note in
    // progress: an empty buffer means "find my old notes", anywhere else means
    // "cite one here".
    const rest = editor.value.slice(0, range.start) + editor.value.slice(range.end);
    if (range.kind === "note" && rest.trim() === "") {
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
    case "Tab":
      // Not from the picker: there, you are part-way through writing a note.
      if (pick) break;
      e.preventDefault();
      nextView();
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
  // The editor and the query field handle their own keys, and their events
  // bubble to here afterwards. Without this, one Tab in the search box would
  // open the wall and then be read again as a Tab *on* the wall, skipping a
  // view — the handler must not see a key that has already been acted on.
  if (e.target === query || e.target === editor) return;

  if (mode === "wall") return wallKey(e);
  if (mode === "timeline") return timelineKey(e);
  if (mode === "tags") return tagKey(e);
  if (mode !== "viewer") return;

  const step = scrollStep(e);
  if (step !== null) {
    e.preventDefault();
    panes.viewer.scrollBy({ top: step, behavior: "instant" });
    return;
  }

  if (e.key === "Escape") {
    e.preventDefault();
    setMode("search");
    query.focus();
  } else if (e.key === "v" && gallery.length) {
    e.preventDefault();
    openImage(gallery[0], gallery);
  }
});

/** Move to the next browse view, wrapping back round to search. */
function nextView() {
  const next = VIEWS[(VIEWS.indexOf(mode) + 1) % VIEWS.length];
  if (next === "wall") return openWall();
  if (next === "timeline") return openTimeline();
  openSearch();
}

function timelineKey(e) {
  switch (e.key) {
    case "ArrowDown":
      e.preventDefault();
      return moveEntry(1);
    case "ArrowUp":
      e.preventDefault();
      return moveEntry(-1);
    case "PageDown":
      e.preventDefault();
      return moveEntry(10);
    case "PageUp":
      e.preventDefault();
      return moveEntry(-10);
    case "Enter":
      e.preventDefault();
      return openEntry();
    case "g":
      e.preventDefault();
      return cycleGrouping();
    case "t":
      e.preventDefault();
      return openTagFilter();
    case "Tab":
      e.preventDefault();
      return nextView();
    case "Escape":
      e.preventDefault();
      // Clear the filter first, leave second: Escape should undo the most
      // recent narrowing, not skip past it.
      if (filter) {
        filter = null;
        return openTimeline();
      }
      setMode("search");
      query.focus();
      return;
  }
}

function tagKey(e) {
  switch (e.key) {
    case "ArrowDown":
      e.preventDefault();
      return moveTag(1);
    case "ArrowUp":
      e.preventDefault();
      return moveTag(-1);
    case "Enter":
      e.preventDefault();
      return applyTagFilter();
    case "Escape":
      e.preventDefault();
      return tagFrom === "wall" ? openWall() : openTimeline();
  }
}

/** The wall is a grid, so it moves in two dimensions. */
function wallKey(e) {
  const columns = wallColumns();
  const step = { ArrowRight: 1, ArrowLeft: -1, ArrowDown: columns, ArrowUp: -columns }[e.key];
  if (step !== undefined) {
    e.preventDefault();
    return moveShot(step);
  }
  switch (e.key) {
    case "Enter":
      e.preventDefault();
      return openShotNote();
    case "v":
      e.preventDefault();
      // Every picture on the wall, so the arrows keep working in the image
      // window — the wall is the one place where stepping through everything
      // captured is the obvious thing to want.
      if (shots.length) openImage(shots[shot].name, shots.map((s) => s.name));
      return;
    case "t":
      e.preventDefault();
      return openTagFilter();
    case "Tab":
      e.preventDefault();
      return nextView();
    case "Escape":
      e.preventDefault();
      if (filter) {
        filter = null;
        return openWall();
      }
      setMode("search");
      query.focus();
      return;
  }
}

// Focus returns to the window without the page being told which element should
// have it — after the image window closes, or after a click on the frame. The
// editor is the only thing here worth typing into, so it always takes it back.
window.addEventListener("focus", () => {
  if (mode === "search" || mode === "pick") query.focus();
  else if (mode === "capture") focusEditor();
});

// Sync runs on its own thread and finishes whenever it finishes — usually after
// the window has already hidden itself, which is why the backend also logs it.
listen("photomem://sync", ({ payload: [text, failed] }) => {
  setStatus(text, failed ? "error" : "saved");
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
