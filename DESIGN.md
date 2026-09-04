# Photo Memory — Design

A small, fast, visual note capture tool. Popup from a hotkey, type a note, paste a
screenshot, done. Search everything later — including the text *inside* the screenshots.

Status: design agreed, no code written.

---

## 1. Goals and non-goals

**Goals**

- Capture in under two seconds from any application, without breaking flow.
- Images come from the clipboard. Pasting a screenshot is the primary image path.
- Atomic entries linked to each other, Obsidian-style, with backlinks.
- **Append-only.** The app never edits or deletes a note. Corrections are new notes that
  supersede old ones; nothing is ever removed.
- Runs on Linux (Wayland/Hyprland) and macOS from one codebase.
- All data in one plain-file repo, synced through a private GitHub repo.
- Small: a single background process, tens of MB on disk, instant window.

**Non-goals**

- Not a document editor. No tables, embeds, plugin API, or WYSIWYG.
- No full-resolution image archive. Screenshots are memory aids, not assets.
- No mobile client, no web client, no multi-user sync or sharing.
- No live collaborative editing. One user, one repo, occasional conflicts.

**The bet:** OCR on every captured screenshot is what makes this worth building rather
than using Obsidian. It turns pasted images from decoration into searchable memory.

---

## 2. Data model

The **files are the source of truth**. Everything else — index, thumbnails, OCR text — is
derived, gitignored, and rebuildable from scratch. If this app is ever abandoned, pointing
Obsidian at the folder loses nothing but the OCR index.

### Layout

```
photo-memory/                    # the git repo, path configurable
  notes/
    2026-09-04-1618-kafka-rebalance.md
    2026-09-04-1904-bathroom-tile-options.md
  attachments/
    2026-09-04-1618-a3f2c1.webp
    2026-09-04-1904-9b7e02.webp
  .photomem/                     # gitignored, fully derived
    index.db                     # SQLite: FTS5 + metadata + OCR text
    thumbs/                      # 320px grid thumbnails
    drafts/                      # unsaved buffer, survives Esc
  .gitignore
```

The notes directory is **flat**. No `2026/09/04/` hierarchy. Date grouping (day, week,
month, quarter) is computed by the index from the `created` timestamp, so a new grouping is
a new query, never a directory migration.

The filename is `YYYY-MM-DD-HHMM-<slug-of-title>.md`. It exists so the flat directory sorts
chronologically under `ls` and so slugs stay unique. **The app never parses the filename** —
all metadata comes from frontmatter. Renaming a file by hand is safe.

### Note format

```markdown
---
id: 01JBQX7K2M8VN4RTYCE5FZAW9H
created: 2026-09-04T16:18:42+02:00
modified: 2026-09-04T16:18:42+02:00
supersedes: [[2026-03-11-0902-kafka-poll-tuning]]   # optional
---
Kafka consumer rebalance storm

Consumers were dropping out every ~40s. `max.poll.interval.ms` was lower than the
actual batch processing time, so the broker kept evicting them mid-batch.

![[2026-09-04-a3f2c1.webp]]

Same failure mode as [[2026-05-02-1130-flink-checkpoint-timeouts]].

#work/kafka #debugging
```

- **`id`** — a ULID, assigned at creation, never changes. Links resolve by filename for
  human readability, but `id` is the stable identity that survives a rename. The index keeps
  both; a broken `[[link]]` can be repaired by matching `id`.
- **First line of the body is the title.** No separate title field, no extra keystroke. It
  becomes the filename slug and the display name in search results and link pickers.
- **`#tags`** are parsed out of the body at index time. Never declared anywhere. Typing
  `#` in the editor autocompletes from tags already in use.
- **Hierarchical tags** (`#work/kafka`) are supported by prefix match: `#work` matches
  everything beneath it. This gives a second grouping axis for free.
- **`[[wiki-links]]`** between notes. Backlinks are derived by the index.
- **`supersedes`** is an optional typed link marking an older note as corrected or
  obsoleted by this one. See §6.
