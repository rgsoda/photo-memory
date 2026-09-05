//! The whole API surface the UI has. Four calls.

use anyhow::Context;
use std::sync::Mutex;

use photomem_config::Config;
use photomem_core::Note;
use photomem_index::Index;
use photomem_store::Vault;
use tauri::{Emitter, Manager, Runtime, State, Window};

/// The index is opened once and reused; reopening it per keystroke would make
/// search-as-you-type noticeably slower than typing.
pub struct Search(pub Mutex<Index>);

/// Errors reach the UI as strings; there is nothing it can do but show them.
type CmdResult<T> = Result<T, String>;

/// How thumbnails should be made when one has to be rebuilt.
fn image_opts() -> photomem_images::Options {
    Config::load().map(|cfg| cfg.image.into()).unwrap_or_default()
}

fn vault() -> CmdResult<Vault> {
    let cfg = Config::load().map_err(|e| format!("{e:#}"))?;
    Ok(Vault::new(cfg.vault))
}

/// Save the buffer as a new note and clear the draft. Returns the path, which
/// the UI shows briefly as confirmation that it went somewhere real.
#[tauri::command]
pub fn save_note<R: Runtime>(app: tauri::AppHandle<R>, body: String) -> CmdResult<String> {
    let note = Note::new(&body).map_err(|e| e.to_string())?;
    let vault = vault()?;
    let path = vault.save(&note).map_err(|e| format!("{e:#}"))?;
    vault.clear_draft().map_err(|e| format!("{e:#}"))?;

    // Only after the note is safely on disk. Sync is a convenience; the capture
    // is the thing that must not be lost, and it now cannot be.
    commit_in_background(&app, note.title().to_string());

    // Indexed here rather than at the next window open, so the note is findable
    // straight away — and so the OCR pass can see the screenshots it embeds,
    // which only enter the index when the note embedding them does.
    refresh_index(&app);
    spawn_ocr_pass(&app);
    Ok(path.display().to_string())
}

/// Commit and push the just-saved note, off the UI thread.
///
/// The window closes a few hundred milliseconds after a save, and a push to a
/// slow remote takes longer than that. Doing this inline would make every
/// capture wait on the network — which is the one thing the design says capture
/// must never do.
fn commit_in_background<R: Runtime>(app: &tauri::AppHandle<R>, title: String) {
    let Some(repo) = sync_repo() else { return };
    let app = app.clone();
    std::thread::spawn(move || {
        let _guard = git_lock().lock();
        let message = photomem_sync::message_for(&title);
        match repo.save(&message) {
            Ok(photomem_sync::Synced::Nothing) => {}
            Ok(photomem_sync::Synced::Committed) => report(&app, "committed", false),
            Ok(photomem_sync::Synced::Pushed) => report(&app, "synced", false),
            Err(e) => report(&app, &format!("sync failed: {e:#}"), true),
        }
    });
}

/// Pull anything captured on another machine, off the UI thread.
///
/// Fired when the window is presented, because that is when the notes are about
/// to be read. It cannot block the window appearing, so the index is refreshed
/// afterwards and the page told to redraw if anything actually arrived.
pub fn pull_in_background<R: Runtime>(app: &tauri::AppHandle<R>) {
    let Some(repo) = sync_repo() else { return };
    let app = app.clone();
    std::thread::spawn(move || {
        let _guard = git_lock().lock();
        match repo.pull() {
            Ok(false) => {}
            Ok(true) => {
                refresh_index(&app);
                report(&app, "pulled new notes", false);
            }
            Err(e) => report(&app, &format!("pull failed: {e:#}"), true),
        }
    });
}

/// The vault as a git repo, if sync is on and it is one.
fn sync_repo() -> Option<photomem_sync::Repo> {
    let cfg = Config::load().ok()?;
    cfg.sync.enabled.then_some(())?;
    photomem_sync::Repo::open(&cfg.vault)
}

/// Serialises git across threads.
///
/// A save while a pull is still running would otherwise leave two `git`
/// processes fighting over one index.lock, and the loser reports a failure that
/// is really just a race.
fn git_lock() -> &'static Mutex<()> {
    static LOCK: std::sync::OnceLock<Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

