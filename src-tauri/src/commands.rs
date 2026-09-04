//! The whole API surface the UI has. Four calls.

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

fn vault() -> CmdResult<Vault> {
    let cfg = Config::load().map_err(|e| format!("{e:#}"))?;
    Ok(Vault::new(cfg.vault))
}

/// Save the buffer as a new note and clear the draft. Returns the path, which
/// the UI shows briefly as confirmation that it went somewhere real.
#[tauri::command]
pub fn save_note(body: String) -> CmdResult<String> {
    let note = Note::new(&body).map_err(|e| e.to_string())?;
    let vault = vault()?;
    let path = vault.save(&note).map_err(|e| format!("{e:#}"))?;
    vault.clear_draft().map_err(|e| format!("{e:#}"))?;
    Ok(path.display().to_string())
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
    /// The thumbnail, inlined as a data URL.
    ///
    /// These are a few kilobytes each, so inlining them costs less than opening
    /// Tauri's asset protocol to the whole vault would.
    thumb: String,
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

    Ok(Pasted { thumb: data_url(&attachment.thumb), name })
}

/// Thumbnails for images already referenced in the buffer, so a restored draft
/// shows its pictures again instead of bare filenames.
#[tauri::command]
pub fn thumbnails(names: Vec<String>) -> CmdResult<Vec<Pasted>> {
    let vault = vault()?;
    Ok(names
        .into_iter()
        .filter_map(|name| {
            let bytes = vault.read_thumbnail(&name)?;
            Some(Pasted { name, thumb: data_url(&bytes) })
        })
        .collect())
}

/// A search result as the list renders it.
#[derive(serde::Serialize)]
pub struct HitView {
    path: String,
    title: String,
    /// Pre-formatted for display; the UI does no date maths.
    when: String,
    snippet: String,
}

/// Bring the index in line with the notes on disk.
///
/// Called when the window is presented rather than on a timer: notes change
/// while the window is closed, and a scan of a few thousand files is faster
/// than the window takes to appear.
#[tauri::command]
pub fn refresh(search: State<'_, Search>) -> CmdResult<usize> {
    let vault = vault()?;
    let mut index = search.0.lock().map_err(|_| "index is poisoned".to_string())?;
    index.sync(&vault.notes_dir()).map_err(|e| format!("{e:#}"))?;
    Ok(index.len())
}

#[tauri::command]
pub fn search(search: State<'_, Search>, query: String) -> CmdResult<Vec<HitView>> {
    let index = search.0.lock().map_err(|_| "index is poisoned".to_string())?;
    let hits = index.search(&query, 40).map_err(|e| format!("{e:#}"))?;
    Ok(hits
        .into_iter()
        .map(|h| HitView {
            path: h.path.display().to_string(),
            title: h.title,
            when: h.created.format("%Y-%m-%d %H:%M").to_string(),
            snippet: h.snippet,
        })
        .collect())
}

/// A note opened from search results. Read-only by design (see DESIGN.md §6).
#[derive(serde::Serialize)]
pub struct NoteView {
    title: String,
    when: String,
    /// Body with the title line removed, since the title is displayed separately.
    body: String,
    images: Vec<Pasted>,
}

#[tauri::command]
pub fn open_note(path: String) -> CmdResult<NoteView> {
    let vault = vault()?;
    let path = std::path::PathBuf::from(path);
    // Only ever read notes from inside the vault, whatever the UI passes.
    if !path.starts_with(vault.notes_dir()) {
        return Err("that note is not in the vault".into());
    }

    let text = std::fs::read_to_string(&path).map_err(|e| format!("{e}"))?;
    let note = Note::parse(&text, chrono::Local::now()).map_err(|e| e.to_string())?;

    let images = embedded_names(note.content())
        .into_iter()
        .filter_map(|name| {
            let bytes = vault.read_thumbnail(&name)?;
            Some(Pasted { name, thumb: data_url(&bytes) })
        })
        .collect();

    Ok(NoteView {
        title: note.title().to_string(),
        when: note.created.format("%Y-%m-%d %H:%M").to_string(),
        body: without_embed_lines(note.content()),
        images,
    })
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
    use super::{embedded_names, without_embed_lines};

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
    if let Some((mw, mh)) = monitor_size(&app) {
        let shrink = (mw * 0.95 / w).min(mh * 0.95 / h).min(1.0);
        w *= shrink;
        h *= shrink;
    }

    window.set_size(tauri::LogicalSize::new(w, h)).map_err(|e| e.to_string())?;
    window.center().map_err(|e| e.to_string())?;
    let _ = window.show();
    let _ = window.set_focus();
    Ok(())
}

/// Height of the caption and hints strip under the picture.
const FOOTER_HEIGHT: f64 = 30.0;

fn monitor_size<R: Runtime>(app: &tauri::AppHandle<R>) -> Option<(f64, f64)> {
    let monitor = app.get_webview_window("main")?.current_monitor().ok()??;
    let logical = monitor.size().to_logical::<f64>(monitor.scale_factor());
    Some((logical.width, logical.height))
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