- **`![[attachment.webp]]`** embeds an image.

Frontmatter stays minimal on purpose — three fields, all machine-managed. Tags and links
live in the body where they are visible and editable as text.

---

## 3. Image pipeline

On paste (Ctrl+V with an image on the clipboard):

1. **Read** — `wl-paste -t image/png` on Wayland, `NSPasteboard` on macOS. Both are
   available through the Tauri clipboard plugin, with a direct fallback if it misbehaves.
2. **Downscale** to a maximum long edge of **1600 px** (`image` crate, Lanczos3). No-op if
   already smaller, which is the common case for a window or region capture.
3. **Encode WebP at q75.** A full-screen 4K capture lands at ~104 KB, down from ~2 MB.
4. **Write** `attachments/YYYY-MM-DD-<12 hex of content hash>.webp`. Content-hashed, so the
   same image pasted twice reuses one file and filenames never collide across machines.
5. **Thumbnail** at 320 px into `.photomem/thumbs/` for the grid view.
6. **OCR in the background** (tesseract, ~200 ms) — the result goes into the index only,
   never into the markdown. The note stays clean; the search index gets the text.
   Languages come from config (`ocr.languages`), **defaulting to `["eng"]`**. Polish is
   wanted eventually but adds noise and latency, so it stays a one-line config change
   rather than a default. The index records which languages produced each attachment's
   text, so `photomem reindex --ocr` can find and re-run only the stale ones when the
   setting changes. Since the images are kept forever, re-OCR is always possible.
7. Insert `![[filename]]` at the cursor; render it inline in the editor as a thumbnail.

**Why 1600 and not 1200.** Measured on a real 3840x2160 desktop capture: at 1200 px it is
62 KB but its text is no longer readable, which defeats both the point of keeping it and
the OCR at M6. 1600 px stays legible at 104 KB; 1920 px is comfortably sharp at 145 KB.
Full-screen 4K is the worst case, so these are ceilings rather than typical costs. All
three numbers live in `config.toml` under `[image]` — someone on a 1080p screen can drop
`max_edge` and halve their repo.

**Why a 12-character hash.** Identical images are meant to collide, and dedupe. What must
never happen is two *different* images sharing a name, one silently standing in for the
other. Six hex characters is 24 bits, which reaches a coin-flip chance of that within a few
thousand images. Twelve is 48 bits and does not.

The original is **discarded**. Resolution is deliberately traded for a repo that stays small
for a decade. If a full-resolution escape hatch is ever wanted, it should be an explicit
per-paste modifier (Ctrl+Shift+V), not a default.

### Why not git-lfs

At the expected 4–5 images/day, ~100 KB each — the full-screen worst case, so real usage
lands below this:

| | |
|---|---|
| Per day | ~500 KB |
| Per year | **~180 MB** |
| GitHub comfortable repo size | ~1 GB |
| Runway | **~5 years** |

git-lfs would be *worse*: GitHub's free LFS tier is 1 GB storage and 1 GB/month bandwidth,
which is a tighter ceiling than plain git gives here, and it breaks the escape hatch —
`git clone` would no longer produce your images, and every machine needs `git lfs install`.

The usual objection to binaries in git is that each edit stores a whole new copy. These
images are **write-once and never modified**: each is stored exactly once, forever. That
failure mode does not apply.

If the ceiling is ever reached, the fix is `git filter-repo` splitting old attachments into
an archive repo — an afternoon of work, in year seven, and only if it happens.

---

## 4. Index

SQLite with FTS5, at `.photomem/index.db`, gitignored.

Tables: `notes` (id, path, title, created, modified, body), `tags` (note_id, tag),
`links` (from_id, to_id — backlinks are this table read backwards), `attachments`
(note_id, file, ocr_text), and an FTS5 virtual table over title + body + OCR text.