/// Tell the page what sync did. Also to stderr, since the window is usually
/// gone by the time a push finishes.
fn report<R: Runtime>(app: &tauri::AppHandle<R>, text: &str, is_error: bool) {
    if is_error {
        eprintln!("photomem: {text}");
    }
    let _ = app.emit("photomem://sync", (text, is_error));
    // The window is usually gone by the time a push finishes, so the tray is
    // where this actually gets read.
    crate::tray::set_status(app, text, is_error);
}

#[tauri::command]
pub fn save_draft(text: String) -> CmdResult<()> {
    vault()?.save_draft(&text).map_err(|e| format!("{e:#}"))
}

#[tauri::command]
pub fn load_draft() -> CmdResult<String> {
    Ok(vault()?.load_draft())
}

/// Hide the window without exiting: the process stays warm for the next hotkey.
/// The draft is written first so Escape can never lose what was typed.
#[tauri::command]
pub fn dismiss<R: Runtime>(window: Window<R>, text: String) -> CmdResult<()> {
    vault()?.save_draft(&text).map_err(|e| format!("{e:#}"))?;
    let _ = window.get_webview_window("main").map(|w| w.hide());
    Ok(())
}

/// An image stored and ready to embed.
#[derive(serde::Serialize)]
pub struct Pasted {
    /// Filename to write into the note as `![[name]]`.
    name: String,
    /// The thumbnail, inlined as a data URL. `None` when the image itself is
    /// gone — the viewer draws a marked gap for it, because an embed that
    /// silently vanishes looks like a note that never had a picture.
    ///
    /// These are a few kilobytes each, so inlining them costs less than opening
    /// Tauri's asset protocol to the whole vault would.
    thumb: Option<String>,
}

fn data_url(bytes: &[u8]) -> String {
    use base64::Engine;
    format!("data:image/webp;base64,{}", base64::engine::general_purpose::STANDARD.encode(bytes))
}

/// Take the image currently on the clipboard, store it, and hand back its name.
///
/// The clipboard is read here rather than in the webview because WebKitGTK and
/// WKWebView disagree about what a paste event carries; the system clipboard is
/// the one source both platforms agree on.
#[tauri::command]
pub fn paste_image() -> CmdResult<Pasted> {
    let mut clipboard = arboard::Clipboard::new().map_err(|e| format!("clipboard: {e}"))?;
    let image = clipboard.get_image().map_err(|e| {
        eprintln!("photomem: clipboard image read failed: {e}");
        "no image on the clipboard".to_string()
    })?;

    let cfg = Config::load().map_err(|e| format!("{e:#}"))?;
    let attachment = photomem_images::from_rgba(
        image.width as u32,
        image.height as u32,
        &image.bytes,
        cfg.image.into(),
    )
    .map_err(|e| format!("{e:#}"))?;

    let vault = Vault::new(cfg.vault);
    let date = chrono::Local::now().format("%Y-%m-%d").to_string();
    let name = vault.save_attachment(&attachment, &date).map_err(|e| format!("{e:#}"))?;

    Ok(Pasted { thumb: Some(data_url(&attachment.thumb)), name })
}

/// Thumbnails for images already referenced in the buffer, so a restored draft
/// shows its pictures again instead of bare filenames.
#[tauri::command]
pub fn thumbnails(names: Vec<String>) -> CmdResult<Vec<Pasted>> {
    let vault = vault()?;
    let opts = image_opts();
    Ok(names
        .into_iter()
        .map(|name| {
            let thumb = vault.thumbnail(&name, opts).map(|b| data_url(&b));
            Pasted { name, thumb }
        })
        .collect())
}

/// One row of the timeline, with the heading it belongs under at each
/// granularity.
///
/// All three labels are computed here rather than in the page: switching
/// grouping is then a repaint with no round trip, and the UI still does no date
/// maths — including ISO weeks, which are exactly the kind of thing a
/// hand-rolled version gets wrong at the turn of the year.
#[derive(serde::Serialize)]
pub struct TimelineItem {
    path: String,
    title: String,
    /// Time of day, since the date is already in the heading above.
    at: String,
    day: String,
    week: String,
    month: String,
}

#[tauri::command]
pub fn timeline(search: State<'_, Search>, tag: Option<String>) -> CmdResult<Vec<TimelineItem>> {
    const LIMIT: usize = 2000;

    let index = search.0.lock().map_err(|_| "index is poisoned".to_string())?;
    Ok(index
        .timeline(tag.as_deref(), LIMIT)
        .map_err(|e| format!("{e:#}"))?
        .into_iter()
        .map(|note| {
            // `Datelike` is what carries iso_week; ISO years diverge from
            // calendar years in the last days of December, which is the whole
            // reason for asking chrono instead of formatting one by hand.
            let iso = chrono::Datelike::iso_week(&note.created);
            TimelineItem {
                path: note.path.display().to_string(),
                title: note.title,
                at: note.created.format("%H:%M").to_string(),
                day: note.created.format("%A %-d %B %Y").to_string(),
                week: format!("Week {} · {}", iso.week(), iso.year()),
                month: note.created.format("%B %Y").to_string(),
            }
        })
        .collect())
}

/// A tag and how many notes carry it.
#[derive(serde::Serialize)]
pub struct TagCount {
    tag: String,
    count: usize,
}

/// Every tag in use, most-used first.
#[tauri::command]
pub fn tags(search: State<'_, Search>) -> CmdResult<Vec<TagCount>> {
    let index = search.0.lock().map_err(|_| "index is poisoned".to_string())?;
    Ok(index
        .tags()
        .map_err(|e| format!("{e:#}"))?
        .into_iter()
        .map(|(tag, count)| TagCount { tag, count })
        .collect())
}

/// One tile on the thumbnail wall.
#[derive(serde::Serialize)]
pub struct WallItem {
    name: String,
    thumb: String,
    /// The note the picture belongs to, so Enter can open it.
    path: String,
    title: String,
    when: String,
}

/// Every captured image, newest first.
///
/// Tiles whose thumbnail is missing are dropped rather than shown as gaps: the
/// wall is a way of finding a note by recognising its picture, and a row of
/// placeholders is nothing to recognise. In the note itself a missing embed is
/// still shown, because there the filename is the information.
#[tauri::command]
pub fn wall(search: State<'_, Search>, tag: Option<String>) -> CmdResult<Vec<WallItem>> {
    const LIMIT: usize = 500;

    let vault = vault()?;
    let opts = image_opts();
    let index = search.0.lock().map_err(|_| "index is poisoned".to_string())?;
    Ok(index
        .wall(tag.as_deref(), LIMIT)
        .map_err(|e| format!("{e:#}"))?
        .into_iter()
        .filter_map(|shot| {
            let bytes = vault.thumbnail(&shot.name, opts)?;
            Some(WallItem {
                name: shot.name,
                thumb: data_url(&bytes),
                path: shot.note.display().to_string(),
                title: shot.title,
                when: shot.created.format("%Y-%m-%d").to_string(),
            })
        })
        .collect())
}

/// A search result as the list renders it.
#[derive(serde::Serialize)]
pub struct HitView {
    path: String,
    /// The `[[link]]` target for this note: its filename without `.md`.
    name: String,
    title: String,
    /// Pre-formatted for display; the UI does no date maths.
    when: String,
    snippet: String,
}

/// Sync the index with the notes directory, reporting failure to stderr.
///
/// Nothing the caller can do about a failure except carry on with a stale
/// index, which is better than refusing to show the window.
pub fn refresh_index<R: Runtime>(app: &tauri::AppHandle<R>) {
    let Ok(vault) = vault() else {
        eprintln!("photomem: cannot locate the vault; index not refreshed");
        return;
    };
    let state = app.state::<Search>();
    let Ok(mut index) = state.0.lock() else {
        eprintln!("photomem: index lock poisoned; not refreshed");
        return;
    };
    match index.sync(&vault.notes_dir()) {
        Ok(stats) if stats.changed() => eprintln!("photomem: index {stats:?}"),
        Ok(_) => {}
        Err(e) => eprintln!("photomem: index sync failed: {e:#}"),
    }
}

/// Read any screenshots that have not been read yet, in the background.
///
/// Capture must never wait for this, so it runs off the caller's thread and
/// takes the index lock only to ask what is outstanding and again to record
/// each answer — never while a recogniser is running. A search typed during a
/// pass is therefore not stuck behind it.
pub fn spawn_ocr_pass<R: Runtime>(app: &tauri::AppHandle<R>) {
    let app = app.clone();
    std::thread::spawn(move || {
        if let Err(e) = ocr_pass(&app) {
            eprintln!("photomem: OCR pass stopped: {e:#}");
        }
    });
}