**The index is refreshed by scanning, not watching**, from the Rust side of `present`
rather than by the page. When the window is presented, the notes directory is walked and any file whose mtime or size differs from the indexed copy is
re-read. A scan of a few thousand files costs milliseconds — less than the window takes to
appear — and unlike a watcher it cannot miss an event, leak a thread, or fall out of step
after a `git pull` rewrites files underneath it. A full rebuild from a cold directory is a
couple of seconds, so the index stays disposable: delete it and it comes back.

Search is FTS5 with prefix matching, ranked by BM25 with a recency boost. Two details that
turned out to matter:

- **`bm25()` takes one weight per column, including unindexed ones.** With `path` as the
  first column, `bm25(notes_fts, 4.0, 1.0)` weights *`path`*, not the title — so a passing
  mention outranks a note actually titled after the thing. The weights are `0.0, 4.0, 1.0`.
- **Recency multiplies the score, it does not offset it.** BM25 magnitudes depend on corpus
  size: around 1e-6 in a young vault, units in a large one. A fixed bonus is therefore
  either meaningless or overwhelming, and picking a constant that works at both sizes is not
  possible. Scaling by `1 + 0.25 · e^(−age/30d)` is size-independent, and keeps recency as a
  tie-breaker between comparable matches rather than something that decides them.

The tokenizer is `unicode61 remove_diacritics 2`, so `gesla` finds `gęślą`. One exception
worth knowing: **`ł` does not fold** — it is a distinct letter with no Unicode
decomposition, unlike ą, ć, ę, ń, ó, ś, ż and ź. `zazołc` matches `zażółć`; `zazolc` does
not. Pinned by a test rather than left to be rediscovered.

---

## 5. Sync

A private GitHub repo, driven by `git2`:

- **Pull** on daemon start and on window open if the last pull is older than a few minutes.
- **Commit** on save, debounced ~30 s, message `note: <title>` or `notes: N entries`.
- **Push** when idle for ~60 s, and on daemon shutdown.
- **Conflicts** are rare by construction: attachments are content-hashed so they never
  collide, and one person rarely edits the same note on two machines at once. When one does
  happen, keep both sides as `<name>.md` and `<name>.conflict-<host>.md` and surface a
  banner. Never auto-merge note bodies.

Auth is an SSH key per machine. Sync failures must be silent-but-visible — a status dot in
the tray, never a modal that interrupts capture. **Capture must never block on the network.**

---

## 6. Interaction

One global hotkey (suggest `Super+N`) opens a centered floating window with the cursor in a
single text field. That is the whole interface.

### Capture

| Key | Action |
|---|---|
| *(type)* | First line is the title, the rest is the body |
| `Ctrl+V` | Paste image from clipboard; embeds `![[name]]` and adds it to the strip |
| `#` | Tag autocomplete from existing tags |
| `Ctrl+Enter` | Save and close |
| `Esc` | Close, stashing the buffer as a draft |

Esc never destroys text. The buffer is written to `.photomem/draft.md` and restored on the
next open, cursor at the end, which is what makes the window feel disposable.

**Images in the editor.** A `<textarea>` cannot hold inline images, and swapping it for a
`contenteditable` would cost more in text-editing quality than pictures in the flow are
worth — and the editing surface is the one thing in a notes app that cannot be mediocre. So
the note gets a `![[name]]` reference at the cursor, and the images themselves appear as a
**thumbnail strip beneath the editor**, mirroring the references above it. An embed whose
file is missing shows as a marked gap rather than vanishing.

### Search — one trigger, context decides

**`//` always means "search the index."** What happens to the selected result depends on
where the cursor was:

- **Buffer empty** → results replace the view. Type to filter, Enter opens the note.
  This is the "find my old notes" mode, and it needs no second hotkey.
- **Mid-note** → results appear as an inline picker. Enter inserts `[[that-note]]` at the
  cursor and returns to typing.

Same index, same widget, same keystroke — one thing to build.

`//` is also real text, so: it only fires when preceded by whitespace or start-of-buffer,
which eliminates `https://` entirely. Esc dismisses the picker and leaves a literal `//`
behind, covering pasted `// code comments`. **`[[` is wired to the same picker** — it
matches the on-disk format and carries Obsidian muscle memory.

### Viewing — read-only, append-only

Enter on a search result **displays** the note. It is never editable in the popup. This is
a deliberate constraint: the capture window stays a single-purpose, disposable thing, and
notes accumulate as an honest record of what was believed when. Corrections are new
entries, not rewrites.

This is a *UI* constraint, not a data one. The files are plain markdown and the watcher
picks up outside changes, so fixing a typo means opening the file in any editor. That
escape hatch is what makes the constraint safe to impose.

The failure mode of append-only is reading an old note without learning it was later
proved wrong. A plain backlink is too quiet for that, so **supersession is a typed link**:
writing `supersedes: [[old-note]]` in the new note makes the index mark the old one, and
its read-only view opens with a banner —

```
⚠ Superseded by "Kafka consumer rebalance storm" — 2026-09-04
```

In the `//` picker, `Ctrl+Enter` on a result inserts the reference as a supersedes link
instead of an inline `[[link]]`. The old note is never touched; the relationship lives
entirely in the new one and is derived by the index.

Actions available on a displayed note: copy its `[[link]]`, open the file in `$EDITOR`,
reveal it in a file manager, start a new note that supersedes it.

**Opening an image.** `V` in the viewer, or clicking any thumbnail — including one in the
capture strip — opens the picture in a **separate window sized to the picture**: always
`max_edge` (1600px) on its long side, in the image's own aspect ratio. Images are stored
capped at that, so a full-screen capture opens at exactly its own size, and a smaller one is
scaled up into the same frame rather than the window shrinking and growing as you step
through a note. `Z` or a click toggles fit and 1:1; `←`/`→` step through the note's images; `Esc` closes it
and leaves the capture window as it was. At 1:1 the image stays centred — an image smaller
than its window belongs in the middle, not in a corner — and when it does overflow, the
pane starts scrolled to the centre and drags to pan.

A separate window, because reading the text in a captured Slack thread is the entire point
of keeping the picture, and a 1600px image scaled into a 720px capture window is no more
readable than its thumbnail.

Two things this cost, both worth recording:

- **The window rules have to be split in two.** Every photomem window shares the class, so
  `o.window({ class = "^photomem$" }, { size = "720 400" })` pins the image window to
  720x400 as well — and because a compositor rule outranks the client, it also makes Tauri's
  `set_size` silently do nothing, which looks exactly like Wayland refusing client resizes on
  principle. It is not. But scoping the *whole* rule by title then leaves the image window
  with no `float`, so it tiles instead. Both windows need `float` and `center`; only the
  capture window may have a fixed `size`.
- **Fit scales up, not just down.** On a 4K screen a 1600px capture sits small in the middle
  of an empty window unless fit is allowed to enlarge it — and there, fit is the *larger* of
  the two modes, which is why they are labelled "fit" and "1:1" rather than "fit" and
  "actual size".

**There is no delete.** Not in the popup, not as a CLI command. A note that turned out to
be wrong gets superseded; a note that turned out to be worthless costs a few kilobytes.
This is also honest about git: once a note is pushed, `rm` does not actually remove it —
it stays in history forever, and only a history rewrite would truly erase it. Offering a
delete button would imply a guarantee the storage model cannot make.

A consequence worth stating: attachments are never orphaned, so no garbage collection is
needed.

### Browsing

Two views beyond search, reachable from the search results screen:

1. **Timeline** — entries grouped by day / week / month, toggleable, filterable by tag.
2. **Thumbnail wall** — a scrollable grid of every captured image in time order, filterable
   by tag. For a visual thinker this is the real "map": you navigate by recognizing the
   picture, not by reading node labels.