fn ocr_pass<R: Runtime>(app: &tauri::AppHandle<R>) -> anyhow::Result<()> {
    let cfg = Config::load()?;
    // Normalised here too, so that "what is stale" is asked in exactly the
    // terms the recogniser records its answers in.
    let languages = photomem_images::ocr::languages_or_default(&cfg.ocr.languages);
    let asked = languages.join("+");
    let vault = Vault::new(cfg.vault);

    let pending = {
        let state = app.state::<Search>();
        let index = state.0.lock().map_err(|_| anyhow::anyhow!("index is poisoned"))?;
        index.needs_ocr(&asked)?
    };

    for name in pending {
        // A note can name an image that is not on disk. That is a gap for the
        // viewer to show, not a reason to stop reading the rest.
        let Some(bytes) = vault.read_attachment(&name) else { continue };

        // A failure here means the recogniser itself is unavailable — a Linux
        // box with no tesseract — so stop rather than repeat one error for
        // every image in the vault. The next pass retries, which is what should
        // happen once it is installed.
        let found = photomem_images::ocr::recognize(&bytes, &languages)
            .with_context(|| format!("reading {name}"))?;

        let state = app.state::<Search>();
        let index = state.0.lock().map_err(|_| anyhow::anyhow!("index is poisoned"))?;
        index.set_ocr(&name, &found.text, &found.languages)?;
    }
    Ok(())
}

#[tauri::command]
pub fn search(search: State<'_, Search>, query: String) -> CmdResult<Vec<HitView>> {
    let index = search.0.lock().map_err(|_| "index is poisoned".to_string())?;
    let hits = index.search(&query, 40).map_err(|e| format!("{e:#}"))?;
    Ok(hits
        .into_iter()
        .map(|h| HitView {
            name: link_name(&h.path),
            path: h.path.display().to_string(),
            title: h.title,
            when: h.created.format("%Y-%m-%d %H:%M").to_string(),
            snippet: h.snippet,
        })
        .collect())
}

/// Another note pointing at the one being read.
#[derive(serde::Serialize)]
pub struct RefView {
    name: String,
    title: String,
    when: String,
}

impl From<photomem_index::Ref> for RefView {
    fn from(r: photomem_index::Ref) -> RefView {
        // Date only: a backlink is a pointer, not an event, and the time of day
        // it was written says nothing useful about the note being read.
        RefView { name: r.name, title: r.title, when: r.created.format("%Y-%m-%d").to_string() }
    }
}

/// A note opened from search results. Read-only by design (see DESIGN.md §6).
#[derive(serde::Serialize)]
pub struct NoteView {
    /// This note's own `[[link]]` target, for the "copy link" affordance.
    name: String,
    title: String,
    when: String,
    /// Body with the title line removed, since the title is displayed separately.
    body: String,
    images: Vec<Pasted>,
    /// The newest note declaring it corrects this one, if any. The failure mode
    /// of append-only is reading something later proved wrong without being
    /// told, which is what this exists to prevent.
    superseded_by: Option<RefView>,
    /// Notes that reference this one.
    backlinks: Vec<RefView>,
}

#[tauri::command]
pub fn open_note(search: State<'_, Search>, path: String) -> CmdResult<NoteView> {
    let vault = vault()?;
    let path = std::path::PathBuf::from(path);
    // Only ever read notes from inside the vault, whatever the UI passes.
    if !path.starts_with(vault.notes_dir()) {
        return Err("that note is not in the vault".into());
    }
    let index = search.0.lock().map_err(|_| "index is poisoned".to_string())?;
    read_note(&vault, &index, &path)
}

/// Open the note a `[[link]]` points at.
///
/// Links name a note the way the filename does, so this is a lookup rather than
/// a search: a link that resolves to nothing is a dangling link, and saying so
/// is more useful than showing the nearest match.
#[tauri::command]
pub fn open_link(search: State<'_, Search>, name: String) -> CmdResult<NoteView> {
    let vault = vault()?;
    let name = safe_link(&name).ok_or_else(|| format!("{name} is not a note name"))?;
    let path = vault.notes_dir().join(format!("{name}.md"));
    if !path.is_file() {
        return Err(format!("no note called {name}"));
    }
    let index = search.0.lock().map_err(|_| "index is poisoned".to_string())?;
    read_note(&vault, &index, &path)
}

/// The `[[link]]` target for a note: its filename without the `.md`.
fn link_name(path: &std::path::Path) -> String {
    path.file_stem().unwrap_or_default().to_string_lossy().into_owned()
}

/// Reject anything that is not a bare note name, so a link cannot walk out of
/// the notes directory.
fn safe_link(name: &str) -> Option<&str> {
    let name = name.trim().trim_end_matches(".md");
    let bad = name.is_empty()
        || name.contains('/')
        || name.contains('\\')
        || name.contains("..")
        || std::path::Path::new(name).is_absolute();
    (!bad).then_some(name)
}

fn read_note(vault: &Vault, index: &Index, path: &std::path::Path) -> CmdResult<NoteView> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{e}"))?;
    let note = Note::parse(&text, chrono::Local::now()).map_err(|e| e.to_string())?;
    let name = link_name(path);

    // Rebuilt on demand, not filtered out: a note pulled from another machine
    // has its images and none of the thumbnails, and dropping them here is what
    // made a synced note look like it never had a picture.
    let opts = image_opts();
    let images = embedded_names(note.content())
        .into_iter()
        .map(|name| {
            let thumb = vault.thumbnail(&name, opts).map(|b| data_url(&b));
            Pasted { name, thumb }
        })
        .collect();

    // Both are claims made by *other* notes, so they come from the index rather
    // than from the file in hand — the whole reason M4 is index work.
    let superseded_by = index.superseded_by(&name).map_err(|e| format!("{e:#}"))?;
    let backlinks = index.backlinks(&name).map_err(|e| format!("{e:#}"))?;

    let backlinks = other_backlinks(backlinks, superseded_by.as_ref());

    Ok(NoteView {
        name,
        title: note.title().to_string(),
        when: note.created.format("%Y-%m-%d %H:%M").to_string(),
        body: without_embed_lines(note.content()),
        images,
        superseded_by: superseded_by.map(RefView::from),
        backlinks,
    })
}

/// Backlinks minus the note the banner already names.
///
/// The banner states it loudly above the title; repeating it in the list below
/// would say the same thing twice in a 720px window. Every *other* note that
/// points here still appears — including an older correction that the banner,
/// which names only the newest, did not pick.
fn other_backlinks(
    backlinks: Vec<photomem_index::Ref>,
    banner: Option<&photomem_index::Ref>,
) -> Vec<RefView> {
    backlinks
        .into_iter()
        .filter(|r| banner.is_none_or(|b| b.name != r.name))
        .map(RefView::from)
        .collect()
}

/// Drop lines that are nothing but an embed.
///
/// The viewer renders those images directly underneath, so leaving the raw
/// `![[…]]` in the text shows the reader the filename twice and the picture
/// once. An embed used mid-sentence is left alone.
fn without_embed_lines(body: &str) -> String {
    body.lines()
        .filter(|line| {
            let t = line.trim();
            !(t.starts_with("![[") && t.ends_with("]]") && t.matches("![[").count() == 1)
        })
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}

/// Attachment names embedded as `![[name]]`, in order, without duplicates.
fn embedded_names(body: &str) -> Vec<String> {
    let mut names = Vec::new();
    for (_, rest) in body.match_indices("![[").map(|(i, _)| (i, &body[i + 3..])) {
        if let Some(end) = rest.find("]]") {
            let name = rest[..end].to_string();
            if !names.contains(&name) {
                names.push(name);
            }
        }
    }
    names
}

#[cfg(test)]
mod tests {
    use super::{embedded_names, other_backlinks, safe_link, without_embed_lines};

    fn reference(name: &str) -> photomem_index::Ref {
        photomem_index::Ref {
            path: std::path::PathBuf::from(format!("/notes/{name}.md")),
            name: name.to_string(),
            title: name.to_string(),
            created: chrono::Local::now(),
        }
    }