**Backlinks** are shown at the bottom of every open note. A force-directed graph view is
explicitly deferred — the value in Obsidian's graph is mostly in the backlinks it implies,
and the thumbnail wall is likely to earn its keep far sooner.

---

## 7. Architecture

**Tauri v2.** Rust core, webview UI, ~10 MB binary, one codebase for both platforms.

The alternative considered was **egui** — pure Rust, no webview, ~5 MB, faster cold start.
Rejected on one point: its multiline text editor is merely serviceable (wrapping, undo
granularity, IME, selection), and the editing surface is the one thing in a notes app that
cannot be mediocre. A webview supplies a real text engine for free, and makes the thumbnail
wall trivial. This is worth revisiting only if webview startup latency proves unacceptable.

### Process model

A **long-lived daemon** owns the index, the file watcher, git sync, and a pre-warmed hidden
window. The `photomem` CLI is a thin client that pokes it over a unix socket. Showing the
window is then a matter of unhiding it, not starting a runtime — which is the entire reason
capture feels instant.

**Linux/Hyprland.** Wayland has no protocol for app-registered global hotkeys, so the
binding lives in the compositor config:

```
bind = SUPER, N, exec, photomem capture
windowrulev2 = float, class:^(photomem)$
windowrulev2 = center, class:^(photomem)$
windowrulev2 = stayfocused, class:^(photomem)$
```

The daemon autostarts via `exec-once` or a systemd user unit.

**macOS.** The daemon is a menu bar agent (`LSUIElement`) and registers the hotkey itself
through `RegisterEventHotKey` — no accessibility permission required. Launch at login via a
`LaunchAgent`.

Everything below the window layer is shared. Only hotkey registration, clipboard access, and
autostart differ, and each is a small platform module behind one trait.

---

## 8. Module layout

```
photo-memory/
  crates/
    core/         # note parsing, frontmatter, links, tags, slugs. No I/O.
    config/       # ~/.config/photomem/config.toml
    store/        # file read/write, atomic saves, drafts, attachments
    index/        # SQLite + FTS5, directory scan, search, backlinks, groupings
    images/       # clipboard decode, downscale, WebP encode, thumbs, OCR
    sync/         # git2: pull/commit/push, conflict handling
    platform/     # trait + linux/ and macos/ impls: hotkey, clipboard, autostart
    daemon/       # socket server, wires everything, owns the window
  src-tauri/      # Tauri shell, commands, tray
  ui/             # frontend: editor, search, timeline, thumbnail wall — plain
                  # HTML/CSS/JS, deliberately no bundler and no npm in the build
  DESIGN.md
```

`core` stays pure and has no dependencies on the others — it is where the format lives, and
it should be trivially testable with string in, struct out.

---

## 9. Build order

Each milestone is meant to be usable on its own, so the thing can be lived with early and
the design can be corrected by use rather than by argument.

**M1 — Capture loop.** Tauri window, text field, save to `notes/`, first-line title,
frontmatter, CLI + Hyprland binding. No search, no images. *Usable: it writes notes.*

**M2 — Images.** Clipboard paste, downscale, WebP, thumbnail strip, attachment refs.
*Usable: the actual point of the app.*

**M3 — Index and search.** SQLite/FTS5, directory scan on present, `//` search over an
empty buffer, and the read-only viewer — a search whose results cannot be opened is not
usable, so the viewer moved up from M4 with backlinks left behind.
*Usable: notes become findable.*

**M4 — Links.** `//` and `[[` pickers mid-note, link resolution, backlinks in the viewer,
`supersedes` and its banner. Done.
*Usable: it becomes a connected system rather than a pile.*
The index grew a `links` table and a schema version, because both halves are derived from
every *other* note: a backlink is that table read backwards, and the banner is the same
read filtered to `supersedes`. The version matters — without it an existing index keeps its
recorded mtimes, the scan skips every note as unchanged, and nothing captured before the
upgrade would ever acquire a link. `supersedes` is read from a body line *and* the
frontmatter key §2 documents, since the picker writes the first and a hand-edited note may
carry the second.