    #[test]
    fn the_banner_note_is_not_repeated_in_the_backlinks() {
        let refs = vec![reference("second-fix"), reference("first-fix"), reference("mentions")];
        let banner = reference("second-fix");

        let shown: Vec<String> =
            other_backlinks(refs, Some(&banner)).into_iter().map(|r| r.name).collect();
        // The older correction must survive: the banner names only the newest,
        // and an append-only record that hid one of its own corrections would
        // be doing the exact thing M4 exists to prevent.
        assert_eq!(shown, ["first-fix", "mentions"]);
    }

    #[test]
    fn with_no_correction_every_backlink_is_listed() {
        let refs = vec![reference("a"), reference("b")];
        assert_eq!(other_backlinks(refs, None).len(), 2);
    }


    #[test]
    fn link_names_are_bare_and_extensionless() {
        assert_eq!(safe_link("2026-09-04-1703-deploy"), Some("2026-09-04-1703-deploy"));
        // Obsidian habits, and our own copy-link affordance, both produce these.
        assert_eq!(safe_link(" 2026-09-04-1703-deploy.md "), Some("2026-09-04-1703-deploy"));
        assert_eq!(safe_link("../../etc/passwd"), None);
        assert_eq!(safe_link("/etc/passwd"), None);
        assert_eq!(safe_link("sub/note"), None);
        assert_eq!(safe_link("  "), None);
    }

    #[test]
    fn strips_standalone_embed_lines_only() {
        let body = "before\n![[a.webp]]\nsee ![[b.webp]] inline\nafter";
        assert_eq!(without_embed_lines(body), "before\nsee ![[b.webp]] inline\nafter");
    }

    #[test]
    fn a_note_that_is_only_an_image_has_an_empty_body() {
        assert_eq!(without_embed_lines("![[a.webp]]"), "");
    }

    #[test]
    fn reads_embeds_in_order_without_duplicates() {
        let body = "One ![[a.webp]]\ntext\n![[b.webp]] and ![[a.webp]] again\n";
        assert_eq!(embedded_names(body), vec!["a.webp", "b.webp"]);
    }

    #[test]
    fn ignores_unterminated_or_absent_embeds() {
        assert!(embedded_names("no images here").is_empty());
        assert!(embedded_names("![[never closed").is_empty());
    }
}

/// The stored image at full size, with the window size it wants.
///
/// Around 100 KB, so ~140 KB as base64 — worth it to keep the asset protocol
/// closed and every image path going through one guarded reader.
#[derive(serde::Serialize)]
pub struct FullImage {
    url: String,
    width: f64,
    height: f64,
}

#[tauri::command]
pub fn read_image(name: String) -> CmdResult<FullImage> {
    let vault = vault()?;
    let bytes = vault.read_attachment(&name).ok_or_else(|| format!("no image named {name}"))?;
    let (w, h) = photomem_images::dimensions(&bytes).ok_or("that file is not a WebP image")?;
    let (width, height) = window_size_for(w, h);
    Ok(FullImage { url: data_url(&bytes), width, height })
}

/// The window an image of `w`x`h` should open at.
///
/// Always `MAX_EDGE` on the long side, whatever the picture measures: images
/// are stored capped at that, so a full-screen capture opens at its own size
/// and a smaller one is scaled up to the same frame rather than opening a
/// window that shrinks and grows as you step through a note.
fn window_size_for(w: u32, h: u32) -> (f64, f64) {
    let edge = photomem_images::MAX_EDGE as f64;
    let scale = edge / w.max(h) as f64;
    (w as f64 * scale, h as f64 * scale)
}

#[cfg(test)]
mod size_tests {
    use super::window_size_for;

    #[test]
    fn a_stored_image_opens_at_its_own_size() {
        assert_eq!(window_size_for(1600, 900), (1600.0, 900.0));
    }

    #[test]
    fn a_smaller_image_is_scaled_up_to_the_same_long_edge() {
        assert_eq!(window_size_for(800, 600), (1600.0, 1200.0));
        // Portrait: the long edge is the height.
        assert_eq!(window_size_for(600, 800), (1200.0, 1600.0));
    }
}