**M5 — Sync.** git pull/commit/push, tray status, conflict handling. Done, less the tray.
*Usable: it works on two machines.*
Driving the `git` binary rather than linking libgit2, because the vault is a repo the user
owns and sometimes fixes by hand: sync has to honour the same SSH keys, credential helpers
and `commit.gpgsign` their own commands do. One commit per save, named after the note, so
the history reads as a list of captures. Both directions run on a thread — a capture must
never wait on the network — behind one lock, since a save landing during a pull would
otherwise be two `git` processes fighting over one `index.lock`.

**Conflicts barely exist here, by construction.** Notes are append-only and named for their
own timestamp and slug, so two machines capturing at once touch different files; `pull
--rebase --autostash` merges them without a merge commit. A test covers exactly that case.
What is left is a genuine edit of the same file on two machines, which the UI cannot even
produce — and there the right answer is to stop and let the user resolve it in git, not to
guess.

**M6 — OCR.** tesseract on paste, OCR text into the index. *The differentiator.*

**M7 — Browsing.** Timeline grouping and the thumbnail wall, both filterable by tag. Done.
`Tab` cycles search → wall → timeline → search, one key for all of it. The three group
labels (day, week, month) are computed in Rust and sent with every row, so switching
grouping is a repaint with no round trip and the page still does no date maths — ISO weeks
in particular are the sort of thing a hand-rolled version gets wrong only at the turn of the
year, which is the worst time to find out.

**Tags are inline text, not a field.** They are typed mid-sentence and stay in the note,
because anything that demands a separate field is something you stop doing on the third day.
The whole difficulty is that `#` is ordinary punctuation: a URL fragment, a markdown
heading, an issue number and a CSS colour all contain one. The rules are deliberately narrow
— a tag must start a word, must begin with a letter, and loses its trailing punctuation —
because a false negative costs one tag typed differently, while a false positive sits in the
filter list forever. `#ff0000` is the case that gets through, and it is worth the trade.
Tags fold to lower case so `#Kafka` and `#kafka` are one tag; `#` in the editor completes
from the tags already in use, which is what keeps a free-form vocabulary from becoming five
spellings of the same idea.
The index grew an `attachments` table, populated from `![[name]]` exactly as `links` is from
`[[name]]` — schema version 2, so an existing index is thrown away and rebuilt rather than
left with an empty table it would never fill. One row per *picture*, not per embed: the same
screenshot pasted into two notes is one thing you would recognise, and it belongs to the note
most recently written about it. Tiles with no thumbnail on disk are dropped rather than shown
as gaps — a wall is for recognising pictures, and a placeholder is nothing to recognise,
which is the opposite of the rule inside a note, where a missing embed still shows its
filename because there the filename is the information.

**M8 — macOS.** Platform module, menu bar agent, hotkey, LaunchAgent, packaging.

M1–M4 is the smallest thing worth using daily. M5 can wait longer than it feels like it can;
M6 is the one to protect from being cut when the project gets boring.

---

## 10. Deferred and open

**Deferred deliberately**

- Force-directed graph view — build backlinks and the thumbnail wall first, then see.
- Full-resolution originals behind a modifier key.
- Encryption at rest. The repo is private; that is the current threat model.
- In-app editing of any kind. Decided against: see §6. External editor plus the watcher
  is the escape hatch.

**Settled**

- *In-app editing* — no. Read-only viewing, supersession for corrections (§6).
- *Deletion* — none, at any level. Nothing is removed, so attachment GC is unnecessary (§6).
- *OCR languages* — `["eng"]` to start, Polish later via config plus a re-OCR pass (§3).

No open questions block M1. The next decision that matters is the config file's shape and
location, which can wait until M5 (sync) needs a repo path anyway.