/// Open the image viewer window, or point the existing one at another image.
///
/// A separate window rather than an overlay in the capture window: a compositor
/// honours the size a window asks for when it is *created*, but not a resize
/// requested later — Hyprland accepts `set_size` and ignores it. So the way to
/// get a big window is to open one that was big to begin with.
#[tauri::command]
pub fn open_image<R: Runtime>(
    app: tauri::AppHandle<R>,
    names: Vec<String>,
    index: usize,
) -> CmdResult<()> {
    if names.is_empty() {
        return Err("no images to show".into());
    }

    if let Some(window) = app.get_webview_window(IMAGE_WINDOW) {
        window.emit_to(IMAGE_WINDOW, "photomem://show", (&names, index)).map_err(|e| e.to_string())?;
        let _ = window.show();
        let _ = window.set_focus();
        return Ok(());
    }

    let query = format!(
        "image.html?i={index}&n={}",
        urlencode(&serde_json::to_string(&names).map_err(|e| e.to_string())?)
    );

    // Sized from the first image once it is known; the window is created small
    // and resized by `fit_image_window` as soon as the page has its picture,
    // which avoids reading the file twice.
    tauri::WebviewWindowBuilder::new(&app, IMAGE_WINDOW, tauri::WebviewUrl::App(query.into()))
        .title("photomem — image")
        .decorations(false)
        .center()
        .focused(true)
        .visible(false)
        // The capture window is always-on-top, so a normal-level window opens
        // *underneath* the thing that asked for it. macOS honours window levels
        // strictly; Hyprland's own stacking rules hid this on Linux.
        .always_on_top(true)
        .build()
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Resize the image window to the picture it is showing, then reveal it.
///
/// Called by the page rather than at build time so that stepping to the next
/// image reshapes the window too. The window stays hidden until this runs, so
/// it never appears at the wrong size first.
#[tauri::command]
pub fn fit_image_window<R: Runtime>(
    app: tauri::AppHandle<R>,
    width: f64,
    height: f64,
) -> CmdResult<()> {
    let window = app.get_webview_window(IMAGE_WINDOW).ok_or("no image window")?;

    // Never larger than the screen it has to fit on.
    let (mut w, mut h) = (width, height + FOOTER_HEIGHT);
    let screen = monitor_geometry(&app);
    if let Some((_, _, mw, mh)) = screen {
        let shrink = (mw * 0.95 / w).min(mh * 0.95 / h).min(1.0);
        w *= shrink;
        h *= shrink;
    }

    window.set_size(tauri::LogicalSize::new(w, h)).map_err(|e| e.to_string())?;

    // Centred from the size just asked for, rather than by `center()`, which
    // measures the window as it currently is. On macOS the resize above is
    // dispatched to the main thread and has not landed yet, so `center()`
    // centres the old, smaller geometry and leaves the window sitting off to
    // one side. Falls back to `center()` only if the monitor is unreadable.
    match screen {
        Some((mx, my, mw, mh)) => {
            let at = tauri::LogicalPosition::new(mx + (mw - w) / 2.0, my + (mh - h) / 2.0);
            window.set_position(at).map_err(|e| e.to_string())?;
        }
        None => window.center().map_err(|e| e.to_string())?,
    }
    let _ = window.show();
    let _ = window.set_focus();
    Ok(())
}

/// Height of the caption and hints strip under the picture.
const FOOTER_HEIGHT: f64 = 30.0;

/// Position and size of the monitor the capture window is on, in logical
/// pixels. The position matters: centring on a second monitor has to start from
/// that monitor's origin, not the desktop's.
fn monitor_geometry<R: Runtime>(app: &tauri::AppHandle<R>) -> Option<(f64, f64, f64, f64)> {
    let monitor = app.get_webview_window("main")?.current_monitor().ok()??;
    let scale = monitor.scale_factor();
    let size = monitor.size().to_logical::<f64>(scale);
    let at = monitor.position().to_logical::<f64>(scale);
    Some((at.x, at.y, size.width, size.height))
}

#[tauri::command]
pub fn close_image<R: Runtime>(app: tauri::AppHandle<R>) -> CmdResult<()> {
    if let Some(window) = app.get_webview_window(IMAGE_WINDOW) {
        window.close().map_err(|e| e.to_string())?;
    }
    Ok(())
}

const IMAGE_WINDOW: &str = "image";

/// Enough escaping for a filename in a query string. Attachment names are
/// `date-hash.webp`, but a hand-edited note can say anything.
fn urlencode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}
